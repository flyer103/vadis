//! CONF-32 (spec §4.6 / ADR-014 items 4–5): **plan-first priority and the
//! priced spill.** While the family's account state is `primary` a family
//! request is served by the primary route's own `base_url`; when the primary
//! answers `403 quota_exhausted` the request spills to the overflow route,
//! the account move is recorded as one `plan.switched` event (durability
//! FULL), the trace carries `result.plan_switch` with the re-prefill cost
//! priced at the destination account's miss price, and `failover_from` names
//! the route the failure-class fact moved the request off.
//!
//! The 403 fixture's wording deliberately contains `insufficient_quota` —
//! one of the classifier's QUOTA_EXHAUSTED_PATTERNS — and carries
//! `Retry-After: 1` so the ADR-011 provider demotion that follows the 403
//! lasts one second instead of the 60s default (this case needs no second
//! request on the primary, so the short demotion only keeps the rig fast).

#![forbid(unsafe_code)]

use router_conformance::testkit::{self, CannedResponse};

fn config_yaml(plan_port: u16, api_port: u16, listen_port: u16) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: p-plan
    base_url: http://127.0.0.1:{plan_port}/v1
    api_key_env: CONF_PLAN_KEY
    wire_api: chat
    supports: [chat]
    account: coding_plan
    models:
      - id: m1
        context: 128k
        price:
          input_miss: 0.001
          input_hit: 0.0001
          cache_write: 0.0
          output: 0.002
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "mock upstream (no price; test fixture)"
  - name: p-api
    base_url: http://127.0.0.1:{api_port}/v1
    api_key_env: CONF_API_KEY
    wire_api: chat
    supports: [chat]
    account: api
    models:
      - id: m1
        context: 128k
        price:
          input_miss: 0.002
          input_hit: 0.0002
          cache_write: 0.0
          output: 0.004
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "mock upstream (no price; test fixture)"

aliases: {{}}
plugins: []
fallback: []

plan_policy:
  family: m1
  primary: p-plan/m1
  overflow: p-api/m1
  on_primary_exhausted: spill
  recover: probe
  cooldown: 0s
"#
    )
}

const OK_PLAN: &str = r#"{"id":"ok-plan","choices":[{"index":0,"message":{"role":"assistant","content":"from-plan"},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":5,"total_tokens":105,"prompt_tokens_details":{"cached_tokens":20}}}"#;
const OK_API: &str = r#"{"id":"ok-api","choices":[{"index":0,"message":{"role":"assistant","content":"from-api"},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":5,"total_tokens":105,"prompt_tokens_details":{"cached_tokens":20}}}"#;

/// 403 + `insufficient_quota` wording → classified `quota_exhausted` (the
/// only account-moving signal, ADR-014 item 2). `Retry-After: 1` shrinks
/// the ADR-011 provider demotion to one second.
fn forbidden() -> CannedResponse {
    CannedResponse::json(
        403,
        "Forbidden",
        br#"{"error":{"message":"You have exceeded your current quota","type":"insufficient_quota","code":"insufficient_quota"}}"#,
    )
    .with_header("retry-after", "1")
}

