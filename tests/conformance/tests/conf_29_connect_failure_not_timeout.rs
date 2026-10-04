//! CONF-29 (DESIGN §12.8, §8's failure path, ADR-011 item 6 row 1):
//! **a connection failure is not a timeout.** A connect refusal is a
//! deterministic, sub-millisecond transport failure in which *no request
//! byte ever left the process* — nothing can have been billed — so it
//! classifies as `connect_failure` with `action = fallback_provider`
//! and walks the fallback chain. The earlier classifier flattened every
//! no-status failure to `timeout` (which never fails over), polluting
//! the "why did we switch" evidence and putting the recovery action on
//! the wrong class.
//!
//! Two claims, both with the upstream pointed at a closed local port:
//!
//!   (a) empty chain: `error.classified` carries `reason =
//!       "connect_failure"`, `action = "fallback_provider"`, `matched =
//!       "no-status"`, `status = null` — never the `timeout` / `abort`
//!       pair — and the client's terminal 502 names the same class;
//!   (b) a live fallback exists: the chain is actually walked (the
//!       fallback answers 200, the trace records `failover_from`), and
//!       the dead provider is never re-attempted.

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, CannedResponse};
use vadis_core::store::{Query, QueryRow, Store as _};

/// A port with no listener: bind then drop, so it is genuinely closed.
fn dead_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}

fn config_yaml(dead_port: u16, live_port: u16, listen_port: u16, with_fallback: bool) -> String {
    let fallback = if with_fallback {
        "fallback: [mock-b/glm]"
    } else {
        "fallback: []"
    };
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: mock-a
    urls:
      chat: http://127.0.0.1:{dead_port}/v1/chat/completions
    api_key_env: CONF29_DEAD_KEY
    wire_api: chat
    supports: [chat]
    models:
      - id: glm
        context: 128k
        price:
          input_miss: 0.001
          input_hit: 0.0001
          cache_write: 0.0
          output: 0.002
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "closed port (test fixture)"
  - name: mock-b
    urls:
      chat: http://127.0.0.1:{live_port}/v1/chat/completions
    api_key_env: CONF29_LIVE_KEY
    wire_api: chat
    supports: [chat]
    models:
      - id: glm
        context: 128k
        price:
          input_miss: 0.001
          input_hit: 0.0001
          cache_write: 0.0
          output: 0.002
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "mock upstream (no price; test fixture)"

aliases: {{}}
plugins: []
{fallback}
"#,
    )
}

const CLIENT_BODY: &str = r#"{"model":"mock-a/glm","messages":[{"role":"user","content":"hi"}],"prompt_cache_key":"conf29-sess"}"#;

const UPSTREAM_OK: &str = r#"{"id":"r1","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12,"prompt_tokens_details":{"cached_tokens":0}}}"#;

/// The `error.classified` event rows a request wrote, from the store.
fn classified_events(dir: &std::path::Path) -> Vec<serde_json::Value> {
    let store = vadis_store::SqliteStore::open(&dir.join("state/router.db")).unwrap();
    let QueryRow::Events(events) = store.query(Query::AllEvents).unwrap() else {
        panic!("events");
    };
    events
        .iter()
        .filter(|e| e.kind_raw == "error.classified")
        .map(|e| e.payload.clone())
        .collect()
}

/// Every JSONL line under the trace dir, parsed.
fn read_records(trace_dir: &std::path::Path) -> Vec<serde_json::Value> {
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

async fn start_router(
    config_path: &std::path::Path,
    listen_port: u16,
) -> tokio::task::JoinHandle<()> {
    std::env::set_var("CONF29_DEAD_KEY", "sk-conf29-dead");
    std::env::set_var("CONF29_LIVE_KEY", "sk-conf29-live");
    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move {
        let _ = vadis_cli::serve(&cfg).await;
    });
    testkit::wait_listening(&format!("127.0.0.1:{listen_port}"));
    serve_task
}

