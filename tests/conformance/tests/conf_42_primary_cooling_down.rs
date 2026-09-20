//! CONF-42 (spec §6's producer table, R5-1's R4-G2 ruling / DESIGN §12.8):
//! **`plan_switch.reason: primary_cooling_down` is produced, in the trace
//! row only.** When ADR-011's cooldown projection refuses the family's
//! primary route *before any attempt* — so the request is served by the
//! family's overflow route while the account state stays `primary` — the
//! request's own record carries `plan_switch{reason: primary_cooling_down}`
//! with `failover_from` naming the abandoned route, and **nothing else**:
//! no `plan.switched` event and no `plan_state` move, because a cooldown is
//! route availability, not a verdict on the plan (spec §4.6 rule 3 — only
//! an upstream `403 quota_exhausted` may move the account).
//!
//! Reaching the path deterministically: a `403 quota_exhausted` on the
//! primary provider under an **out-of-family model** (`m2`) demotes the
//! provider for 60s (ADR-011 item 4, provider-wide) while leaving the
//! family's account state untouched (`candidate != policy.primary`, so no
//! spill). The very next family request then resolves to a primary the
//! projection refuses — the produced case, with no sleeps and no probe
//! window to reason about.
//!
//! Both forwarding paths are witnessed (the buffered engine and the
//! streaming twin share the guard but not the walk), plus two controls:
//! a healthy primary produces no displacement markers (negative control),
//! and a cooldown skip of a route that is *not* the family's primary sets
//! `failover_from` but keeps `plan_switch` null (the §6 scoping: the
//! cooldown displaced the client's own choice, not the family's account).

#![forbid(unsafe_code)]

use router_conformance::testkit::{self, CannedResponse, PlanRig, SseChunk};

/// The default policy body, matching the rig's (spill, probe recovery,
/// `cooldown: 0s` — the zero cooldown removes "the family's own cooldown
/// has not elapsed" as an alternative explanation everywhere below).
const POLICY: &str =
    "  family: m1\n  primary: p-plan/m1\n  overflow: p-api/m1\n  on_primary_exhausted: spill\n  recover: probe\n  cooldown: 0s";

/// The demotion trigger: the classifier's QUOTA_EXHAUSTED wording with a
/// 60s `Retry-After`, so the ADR-011 provider demotion outlives the test.
/// Fired at out-of-family model `m2` it demotes the provider **without**
/// moving the family; fired at the family's primary (`m1`) it does both.
fn quota_403(retry_after_s: u64) -> CannedResponse {
    CannedResponse::json(
        403,
        "Forbidden",
        br#"{"error":{"message":"You have exceeded your current quota","type":"insufficient_quota","code":"insufficient_quota"}}"#,
    )
    .with_header("retry-after", &retry_after_s.to_string())
}

/// The plan-first config with two additions over the shared rig: each
/// provider also serves out-of-family model `m2` (the handle the tests
/// use to demote a provider without touching the family's account
/// state), and the metered provider's `m1` is priced at a distinct
/// `input_miss` (0.002, the shared rig's table) so a displacement's
/// `switch_cost_nano` proves it priced at the DESTINATION route's table.
fn config_yaml(plan_port: u16, api_port: u16, listen_port: u16) -> String {
    let model = |id: &str, miss: &str| {
        format!(
            "      - id: {id}\n        context: 128k\n        price: {{ input_miss: {miss}, input_hit: 0.0001, cache_write: 0.0, output: 0.002, peak: {{ multiplier: 1.0, windows: [] }} }}\n        source: \"mock upstream (no price; test fixture)\"\n"
        )
    };
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: p-plan
    base_url: http://127.0.0.1:{plan_port}/v1
    api_key_env: CONF_PF_PLAN_KEY
    wire_api: chat
    supports: [chat]
    account: coding_plan
    models:
{m1p}{m2p}
  - name: p-api
    base_url: http://127.0.0.1:{api_port}/v1
    api_key_env: CONF_PF_API_KEY
    wire_api: chat
    supports: [chat]
    account: api
    models:
{m1a}{m2a}
aliases: {{}}
plugins: []
fallback: []

plan_policy:
{POLICY}
"#,
        m1p = model("m1", "0.001"),
        m2p = model("m2", "0.001"),
        m1a = model("m1", "0.002"),
        m2a = model("m2", "0.002"),
    )
}