fn client_body(session: Option<&str>, turn: u32) -> String {
    let key = session
        .map(|s| format!(r#","prompt_cache_key":"{s}""#))
        .unwrap_or_default();
    format!(
        r#"{{"model":"p-plan/m1","messages":[{{"role":"user","content":"turn {turn}"}}]{key},"stream":false}}"#
    )
}

struct Rig {
    plan: testkit::MockUpstream,
    api: testkit::MockUpstream,
    listen_addr: String,
    dir: std::path::PathBuf,
    serve_task: tokio::task::JoinHandle<i32>,
}

async fn rig(tag: &str) -> Rig {
    let dir = testkit::tempdir(tag);
    let plan = testkit::MockUpstream::start().await.unwrap();
    let api = testkit::MockUpstream::start().await.unwrap();
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(
        &config_path,
        config_yaml(plan.addr.port(), api.addr.port(), listen_port),
    )
    .unwrap();
    std::env::set_var("CONF_PLAN_KEY", "sk-plan");
    std::env::set_var("CONF_API_KEY", "sk-api");
    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    Rig {
        plan,
        api,
        listen_addr,
        dir,
        serve_task,
    }
}

impl Rig {
    fn post(&self, session: Option<&str>, turn: u32) -> (u16, Vec<u8>, Vec<(String, String)>) {
        testkit::http_post(
            &self.listen_addr,
            "/v1/chat/completions",
            client_body(session, turn).as_bytes(),
            &[],
        )
    }

    fn stop(self) {
        self.serve_task.abort();
        drop(self.serve_task);
    }
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

/// Rule 1 (priority): the family's request goes to the primary route's own
/// `base_url` — the plan mock receives it, the metered mock receives nothing
/// — and the primary's bytes are relayed verbatim.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_32_primary_is_preferred_while_healthy() {
    let rig = rig("conf32-priority").await;
    rig.plan.queue(CannedResponse::json(200, "OK", OK_PLAN.as_bytes()));

    let (status, body, _h) = rig.post(None, 1);

    assert_eq!(status, 200, "the plan account answered");
    assert_eq!(body, OK_PLAN.as_bytes(), "relayed verbatim");
    assert_eq!(rig.plan.requests().len(), 1, "the primary was attempted");
    assert_eq!(rig.api.requests().len(), 0, "the metered account is untouched");
    let seen = &rig.plan.requests()[0];
    assert!(
        seen.path.starts_with("/v1/"),
        "the request went to the primary's base_url path, got path {}",
        seen.path
    );

    rig.stop();
}

/// Rule 2 (overflow): a primary `403 quota_exhausted` spills the request to
/// the overflow route; `plan.switched` is recorded once (durability FULL)
/// with the priced re-prefill; the trace shows the displacement
/// (`plan_switch` + `failover_from`) and the metered cost.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_32_quota_exhausted_spills_records_switch_and_prices_reprefill() {
    let rig = rig("conf32-spill").await;
    // Turn 1 succeeds on the plan (the ledger learns 100 prefix tokens);
    // turn 2's 403 is what prices the switch.
    rig.plan.queue(CannedResponse::json(200, "OK", OK_PLAN.as_bytes()));
    rig.plan.queue(forbidden());
    rig.api.queue(CannedResponse::json(200, "OK", OK_API.as_bytes()));

    let (s1, _b1, _h1) = rig.post(Some("S1"), 1);
    assert_eq!(s1, 200);
    let (s2, body2, headers2) = rig.post(Some("S1"), 2);
    assert_eq!(s2, 200, "the overflow account answered the spilled request");
    assert_eq!(body2, OK_API.as_bytes(), "the metered bytes relayed verbatim");
    assert_eq!(rig.plan.requests().len(), 2, "one attempt per turn on the primary");
    assert_eq!(rig.api.requests().len(), 1, "one overflow attempt");
    let ff = headers2
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("x-router-failover-from"))
        .map(|(_, v)| v.as_str());
    assert_eq!(ff, Some("p-plan/m1"), "the failure-class fact is recorded");

    let dir = rig.dir.clone();
    rig.stop();

    // One transition, one row: family, direction, reason, priced re-prefill.
    let evs = events(&dir);
    let switches: Vec<&(String, serde_json::Value)> = evs
        .iter()
        .filter(|(k, _)| k == "plan.switched")
        .collect();
    assert_eq!(switches.len(), 1, "exactly one plan.switched per transition");
    let (_k, p) = switches[0];
    assert_eq!(p["family"], "m1");
    assert_eq!(p["from_account"], "primary");
    assert_eq!(p["to_account"], "overflow");
    assert_eq!(p["reason"], "primary_exhausted");
    assert_eq!(p["probe"], false);
    assert_eq!(p["reprefill_tokens"], 100, "the session's prefix tokens");
    assert_eq!(
        p["switch_cost_nano"], 200_000,
        "100 tokens x p-api input_miss 0.002/1K = 0.0002 USD"
    );
    // Durability FULL (ADR-014 item 8): the wire word maps to the FULL tier.
    let kind = router_core::store::EventKind::from_str_lossy("plan.switched").unwrap();
    assert!(kind.is_full(), "plan.switched commits at durability FULL");

    // The trace: the spilled request's own record carries both markers and
    // the metered cost (the in-plan marginal price is 0, the spill is real).
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
    assert_eq!(ps["reason"], "primary_exhausted");
    assert_eq!(ps["probe"], false);
    assert_eq!(ps["reprefill_tokens"], 100);
    assert_eq!(ps["switch_cost_nano"], 200_000);
    assert_eq!(
        rec["cost"]["total"], 184_000,
        "metered: 80 miss x 2000 + 20 hit x 200 + 5 out x 4000 nano"
    );
}
