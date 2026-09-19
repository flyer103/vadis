//! CONF-24 (§12.8): **trace ↔ event join** — every `DecisionRecord` carries
//! an `event_id` that exists in `events` with `kind = request.received`
//! and the same `request_id`; the request's accounting rows carry a
//! `trace_ref` that resolves to that record's own line.
//!
//! Drives the real `serve` assembly over loopback HTTP (one request with
//! usage, one without — `usage_missing` writes no accounting rows but the
//! join keys must still be sound), then checks the join in both
//! directions against the SQLite store and the hourly JSONL trace.

#![forbid(unsafe_code)]

use router_conformance::testkit::{self, CannedResponse};

fn config_yaml(upstream_port: u16, listen_port: u16) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: mock
    base_url: http://127.0.0.1:{upstream_port}/v1
    api_key_env: CONF24_MOCK_KEY
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
fallback: []
"#
    )
}

const BODY_WITH_USAGE: &str = r#"{"model":"mock/glm","messages":[{"role":"user","content":"hi"}],"prompt_cache_key":"sess-conf24-a"}"#;
// No `usage` member in the response: usage_missing, no accounting rows.
const BODY_NO_USAGE: &str = r#"{"model":"mock/glm","messages":[{"role":"user","content":"hi again"}],"prompt_cache_key":"sess-conf24-b"}"#;

fn resp_with_usage() -> CannedResponse {
    CannedResponse::json(
        200,
        "OK",
        br#"{"id":"r1","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":10,"total_tokens":110,"prompt_tokens_details":{"cached_tokens":50}}}"#.as_slice(),
    )
}

fn resp_no_usage() -> CannedResponse {
    CannedResponse::json(
        200,
        "OK",
        br#"{"id":"r2","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]}"#.as_slice(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_24_trace_event_join() {
    let dir = testkit::tempdir("conf24");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    upstream.queue(resp_with_usage());
    upstream.queue(resp_no_usage());

    let listen_port = testkit::free_port();
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(upstream.addr.port(), listen_port)).unwrap();

    std::env::set_var("CONF24_MOCK_KEY", "sk-conf24");
    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
    testkit::wait_listening(&format!("127.0.0.1:{listen_port}"));

    let (s1, _, _) = testkit::http_post(
        &format!("127.0.0.1:{listen_port}"),
        "/v1/chat/completions",
        BODY_WITH_USAGE.as_bytes(),
        &[],
    );
    assert_eq!(s1, 200);
    let (s2, _, _) = testkit::http_post(
        &format!("127.0.0.1:{listen_port}"),
        "/v1/chat/completions",
        BODY_NO_USAGE.as_bytes(),
        &[],
    );
    assert_eq!(s2, 200);

    serve_task.abort();
    let _ = serve_task.await;

    // Read the trace: every line a DecisionRecord with the join keys.
    let trace_dir = dir.join("state/traces");
    let mut records: Vec<(String, u64, serde_json::Value)> = Vec::new();
    for entry in std::fs::read_dir(&trace_dir).expect("trace dir") {
        let entry = entry.unwrap();
        if entry.path().extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let file = entry.file_name().to_string_lossy().into_owned();
        for (i, line) in std::fs::read_to_string(entry.path())
            .unwrap()
            .lines()
            .enumerate()
        {
            let v: serde_json::Value = serde_json::from_str(line).expect("record json");
            records.push((format!("{file}:{}", i + 1), (i + 1) as u64, v));
        }
    }
    assert_eq!(records.len(), 2, "one record per request");
    let mut by_request: std::collections::HashMap<String, (String, serde_json::Value)> =
        std::collections::HashMap::new();
    for (ptr, _line, v) in &records {
        let req = v["identity"]["request_id"]
            .as_str()
            .expect("request_id")
            .to_string();
        by_request.insert(req, (ptr.clone(), v.clone()));
    }

    let store = router_store::SqliteStore::open(&dir.join("state/router.db")).unwrap();
    use router_core::store::{Query, QueryRow, Store as _};
    let QueryRow::Events(events) = store.query(Query::AllEvents).unwrap() else {
        panic!("events");
    };

    // Direction 1 (trace → events): every record's event_id resolves to a
    // request.received row with the same request_id.
    for (req, (ptr, rec)) in &by_request {
        let event_id = rec["identity"]["event_id"].as_i64().expect("event_id");
        let row = events
            .iter()
            .find(|e| e.event_id.0 == event_id)
            .unwrap_or_else(|| panic!("event_id {event_id} of {req} resolves in events"));
        assert_eq!(
            row.kind_raw, "request.received",
            "the join anchor is the request.received row"
        );
        assert_eq!(
            row.request_id.as_deref(),
            Some(req.as_str()),
            "same request_id on both sides"
        );
        let _ = ptr; // used below
    }

    // Direction 2 (events → trace): the accounting rows of a request with
    // usage carry a trace_ref that resolves to that record's own line.
    for e in &events {
        if !matches!(e.kind_raw.as_str(), "cost.computed" | "quota.charged") {
            continue;
        }
        let Some(ptr) = &e.trace_ref else {
            panic!("{} must carry a trace_ref (usage arrived)", e.kind_raw);
        };
        let req = e
            .request_id
            .as_deref()
            .expect("accounting row's request_id");
        let (own_ptr, _rec) = by_request
            .get(req)
            .unwrap_or_else(|| panic!("request {req} has a trace record"));
        assert_eq!(
            ptr, own_ptr,
            "trace_ref must resolve to the request's own record line"
        );
    }

    // And the missing-usage request wrote no accounting rows at all —
    // but its record still joined on request.received above.
    let accounting_rows: Vec<_> = events
        .iter()
        .filter(|e| matches!(e.kind_raw.as_str(), "cost.computed" | "quota.charged"))
        .collect();
    // One request had usage: at least its cost.computed exists, and every
    // accounting row carries a non-null trace_ref (checked above).
    assert!(
        accounting_rows
            .iter()
            .any(|e| e.kind_raw == "cost.computed"),
        "the usage request's cost.computed row exists"
    );
    let no_usage_req = records
        .iter()
        .find(|(_, _, v)| v["usage_missing"].as_bool() == Some(true))
        .map(|(_, _, v)| v["identity"]["request_id"].as_str().unwrap().to_string())
        .expect("one usage_missing record");
    assert!(
        accounting_rows
            .iter()
            .all(|e| e.request_id.as_deref() != Some(no_usage_req.as_str())),
        "usage_missing charges nothing and writes no accounting rows"
    );
}
