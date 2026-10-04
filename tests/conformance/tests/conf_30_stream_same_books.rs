//! CONF-30 (DESIGN §12.8, §12.10.5 note R3): **the streaming path keeps
//! the same books as the buffered path**. Measured motivation (the
//! smoke that motivated it): a real codex agent loop left the trace directory empty for its
//! streamed requests — session resolution, `session.bound`, prefix
//! blocks, `cost.computed` and the `DecisionRecord` itself existed only
//! on the buffered path, while codex/hermes traffic is permanently
//! streaming.
//!
//! Cases over a mock SSE upstream:
//! - (a) a streamed request with usage: one `DecisionRecord`, `session`
//!   resolved from `prompt_cache_key`, `session.bound` + `cost.computed`
//!   in the event log, and the event-chain shape the buffered path
//!   writes (modulo the stream-specific fields);
//! - (b) the same session's second streaming turn (full resend + one new
//!   message): `prefix.continuity == 1.0`; swapping the first message
//!   instead drops it below 1.0;
//! - (c) a stream without usage (`include_usage` not requested):
//!   `usage_missing: true` and nothing charged — cost absent, never
//!   invented;
//! - (d) a pre-relay connect failure classifies `connect_failure` (not
//!   `timeout`), same as the buffered path, and the trace line records
//!   it.

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, SseChunk};
use vadis_core::store::{Query, QueryRow, Store as _};
use serde_json::Value;

fn config_yaml(upstream_port: u16, listen_port: u16) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: mock
    urls:
      chat: http://127.0.0.1:{upstream_port}/v1/chat/completions
    api_key_env: CONF30_MOCK_KEY
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
"#,
    )
}

/// A chat SSE stream whose final data event carries usage (the carrier
/// exists only when the client asked — the body sets include_usage).
fn sse_with_usage() -> Vec<SseChunk> {
    vec![
        SseChunk::event(b"data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\n"),
        SseChunk::event(
            b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":5,\"total_tokens\":105,\"prompt_tokens_details\":{\"cached_tokens\":80}}}\n\n",
        ),
        SseChunk::event(b"data: [DONE]\n\n"),
    ]
}