async fn rig(tag: &str) -> PlanRig {
    let dir = testkit::tempdir(tag);
    let plan = testkit::MockUpstream::start().await.unwrap();
    let api = testkit::MockUpstream::start().await.unwrap();
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    std::fs::write(
        dir.join("config.yaml"),
        config_yaml(plan.addr.port(), api.addr.port(), listen_port),
    )
    .unwrap();
    std::env::set_var("CONF_PF_PLAN_KEY", "sk-plan");
    std::env::set_var("CONF_PF_API_KEY", "sk-api");
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    PlanRig {
        plan,
        api,
        listen_addr,
        dir,
        serve_task,
    }
}

/// A buffered family request (the shared rig's shape).
fn post_family(addr: &str, session: &str, turn: u32) -> (u16, Vec<u8>, Vec<(String, String)>) {
    let body = format!(
        r#"{{"model":"p-plan/m1","messages":[{{"role":"user","content":"turn {turn}"}}],"prompt_cache_key":"{session}","stream":false}}"#
    );
    testkit::http_post(addr, "/v1/chat/completions", body.as_bytes(), &[])
}

/// A buffered out-of-family request against `<provider>/m2` (no session):
/// the provider-demotion handle.
fn post_m2(addr: &str, provider: &str) -> (u16, Vec<u8>, Vec<(String, String)>) {
    let body = format!(
        r#"{{"model":"{provider}/m2","messages":[{{"role":"user","content":"demote"}}],"stream":false}}"#
    );
    testkit::http_post(addr, "/v1/chat/completions", body.as_bytes(), &[])
}

/// A streaming family request with usage requested (the ledger's source).
fn post_family_stream(
    addr: &str,
    session: &str,
    turn: u32,
) -> (u16, Vec<u8>, Vec<(String, String)>) {
    let body = format!(
        r#"{{"model":"p-plan/m1","messages":[{{"role":"user","content":"turn {turn}"}}],"prompt_cache_key":"{session}","stream":true,"stream_options":{{"include_usage":true}}}}"#
    );
    testkit::http_post(addr, "/v1/chat/completions", body.as_bytes(), &[])
}

/// A chat SSE stream whose final data event carries usage (100 prompt
/// tokens) — CONF-30's carrier shape.
fn sse_with_usage() -> Vec<SseChunk> {
    vec![
        SseChunk::event(b"data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\n"),
        SseChunk::event(
            b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":5,\"total_tokens\":105,\"prompt_tokens_details\":{\"cached_tokens\":20}}}\n\n",
        ),
        SseChunk::event(b"data: [DONE]\n\n"),
    ]
}

fn sse_bytes() -> Vec<u8> {
    sse_with_usage()
        .iter()
        .flat_map(|c| c.bytes.clone())
        .collect()
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Every stored event as (kind_raw, payload), read after the server stopped.
fn events(dir: &std::path::Path) -> Vec<(String, serde_json::Value)> {
    let store = router_store::SqliteStore::open(&dir.join("state/router.db")).unwrap();
    use router_core::store::{Query, QueryRow, Store as _};
    let QueryRow::Events(rows) = store.query(Query::AllEvents).unwrap() else {
        panic!("events query");
    };
    rows.into_iter().map(|r| (r.kind_raw, r.payload)).collect()
}

/// Every trace record, in write order.
fn trace_records(dir: &std::path::Path) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir.join("state/traces")).expect("trace dir") {
        let entry = entry.unwrap();
        if entry.path().extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        for line in std::fs::read_to_string(entry.path()).unwrap().lines() {
            out.push(serde_json::from_str(line).expect("record json"));
        }
    }
    out
}

fn plan_switch_count(dir: &std::path::Path) -> usize {
    events(dir)
        .iter()
        .filter(|(k, _)| k == "plan.switched")
        .count()
}

