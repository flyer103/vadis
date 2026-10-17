//! CONF-110 (spec §4.20.5; ADR-011 item 4's route-scoped half; ADR-024's skip
//! shape; CONF-21's projection == rebuild identity; DESIGN §12.8's row;
//! ADR-056): **a silent route loses its next selection — one short,
//! route-scoped, advisory row — and `QuotaExhausted` stays exactly as it is.**
//!
//! Landings asserted, not inferred: the `provider_cooldown` row read through
//! the store's own `Query::Cooldown` (both the route-scoped and the
//! provider-wide question), and the `error.classified` payload's `demotion`
//! object — the source `rebuild_cooldown` projects from (CONF-21).
//!
//! Limbs:
//! - (a) **one silent truncation leaves one route-scoped row**: on a two-route
//!   rig (`p1/m1` with `fallback: ["p2/m2"]`) whose `p1` mock stalls after the
//!   head, `Query::Cooldown { provider: "p1", model: Some("m1") }` answers a
//!   row with `scope: "route"`, `model: "m1"`, `reason: "stream_silence"` and a
//!   TTL of 60 s (asserted against the event's own `ts_us`, with a ±0.5 s
//!   tolerance for the write skew of one request), while the **provider-wide**
//!   query (`model: None`) answers **no row**; the event's `demotion` object
//!   carries `{scope, provider, model, until_us, ttl_s, reason}` — the shape
//!   `rebuild_cooldown` reads. **Red at the base** (`provider_cooldown` stays
//!   empty — the audited window's own sighting), and red for a provider-wide
//!   row, a different TTL, or an object the log cannot rebuild from.
//! - (b) **the next request walks away, and only its selection moves**: the
//!   immediately following request resolving to `p1/m1` reaches `p2`'s mock
//!   and never `p1`'s, is served `200`, carries
//!   `result.failover_from: "p1/m1"`, and there is **no** `failover.triggered`
//!   row (ADR-024's skip shape). Red for a row written but never read, and red
//!   for a displacement narrated without a move.
//! - (c) **`QuotaExhausted` is unchanged, side by side**: a streamed `403`
//!   with the quota wording demotes the **provider** — `scope: "provider"`,
//!   `model: ""`, the reason word the taxonomy already uses — and no
//!   route-scoped row is produced. Red for an implementation that widens
//!   `demotes_provider()` instead of adding a route arm.
//! - (d) **advisory means never a refusal**: on a single-route rig with a live
//!   `stream_silence` row, the next request is **served** (the route is
//!   attempted), with no `no_available_route` and no `skipped[]` entry naming
//!   the cooling route. Red for a binding reading of the row — the availability
//!   cliff spec §4.20.5 refuses.
//!
//! No network egress: loopback mocks only.
#![forbid(unsafe_code)]

use serde_json::Value;
use vadis_conformance::testkit::{self, SseChunk};
use vadis_core::store::{CooldownRow, Query, QueryRow, Store as _};

const IDLE: &str = "400ms";