fn turn_body(session: &str, first_message: &str, extra: Option<&str>) -> String {
    let mut messages = format!(r#"{{"role":"user","content":"{first_message}"}}"#);
    if let Some(e) = extra {
        messages.push_str(&format!(",{{\"role\":\"user\",\"content\":\"{e}\"}}"));
    }
    format!(
        r#"{{"model":"mock/glm","messages":[{messages}],"stream":true,"stream_options":{{"include_usage":true}},"prompt_cache_key":"{session}"}}"#
    )
}

fn start_router(config_path: &std::path::Path, listen_port: u16) -> tokio::task::JoinHandle<i32> {
    std::env::set_var("CONF30_MOCK_KEY", "sk-conf30");
    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&format!("127.0.0.1:{listen_port}"));
    serve_task
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

fn event_kinds(dir: &std::path::Path) -> Vec<(String, Value)> {
    let store = vadis_store::SqliteStore::open(&dir.join("state/router.db")).unwrap();
    let QueryRow::Events(events) = store.query(Query::AllEvents).unwrap() else {
        panic!("events");
    };
    events
        .iter()
        .map(|e| (e.kind_raw.clone(), e.payload.clone()))
        .collect()
}

/// (a) The normative case: a streamed request with usage keeps the same
/// books — one trace line with the session resolved, the buffered path's
/// event chain, cost computed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_30_a_stream_writes_the_same_books() {
    let dir = testkit::tempdir("conf30a");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    upstream.queue(testkit::CannedResponse::sse(sse_with_usage()));

    let listen_port = testkit::free_port();
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(upstream.addr.port(), listen_port)).unwrap();
    let serve_task = start_router(&config_path, listen_port);

    let (status, body, _h) = testkit::http_post(
        &format!("127.0.0.1:{listen_port}"),
        "/v1/chat/completions",
        turn_body("conf30-sess", "hello", None).as_bytes(),
        &[],
    );
    serve_task.abort();
    let _ = serve_task.await;

    assert_eq!(status, 200, "the streamed relay answered 200");
    let events = testkit::dechunk(&body);
    assert!(
        events.ends_with(b"data: [DONE]\n\n"),
        "the terminal marker was relayed"
    );

    // One trace line, session resolved, no blanks on the analysis keys.
    let records = read_records(&dir.join("state/traces"));
    assert_eq!(records.len(), 1, "exactly one trace line for the stream");
    let rec = &records[0];
    assert_eq!(rec["identity"]["session"], "conf30-sess");
    assert_eq!(rec["identity"]["turn_index"], 1);
    assert_eq!(rec["decision"]["provider"], "mock");
    assert_eq!(rec["decision"]["model"], "glm");
    assert_eq!(rec["decision"]["requested_model"], "mock/glm");
    assert_eq!(rec["result"]["status"], 200);
    assert_eq!(rec["result"]["upstream_status"], 200);

    // Usage arrived through the tap: measured, attributed, priced.
    assert_eq!(rec["usage_missing"], false);
    assert_eq!(rec["usage"]["input_total"], 100);
    assert_eq!(rec["usage"]["input_cached"], 80);
    assert!(
        rec["cost"]["total"].as_u64().unwrap_or(0) > 0,
        "usage priced"
    );
    // Turn 1 has no previous request: continuity is null, not invented.
    assert!(rec["prefix"]["continuity"].is_null());
    // The prefix blocks were extracted (one message block).
    let blocks = rec["prefix"]["blocks"].as_array().expect("blocks");
    assert_eq!(blocks.len(), 1);

    // The event chain matches the buffered path's vocabulary (modulo
    // stream-specific fields), in order.
    let kinds: Vec<String> = event_kinds(&dir)
        .into_iter()
        .map(|(k, _)| k)
        .filter(|k| k != "config.applied")
        .collect();
    let expected = [
        "request.received",
        "decision.made",
        "transform.applied",
        "session.bound",
        "upstream.submitted",
        "upstream.responded",
        "cost.computed",
    ];
    assert_eq!(kinds, expected.to_vec(), "same chain as the buffered path");
}