/// The produced case, buffered path: the family's account state is
/// `primary`, the guard passes the primary, and ADR-011's cooldown refuses
/// it before any attempt — the overflow route serves the request and the
/// record says why, with the switch priced by the session's ledger.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_42_cooling_primary_is_displaced_with_the_reason_and_no_state_move() {
    let rig = rig("conf42-cool").await;
    // Turn 1 on the healthy primary: the ledger learns 100 prefix tokens,
    // so the displacement below carries a price rather than two nulls.
    rig.plan.queue(testkit::plan_ok("t1"));
    // The provider-wide demotion: quota-exhausted 403 on `p-plan/m2` —
    // same provider, out-of-family model, so the family stays `primary`.
    rig.plan.queue(quota_403(60));
    // What the displaced turn 2 is served by.
    rig.api.queue(testkit::plan_ok("served-by-overflow"));

    let (s1, _b, _h) = post_family(&rig.listen_addr, "S1", 1);
    assert_eq!(s1, 200);

    let (s2, _b2, _h2) = post_m2(&rig.listen_addr, "p-plan");
    assert_eq!(s2, 502, "the m2 walk exhausts its (single-candidate) chain");

    // The produced case: no probe window, no state move — only the
    // projection refusing the primary's provider.
    let (s3, _b3, h3) = post_family(&rig.listen_addr, "S1", 2);
    assert_eq!(
        s3, 200,
        "the request is still served (by the overflow route)"
    );
    assert_eq!(
        rig.plan.requests().len(),
        2,
        "m1 reached the plan mock only for turn 1; the m2 403 is the other one"
    );
    assert_eq!(rig.api.requests().len(), 1, "the overflow answered turn 2");
    assert_eq!(
        header(&h3, "x-router-failover-from"),
        Some("p-plan/m1"),
        "the skip abandoned the primary route (spec §6 row 2)"
    );

    let dir = rig.stop();

    // No state transition: a cooldown is route availability, not a verdict
    // on the plan — `plan.switched`'s only writer never ran.
    assert_eq!(
        plan_switch_count(&dir),
        0,
        "a plan_switch with no plan.switched behind it (spec §6)"
    );

    // The trace row: the displacement, priced by the session's ledger
    // (100 tokens x p-api input_miss 0.002/1K = 200_000 nano).
    let rec = trace_records(&dir)
        .into_iter()
        .find(|r| r["identity"]["session"] == "S1" && r["identity"]["turn_index"] == 2)
        .expect("turn 2 record");
    assert_eq!(rec["decision"]["provider"], "p-api");
    assert_eq!(rec["decision"]["requested_model"], "p-plan/m1");
    assert_eq!(rec["result"]["failover_from"], "p-plan/m1");
    let ps = &rec["result"]["plan_switch"];
    assert_eq!(ps["from"], "p-plan/m1");
    assert_eq!(ps["to"], "p-api/m1");
    assert_eq!(ps["reason"], "primary_cooling_down");
    assert_eq!(ps["probe"], false);
    assert_eq!(ps["reprefill_tokens"], 100);
    assert_eq!(ps["switch_cost_nano"], 200_000);
}

/// Negative control: a healthy primary produces neither marker — the value
/// is the cooldown skip's, not the plan policy's mere presence.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_42_healthy_primary_produces_no_displacement_markers() {
    let rig = rig("conf42-healthy").await;
    rig.plan.queue(testkit::plan_ok("t1"));

    let (s, _b, h) = post_family(&rig.listen_addr, "S1", 1);
    assert_eq!(s, 200);
    assert_eq!(
        header(&h, "x-router-failover-from"),
        None,
        "nothing failed and nothing was skipped"
    );
    assert_eq!(
        rig.api.requests().len(),
        0,
        "the metered account is untouched"
    );

    let dir = rig.stop();
    assert_eq!(plan_switch_count(&dir), 0);

    let rec = trace_records(&dir)
        .into_iter()
        .find(|r| r["identity"]["session"] == "S1" && r["identity"]["turn_index"] == 1)
        .expect("turn 1 record");
    assert_eq!(rec["result"]["failover_from"], serde_json::Value::Null);
    assert_eq!(rec["result"]["plan_switch"], serde_json::Value::Null);
}