/// One roster of `(provider, upstream port, native model id)` entries.
fn config_yaml(listen_port: u16, providers: &[(&str, u16, &str)], fallback: &[&str]) -> String {
    let mut roster = String::new();
    for (name, port, model) in providers {
        roster.push_str(&format!(
            r#"
  - name: {name}
    urls:
      chat: http://127.0.0.1:{port}/v1/chat/completions
    api_key_env: CONF110_KEY
    wire_api: chat
    supports: [chat]
    models:
      - id: {model}
        context: 128k
        price:
          input_miss: 0.001
          input_hit: 0.0001
          cache_write: 0.0
          output: 0.002
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "mock upstream (no price; test fixture)"
"#
        ));
    }
    let chain = if fallback.is_empty() {
        "[]".to_string()
    } else {
        format!(
            "[{}]",
            fallback
                .iter()
                .map(|r| format!("\"{r}\""))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    format!(
        r#"
server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 3s, request_timeout: 8s, stream_idle_timeout: {IDLE} }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:{roster}
aliases: {{}}
plugins: []
fallback: {chain}
"#
    )
}

fn stream_body(model: &str) -> String {
    format!(
        r#"{{"model":"{model}","messages":[{{"role":"user","content":"hi"}}],"stream":true,"prompt_cache_key":"conf110-sess"}}"#
    )
}

fn read_records(trace_dir: &std::path::Path) -> Vec<Value> {
    let mut records = Vec::new();
    for entry in std::fs::read_dir(trace_dir).expect("trace dir exists") {
        let entry = entry.unwrap();
        if entry.path().extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        for line in std::fs::read_to_string(entry.path()).unwrap().lines() {
            records.push(serde_json::from_str(line).expect("record json"));
        }
    }
    records
}

/// Every stored event as (kind_raw, payload, ts_us), read after `serve` stopped.
fn read_events(dir: &std::path::Path) -> Vec<(String, Value, i64)> {
    let store = vadis_store::SqliteStore::open(&dir.join("state/vadis.db")).unwrap();
    let QueryRow::Events(rows) = store.query(Query::AllEvents).unwrap() else {
        panic!("events query");
    };
    rows.into_iter()
        .map(|r| (r.kind_raw, r.payload, r.ts_us))
        .collect()
}

/// The cooldown row a **route** sees, if any.
fn route_cooldown(dir: &std::path::Path, provider: &str, model: &str) -> Option<CooldownRow> {
    let store = vadis_store::SqliteStore::open(&dir.join("state/vadis.db")).unwrap();
    match store
        .query(Query::Cooldown {
            provider,
            model: Some(model),
        })
        .unwrap()
    {
        QueryRow::Cooldown(row) => row,
        other => panic!("cooldown query: {other:?}"),
    }
}

/// The cooldown row a **provider-wide** read sees, if any.
fn provider_cooldown(dir: &std::path::Path, provider: &str) -> Option<CooldownRow> {
    let store = vadis_store::SqliteStore::open(&dir.join("state/vadis.db")).unwrap();
    match store
        .query(Query::Cooldown {
            provider,
            model: None,
        })
        .unwrap()
    {
        QueryRow::Cooldown(row) => row,
        other => panic!("cooldown query: {other:?}"),
    }
}

/// A head + one event, then silence with the socket held open.
fn silence() -> testkit::CannedResponse {
    testkit::CannedResponse::sse(vec![
        SseChunk::event(b"data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\n"),
        SseChunk::event_after(b"data: never\n\n", 60_000),
    ])
}

/// A clean, byte-complete stream.
fn clean() -> testkit::CannedResponse {
    testkit::CannedResponse::sse(vec![
        SseChunk::event(b"data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n"),
        SseChunk::event(b"data: [DONE]\n\n"),
    ])
}

async fn start_vadis(
    config_path: &std::path::Path,
    listen_port: u16,
) -> tokio::task::JoinHandle<()> {
    std::env::set_var("CONF110_KEY", "sk-conf110");
    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move {
        let _ = vadis_cli::serve(&cfg).await;
    });
    testkit::wait_listening(&format!("127.0.0.1:{listen_port}"));
    serve_task
}

/// (a)+(b) One silent truncation leaves one route-scoped row, and the next
/// request walks away.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "CONF-110: depends on R74-1"]
async fn conf_110_a_b_one_silence_steers_the_next_selection() {
    let dir = testkit::tempdir("conf110ab");
    let p1 = testkit::MockUpstream::start().await.unwrap();
    let p2 = testkit::MockUpstream::start().await.unwrap();
    p1.queue(silence());
    p2.queue(clean());

    let listen_port = testkit::free_port();
    let addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(
        &config_path,
        config_yaml(
            listen_port,
            &[("p1", p1.addr.port(), "m1"), ("p2", p2.addr.port(), "m2")],
            &["p2/m2"],
        ),
    )
    .unwrap();
    let serve_task = start_vadis(&config_path, listen_port).await;

    // #1: truncates on p1/m1 and orders the row.
    let (s1, _b1, _h1) = testkit::http_post(
        &addr,
        "/v1/chat/completions",
        stream_body("p1/m1").as_bytes(),
        &[],
    );
    assert_eq!(s1, 200, "the head was committed");
    assert_eq!(p1.requests().len(), 1, "one attempt on p1");

    // #2: the same resolution, one moment later.
    let (s2, _b2, _h2) = testkit::http_post(
        &addr,
        "/v1/chat/completions",
        stream_body("p1/m1").as_bytes(),
        &[],
    );
    serve_task.abort();
    let _ = serve_task.await;

    assert_eq!(s2, 200, "the next request is served, not refused");
    assert_eq!(
        p1.requests().len(),
        1,
        "the cooling route is not attempted again inside the window"
    );
    assert_eq!(p2.requests().len(), 1, "the walk moved to p2");

    // (a) the row and the payload it was projected from.
    let row = route_cooldown(&dir, "p1", "m1").expect("a route-scoped cooldown row");
    assert_eq!(row.scope, "route", "the scope word");
    assert_eq!(row.provider, "p1");
    assert_eq!(row.model, "m1", "the native id, never an alias");
    assert_eq!(row.reason, "stream_silence", "the advisory reason word");
    assert!(
        provider_cooldown(&dir, "p1").is_none(),
        "a silence row is NOT provider-wide (that is QuotaExhausted's shape)"
    );

    let events = read_events(&dir);
    let classified = events
        .iter()
        .find(|(k, p, _)| k == "error.classified" && p["demotion"].is_object())
        .expect("an error.classified with a demotion");
    let d = &classified.1["demotion"];
    assert_eq!(d["scope"], "route");
    assert_eq!(d["provider"], "p1");
    assert_eq!(d["model"], "m1");
    assert_eq!(d["reason"], "stream_silence");
    assert_eq!(d["ttl_s"], 60, "the TTL is the 60 s constant");
    let until_us = d["until_us"].as_i64().expect("until_us is an integer");
    assert_eq!(
        until_us, row.until_us,
        "the payload and the projection carry ONE value (CONF-21)"
    );
    let ttl_us = until_us - classified.2;
    assert!(
        (59_500_000..=60_500_000).contains(&ttl_us),
        "the row's own bound is 60 s from the classification event: {ttl_us} µs"
    );

    // (b) the displacement is narrated, the move is not.
    let records = read_records(&dir.join("state/traces"));
    assert_eq!(records.len(), 2, "one record per request");
    assert_eq!(
        records[1]["result"]["failover_from"], "p1/m1",
        "the pre-attempt skip names the abandoned route (ADR-024)"
    );
    assert!(
        !events.iter().any(|(k, _, _)| k == "failover.triggered"),
        "a pre-attempt skip writes no failover.triggered row"
    );
}