/// (b) The same session's second streaming turn: full resend + one new
/// message ⇒ continuity exactly 1.0 (the denominator is the previous
/// request's block count, spec §6); swapping the first message instead
/// drops it below 1.0.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_30_b_second_streaming_turn_continuity() {
    let dir = testkit::tempdir("conf30b");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    upstream.queue(testkit::CannedResponse::sse(sse_with_usage()));
    upstream.queue(testkit::CannedResponse::sse(sse_with_usage()));

    let listen_port = testkit::free_port();
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(upstream.addr.port(), listen_port)).unwrap();
    let serve_task = start_router(&config_path, listen_port);

    // Turn 1, then the stateless-client turn 2: same first message, one
    // message more (the captured codex/hermes shape).
    let (s1, _, _) = testkit::http_post(
        &format!("127.0.0.1:{listen_port}"),
        "/v1/chat/completions",
        turn_body("conf30-sess-b", "hello", None).as_bytes(),
        &[],
    );
    assert_eq!(s1, 200);
    let (s2, _, _) = testkit::http_post(
        &format!("127.0.0.1:{listen_port}"),
        "/v1/chat/completions",
        turn_body("conf30-sess-b", "hello", Some("and then")).as_bytes(),
        &[],
    );
    assert_eq!(s2, 200);
    serve_task.abort();
    let _ = serve_task.await;

    let records = read_records(&dir.join("state/traces"));
    assert_eq!(records.len(), 2, "one line per streamed request");
    let second = records
        .iter()
        .find(|r| r["identity"]["turn_index"] == 2)
        .expect("a turn-2 record");
    assert_eq!(second["identity"]["session"], "conf30-sess-b");
    let blocks2 = second["prefix"]["blocks"].as_array().expect("blocks");
    assert_eq!(blocks2.len(), 2, "turn 2 carries the resent + new message");
    // The first turn-1 block hash reappears as the first turn-2 block.
    let first = records
        .iter()
        .find(|r| r["identity"]["turn_index"] == 1)
        .expect("a turn-1 record");
    let blocks1 = first["prefix"]["blocks"].as_array().expect("blocks");
    assert_eq!(blocks1[0]["hash"], blocks2[0]["hash"]);
    let n = second["prefix"]["continuity"].as_f64().expect("measured");
    assert_eq!(n, 1.0, "a full resend preserves continuity exactly");

    // Control: a different session whose first message is swapped drops
    // below 1.0 — the metric measures the prefix, not the session id.
    let dir2 = testkit::tempdir("conf30b2");
    let upstream2 = testkit::MockUpstream::start().await.unwrap();
    upstream2.queue(testkit::CannedResponse::sse(sse_with_usage()));
    upstream2.queue(testkit::CannedResponse::sse(sse_with_usage()));
    let listen2 = testkit::free_port();
    let cfg2 = dir2.join("config.yaml");
    std::fs::write(&cfg2, config_yaml(upstream2.addr.port(), listen2)).unwrap();
    let serve2 = start_router(&cfg2, listen2);
    let (t1, _, _) = testkit::http_post(
        &format!("127.0.0.1:{listen2}"),
        "/v1/chat/completions",
        turn_body("conf30-sess-c", "hello", None).as_bytes(),
        &[],
    );
    assert_eq!(t1, 200);
    let (t2, _, _) = testkit::http_post(
        &format!("127.0.0.1:{listen2}"),
        "/v1/chat/completions",
        turn_body("conf30-sess-c", "swapped", Some("and then")).as_bytes(),
        &[],
    );
    assert_eq!(t2, 200);
    serve2.abort();
    let _ = serve2.await;

    let records2 = read_records(&dir2.join("state/traces"));
    assert_eq!(records2.len(), 2);
    let second2 = records2
        .iter()
        .find(|r| r["identity"]["turn_index"] == 2)
        .expect("turn-2 record");
    let n2 = second2["prefix"]["continuity"].as_f64().expect("measured");
    assert!(
        n2 < 1.0,
        "a first-message swap must drop continuity (got {n2})"
    );
}

/// (c) A stream whose usage never arrived (chat carrier not requested):
/// `usage_missing: true`, cost left uncomputed, nothing charged — the
/// absent number is not zero and not invented.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_30_c_no_usage_is_missing_not_invented() {
    let dir = testkit::tempdir("conf30c");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    // The stream still carries a usage event, but the client did not ask
    // for the chat carrier — the tap reports Missing either way; use a
    // plain stream without a usage event for the honest shape.
    upstream.queue(testkit::CannedResponse::sse(vec![
        SseChunk::event(b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n"),
        SseChunk::event(b"data: [DONE]\n\n"),
    ]));

    let listen_port = testkit::free_port();
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(upstream.addr.port(), listen_port)).unwrap();
    let serve_task = start_router(&config_path, listen_port);

    let body = r#"{"model":"mock/glm","messages":[{"role":"user","content":"hi"}],"stream":true,"prompt_cache_key":"conf30-sess-d"}"#;
    let (status, out, _h) = testkit::http_post(
        &format!("127.0.0.1:{listen_port}"),
        "/v1/chat/completions",
        body.as_bytes(),
        &[],
    );
    serve_task.abort();
    let _ = serve_task.await;

    assert_eq!(status, 200);
    assert!(testkit::dechunk(&out).ends_with(b"data: [DONE]\n\n"));

    let records = read_records(&dir.join("state/traces"));
    assert_eq!(records.len(), 1);
    let rec = &records[0];
    assert_eq!(rec["usage_missing"], true, "no carrier ⇒ missing, never 0");
    // Cost unknown, not zero-with-a-straight-face: the convention's mark
    // is usage_missing itself; the cost rows must not have been written.
    assert_eq!(rec["cost"]["total"], 0, "nothing charged for no usage");
    assert_eq!(rec["cost"]["quota_after"], Value::Null);
    let kinds: Vec<String> = event_kinds(&dir)
        .into_iter()
        .map(|(k, _)| k)
        .filter(|k| k == "cost.computed" || k == "quota.charged")
        .collect();
    assert!(
        kinds.is_empty(),
        "usage_missing charges nothing and writes no accounting rows (got {kinds:?})"
    );
}