/// (a) Empty chain: the closed port's refusal classifies `connect_failure`
/// with the failover action — never `timeout`/`abort` — and the client's
/// terminal failure carries the same class.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_29_connect_failure_is_not_a_timeout() {
    let dir = testkit::tempdir("conf29-dead");
    let listen_port = testkit::free_port();
    let config_path = dir.join("config.yaml");
    // live_port is unused in this case; point it at another closed port.
    std::fs::write(
        &config_path,
        config_yaml(dead_port(), dead_port(), listen_port, false),
    )
    .unwrap();

    let serve_task = start_router(&config_path, listen_port).await;
    let (status, body, _h) = testkit::http_post(
        &format!("127.0.0.1:{listen_port}"),
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[],
    );
    serve_task.abort();
    let _ = serve_task.await;

    assert_eq!(status, 502, "connect refused with an empty chain is a 502");

    // The classification event: reason connect_failure, action matched.
    let events = classified_events(&dir);
    assert_eq!(
        events.len(),
        1,
        "exactly one error.classified for the request"
    );
    let ev = &events[0];
    assert_eq!(
        ev["reason"], "connect_failure",
        "a connection failure must not be classified timeout (body: {body:?})"
    );
    assert_eq!(
        ev["action"], "fallback_provider",
        "nothing was billed: the action walks the chain (ADR-011 item 6 row 1)"
    );
    assert_eq!(ev["matched"], "no-status");
    assert_eq!(
        ev["status"],
        serde_json::Value::Null,
        "no answer arrived, so no status exists"
    );

    // The client-facing failure names the same class (spec §8).
    let v: serde_json::Value = serde_json::from_slice(&body).expect("error body is json");
    assert_eq!(v["error"]["details"]["error_class"], "connect_failure");

    // And the trace line agrees (the analysis subject is the failure).
    let records = read_records(&dir.join("state/traces"));
    assert_eq!(records.len(), 1, "one trace line for the request");
    let errors = records[0]["errors"].as_array().expect("errors array");
    assert_eq!(errors[0]["details"]["error_class"], "connect_failure");
}

/// (b) A live fallback exists: the `fallback_provider` action is real —
/// the chain is walked, the fallback answers 200, the dead provider is
/// attempted exactly once, and the trace records the origin.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_29_connect_failure_walks_the_fallback_chain() {
    let dir = testkit::tempdir("conf29-failover");
    let live = testkit::MockUpstream::start().await.unwrap();
    live.queue(CannedResponse::json(200, "OK", UPSTREAM_OK.as_bytes()));

    let listen_port = testkit::free_port();
    let config_path = dir.join("config.yaml");
    std::fs::write(
        &config_path,
        config_yaml(dead_port(), live.addr.port(), listen_port, true),
    )
    .unwrap();

    let serve_task = start_router(&config_path, listen_port).await;
    let (status, body, _h) = testkit::http_post(
        &format!("127.0.0.1:{listen_port}"),
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[],
    );
    serve_task.abort();
    let _ = serve_task.await;

    assert_eq!(status, 200, "the fallback answered: {body:?}");

    // The dead provider was attempted exactly once; the live one once.
    let requests = live.requests();
    assert_eq!(
        requests.len(),
        1,
        "the fallback provider served the request"
    );

    let events = classified_events(&dir);
    assert_eq!(events.len(), 1, "one classification for the dead attempt");
    assert_eq!(events[0]["reason"], "connect_failure");
    assert_eq!(events[0]["action"], "fallback_provider");

    // The failover event ties the switch to the class, and the trace
    // names the origin route (spec §6 result.failover_from).
    let store = vadis_store::SqliteStore::open(&dir.join("state/router.db")).unwrap();
    let QueryRow::Events(rows) = store.query(Query::AllEvents).unwrap() else {
        panic!("events");
    };
    let failovers: Vec<_> = rows
        .iter()
        .filter(|e| e.kind_raw == "failover.triggered")
        .collect();
    assert_eq!(failovers.len(), 1, "one failover hop");
    assert_eq!(failovers[0].payload["reason"], "connect_failure");

    let records = read_records(&dir.join("state/traces"));
    assert_eq!(records.len(), 1, "one trace line for the request");
    assert_eq!(
        records[0]["result"]["failover_from"], "mock-a/glm",
        "the origin route is recorded"
    );
    assert_eq!(records[0]["decision"]["provider"], "mock-b");
}