/// (c) `QuotaExhausted` keeps its provider-wide shape, side by side.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "CONF-110: depends on R74-1"]
async fn conf_110_c_quota_exhaustion_stays_provider_wide() {
    let dir = testkit::tempdir("conf110c");
    let p1 = testkit::MockUpstream::start().await.unwrap();
    let p2 = testkit::MockUpstream::start().await.unwrap();
    p1.queue(testkit::CannedResponse::json(
        403,
        "Forbidden",
        b"{\"error\":{\"type\":\"insufficient_quota\",\"message\":\"insufficient_quota: your account is out of credits\"}}",
    ));
    p2.queue(clean());

    let listen_port = testkit::free_port();
    let addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(
        &config_path,
        config_yaml(
            listen_port,
            &[("p1", p1.addr.port(), "m1"), ("p2", p2.addr.port(), "m2")],
            &["p2/m2"],
        ),
    )
    .unwrap();
    let serve_task = start_vadis(&config_path, listen_port).await;

    let (status, _b, _h) = testkit::http_post(
        &addr,
        "/v1/chat/completions",
        stream_body("p1/m1").as_bytes(),
        &[],
    );
    serve_task.abort();
    let _ = serve_task.await;

    assert_eq!(status, 200, "the chain served the request from p2");

    let events = read_events(&dir);
    let classified = events
        .iter()
        .find(|(k, p, _)| k == "error.classified" && p["demotion"].is_object())
        .expect("the quota classification carries a demotion");
    assert_eq!(
        classified.1["demotion"]["scope"], "provider",
        "billing exhaustion is a fact about the ACCOUNT"
    );
    assert_eq!(
        classified.1["demotion"]["model"], "",
        "the provider-wide row's model member is the empty string"
    );

    let row = provider_cooldown(&dir, "p1").expect("a provider-wide row");
    assert_eq!(row.scope, "provider");
    assert_eq!(row.model, "");
    assert_ne!(
        row.reason, "stream_silence",
        "the silence reason word belongs to the route-scoped arm only"
    );
    let route_row =
        route_cooldown(&dir, "p1", "m1").expect("the provider-wide row shadows the route");
    assert_eq!(
        route_row.scope, "provider",
        "no route-scoped row was produced by the quota verdict"
    );
}

/// (d) The row is advisory: a lone cooling route is still attempted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "CONF-110: depends on R74-1"]
async fn conf_110_d_advisory_row_never_refuses() {
    let dir = testkit::tempdir("conf110d");
    let p1 = testkit::MockUpstream::start().await.unwrap();
    p1.queue(silence());
    p1.queue(clean());

    let listen_port = testkit::free_port();
    let addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(
        &config_path,
        config_yaml(listen_port, &[("p1", p1.addr.port(), "m1")], &[]),
    )
    .unwrap();
    let serve_task = start_vadis(&config_path, listen_port).await;

    let (s1, _b1, _h1) = testkit::http_post(
        &addr,
        "/v1/chat/completions",
        stream_body("p1/m1").as_bytes(),
        &[],
    );
    assert_eq!(s1, 200);
    // The row is live now; with no alternative, the route must still be tried.
    let (s2, body2, _h2) = testkit::http_post(
        &addr,
        "/v1/chat/completions",
        stream_body("p1/m1").as_bytes(),
        &[],
    );
    serve_task.abort();
    let _ = serve_task.await;

    assert_eq!(
        s2, 200,
        "a stream_silence row may not refuse the last eligible route: {body2:?}"
    );
    assert_eq!(
        p1.requests().len(),
        2,
        "the cooling route was attempted again — advisory, not binding"
    );

    let records = read_records(&dir.join("state/traces"));
    assert_eq!(records.len(), 2);
    let errors = records[1]["errors"].as_array().expect("errors array");
    assert!(
        errors
            .iter()
            .all(|e| e["details"]["stage"] != "no_available_route"),
        "no refusal shape on the advisory path: {errors:?}"
    );
    assert!(
        route_cooldown(&dir, "p1", "m1").is_some(),
        "…and the row existed while it was attempted"
    );
}