/// (d) A pre-relay connect failure on the stream path: `connect_failure`
/// via `transport_cause`, never `timeout` — the same classification the
/// buffered path produces for the same failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_30_d_stream_connect_failure_classifies_connect() {
    let dir = testkit::tempdir("conf30d");
    let dead_port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let p = l.local_addr().unwrap().port();
        drop(l);
        p
    };
    let listen_port = testkit::free_port();
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(dead_port, listen_port)).unwrap();
    let serve_task = start_router(&config_path, listen_port);

    let (status, body, _h) = testkit::http_post(
        &format!("127.0.0.1:{listen_port}"),
        "/v1/chat/completions",
        turn_body("conf30-sess-e", "hi", None).as_bytes(),
        &[],
    );
    serve_task.abort();
    let _ = serve_task.await;

    assert_eq!(status, 502, "connect refused with an empty chain is a 502");
    let v: Value = serde_json::from_slice(&body).expect("error body is json");
    assert_eq!(
        v["error"]["details"]["error_class"], "connect_failure",
        "the stream path must classify a connect failure like the buffered path"
    );

    // The trace line agrees (the stream's terminal failure wrote it).
    let records = read_records(&dir.join("state/traces"));
    assert_eq!(records.len(), 1, "the stream failure left its line");
    let rec = &records[0];
    assert_eq!(rec["result"]["status"], 502);
    let errors = rec["errors"].as_array().expect("errors array");
    assert_eq!(errors[0]["details"]["error_class"], "connect_failure");
    assert_eq!(rec["usage_missing"], true);
}

/// (e) R6 after-first-byte: the stream dies mid-way. The relay truncates
/// (CONF-13b's client-side behavior) **and** the books say so — the
/// record carries the truncation in `errors[]`, usage never arrived so
/// nothing is charged, and no accounting rows were written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_30_e_truncated_stream_records_its_truncation() {
    let dir = testkit::tempdir("conf30e");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    upstream.queue(testkit::CannedResponse::sse(vec![
        SseChunk::event(b"data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\n"),
        // Abrupt close after one event: no usage, no [DONE].
        SseChunk::abort_after(50),
    ]));

    let listen_port = testkit::free_port();
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(upstream.addr.port(), listen_port)).unwrap();
    let serve_task = start_router(&config_path, listen_port);

    let (status, body, _h) = testkit::http_post(
        &format!("127.0.0.1:{listen_port}"),
        "/v1/chat/completions",
        turn_body("conf30-sess-f", "hi", None).as_bytes(),
        &[],
    );
    serve_task.abort();
    let _ = serve_task.await;

    assert_eq!(status, 200, "the head was already committed");
    assert!(!testkit::dechunk(&body).ends_with(b"data: [DONE]\n\n"));
    // No retry: one attempt only.
    assert_eq!(upstream.requests().len(), 1);

    // The books: one line, truncation in errors[], nothing charged.
    let records = read_records(&dir.join("state/traces"));
    assert_eq!(records.len(), 1, "the truncated stream still left its line");
    let rec = &records[0];
    assert_eq!(rec["result"]["status"], 200);
    assert_eq!(
        rec["usage_missing"], true,
        "an incomplete stream has no usage"
    );
    assert_eq!(rec["cost"]["total"], 0);
    let errors = rec["errors"].as_array().expect("errors array");
    assert!(
        errors
            .iter()
            .any(|e| e["details"]["error_class"] == "stream_truncated"),
        "the truncation is recorded, never presented as complete"
    );
    let kinds: Vec<String> = event_kinds(&dir)
        .into_iter()
        .map(|(k, _)| k)
        .filter(|k| k == "cost.computed" || k == "quota.charged")
        .collect();
    assert!(kinds.is_empty(), "a truncated stream charges nothing");
}