/// The §6 scoping: a cooldown skip of a route that is *not* the family's
/// primary sets `failover_from` (the client's own choice was displaced)
/// but keeps `plan_switch` null (the family's account was not). Cleanest
/// construction: the family is on `overflow` and the client resolves
/// **directly to the overflow route** (`p-api/m1` — its own choice; the
/// guard displaces nothing), whose provider then cools down — the walk
/// skips it, nothing is left to attempt, and the failed request's record
/// still says what was skipped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_42_non_primary_abandon_is_failover_only() {
    let rig = rig("conf42-scope").await;
    // Spill the family for real: quota-exhausted 403 on the family's
    // primary — state -> overflow, and p-plan demoted for 60s.
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(quota_403(60));
    rig.api.queue(testkit::plan_ok("spilled"));
    // Then demote the overflow provider out-of-band (m2, out-of-family).
    rig.api.queue(quota_403(60));

    let (s1, _b, _h) = post_family(&rig.listen_addr, "S1", 1);
    assert_eq!(s1, 200);
    let (s2, _b2, _h2) = post_family(&rig.listen_addr, "S1", 2);
    assert_eq!(s2, 200, "the real spill: the overflow answered");
    let (s3, _b3, _h3) = post_m2(&rig.listen_addr, "p-api");
    assert_eq!(s3, 502, "the m2 walk exhausts its chain");

    // The client's own choice: `p-api/m1` explicitly. The guard passes it
    // through (state overflow + resolved overflow ⇒ no displacement), the
    // projection refuses the provider, the walk skips the route and the
    // single-candidate chain is exhausted.
    let body = r#"{"model":"p-api/m1","messages":[{"role":"user","content":"direct"}],"prompt_cache_key":"S3","stream":false}"#;
    let (s4, _b4, _h4) = testkit::http_post(
        &rig.listen_addr,
        "/v1/chat/completions",
        body.as_bytes(),
        &[],
    );
    assert_eq!(s4, 502, "no route is left to attempt");

    let dir = rig.stop();

    // Exactly the real spill's transition — the skips moved nothing.
    let evs = events(&dir);
    let switches: Vec<&(String, serde_json::Value)> =
        evs.iter().filter(|(k, _)| k == "plan.switched").collect();
    assert_eq!(switches.len(), 1);
    assert_eq!(switches[0].1["reason"], "primary_exhausted");

    let rec = trace_records(&dir)
        .into_iter()
        .find(|r| r["identity"]["session"] == "S3" && r["identity"]["turn_index"] == 1)
        .expect("the direct request's record");
    assert_eq!(
        rec["result"]["failover_from"], "p-api/m1",
        "the skip abandoned the overflow route — the client's own resolved route"
    );
    assert_eq!(
        rec["result"]["plan_switch"],
        serde_json::Value::Null,
        "the abandoned route was not the family's primary: the cooldown \
         displaced the client's choice, not the family's account (spec §6)"
    );
}

/// The streaming twin: the same produced case on the streaming path —
/// the pre-relay walk skips the cooling primary, the overflow's head is
/// relayed verbatim, and the relay's terminal record carries the same
/// `plan_switch` with no `plan.switched` behind it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_42_streaming_twin_produces_the_same_reason() {
    let rig = rig("conf42-stream").await;
    rig.plan.queue(CannedResponse::sse(sse_with_usage())); // turn 1, the ledger's source
    rig.plan.queue(quota_403(60)); // provider-wide demotion via m2
    rig.api.queue(CannedResponse::sse(sse_with_usage())); // the displaced turn 2

    let (s1, b1, _h1) = post_family_stream(&rig.listen_addr, "S1", 1);
    assert_eq!(s1, 200);
    assert_eq!(
        testkit::dechunk(&b1),
        sse_bytes(),
        "turn 1 relayed verbatim"
    );

    let (s2, _b2, _h2) = post_m2(&rig.listen_addr, "p-plan");
    assert_eq!(s2, 502);

    let (s3, b3, h3) = post_family_stream(&rig.listen_addr, "S1", 2);
    assert_eq!(s3, 200, "the overflow's head was relayed");
    assert_eq!(
        testkit::dechunk(&b3),
        sse_bytes(),
        "relay byte fidelity held"
    );
    assert_eq!(
        header(&h3, "content-type"),
        Some("text/event-stream"),
        "the stream head, not a buffered body"
    );
    assert_eq!(rig.plan.requests().len(), 2, "turn 1 and the m2 403 only");
    assert_eq!(rig.api.requests().len(), 1);

    let dir = rig.stop();
    assert_eq!(
        plan_switch_count(&dir),
        0,
        "no state move on the stream path"
    );

    let rec = trace_records(&dir)
        .into_iter()
        .find(|r| r["identity"]["session"] == "S1" && r["identity"]["turn_index"] == 2)
        .expect("turn 2 record");
    assert_eq!(rec["decision"]["provider"], "p-api");
    assert_eq!(rec["result"]["failover_from"], "p-plan/m1");
    let ps = &rec["result"]["plan_switch"];
    assert_eq!(ps["from"], "p-plan/m1");
    assert_eq!(ps["to"], "p-api/m1");
    assert_eq!(ps["reason"], "primary_cooling_down");
    assert_eq!(ps["probe"], false);
    assert_eq!(
        ps["reprefill_tokens"], 100,
        "the tapped usage fed the ledger"
    );
    assert_eq!(ps["switch_cost_nano"], 200_000);
}
