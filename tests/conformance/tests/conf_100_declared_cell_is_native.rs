//! CONF-100 (ADR-051 §2.1, spec §2 / §4 / §8, DESIGN §12.10.9): **a declared
//! cell is a native cell**, and an undeclared one is the `400`.
//!
//! The selection rule is binary over the entry's **declared cell set**
//! (`supports`); the entry's own `wire_api` names the diagonal and gates
//! nothing. The case is the rule's two arms, on the real `serve` assembly:
//!
//! * **Arm (a) — a cell the entry declares is served natively.** The roster
//!   holds `three-line`: `supports: [chat, responses, anthropic]`, three
//!   `urls` (one per cell, each a distinct path on the same mock), and
//!   `wire_api: responses`. A **chat** request naming `three-line/m` must be
//!   served `200` with the client's own chat bytes modulo spec §2's two
//!   permitted mutations, **at the `urls.chat` path** — the mock's recorded
//!   `path` is the evidence, not a header or a log line — and the trace must
//!   say `protocol.protocol_in == protocol.protocol_out == "chat"` with
//!   `translated == false`. Red at the base: the same request is a `501`
//!   (`forward.rs:902-911`, `stream_forward.rs:440-453`), because the entry's
//!   `wire_api` is not the inbound protocol.
//! * **Arm (b) — a cell the entry does not declare is the `400`.** A provider
//!   `chat-only` (`supports: [chat]`, `wire_api: chat`) named by a
//!   **responses** request is answered `400 capability_unsupported` with
//!   spec §8's body — the row that is unchanged by ADR-051, asserted so
//!   arm (a) cannot pass by making every cell reachable.
//!
//! Rig shape: two provider entries, one mock upstream each (the mock accepts
//! any path and records it, so one mock carries all three cells of an entry);
//! both keys present in the environment; `fallback: []`.
//!
//! Depends on: the eligibility discriminant moving from `wire_api == proto_in`
//! to `supports ∋ proto_in` on both forwarding paths, the two resolved-route
//! `501` branches being deleted, and the `url_for` argument becoming the
//! inbound protocol.

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, CannedResponse};

/// A 200 chat completion body whose `usage` the accounting path can price.
const CHAT_OK: &str = r#"{"id":"ok","choices":[{"index":0,"message":{"role":"assistant","content":"served"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}"#;

/// The client's chat bytes: deliberately odd (unicode, a float, a nested
/// object, a vadis-owned key) so that a reserialization would be visible.
const CLIENT_BODY: &str = r#"{"model":"three-line/m","messages":[{"role":"user","content":"héllo 😀 {b}"}],"temperature":1e-9,"vadis_meta":{"echo":true},"stream":false}"#;

/// The same bytes after exactly the two permitted mutations: `vadis_meta`
/// removed, the top-level `model` value replaced by the native id.
const EXPECTED_UPSTREAM_BODY: &str = r#"{"model":"m","messages":[{"role":"user","content":"héllo 😀 {b}"}],"temperature":1e-9,"stream":false}"#;

async fn rig(tag: &str) -> (testkit::MockUpstream, String, std::path::PathBuf) {
    let mock = testkit::MockUpstream::start().await.unwrap();
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let dir = testkit::tempdir(tag);
    let mock_port = mock.addr.port();

    std::env::set_var("CONF100_THREE_LINE_KEY", "sk-three-line");
    std::env::set_var("CONF100_CHAT_ONLY_KEY", "sk-chat-only");

    let config = format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: three-line
    urls:
      chat:      http://127.0.0.1:{mock_port}/v1/chat/completions
      responses: http://127.0.0.1:{mock_port}/v1/responses
      anthropic: http://127.0.0.1:{mock_port}/anthropic/v1/messages
    api_key_env: CONF100_THREE_LINE_KEY
    wire_api: responses
    supports: [chat, responses, anthropic]
    account: api
    models:
      - id: m
        context: 128k
        price:
          input_miss: 0.001
          input_hit: 0.0001
          cache_write: 0.0
          output: 0.002
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "mock upstream (no price; test fixture)"
  - name: chat-only
    urls:
      chat: http://127.0.0.1:{mock_port}/v1/chat/completions
    api_key_env: CONF100_CHAT_ONLY_KEY
    wire_api: chat
    supports: [chat]
    account: api
    models:
      - id: m
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
    );
    std::fs::write(dir.join("config.yaml"), config).unwrap();
    (mock, listen_addr, dir)
}

async fn serve(dir: &std::path::Path, listen_addr: &str) -> tokio::task::JoinHandle<i32> {
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let addr = listen_addr.to_string();
    let task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&addr);
    task
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "CONF-100: depends on R69-1"]
async fn conf_100_declared_cell_is_native() {
    let (mock, listen_addr, dir) = rig("conf100").await;
    mock.queue(CannedResponse::json(200, "OK", CHAT_OK.as_bytes()));
    let task = serve(&dir, &listen_addr).await;

    // Arm (a): chat in, and `three-line` declares chat — so the entry serves
    // it natively, at the cell's own URL.
    let (status, body, _h) = testkit::http_post(
        &listen_addr,
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[],
    );
    assert_eq!(status, 200, "a declared cell is a native cell");
    assert!(!body.is_empty());

    let seen = mock.requests();
    assert_eq!(seen.len(), 1, "one upstream request");
    assert_eq!(
        seen[0].path, "/v1/chat/completions",
        "the request went out on the inbound protocol's own URL"
    );
    assert_eq!(
        String::from_utf8_lossy(&seen[0].body),
        EXPECTED_UPSTREAM_BODY,
        "the client's bytes, modulo the two permitted mutations"
    );

    // Arm (b): responses in, and `chat-only` does not declare it — the
    // undeclared cell, for the route the client named, is a 400.
    let (status, body, _h) = testkit::http_post(
        &listen_addr,
        "/v1/responses",
        br#"{"model":"chat-only/m","input":"hi","stream":false}"#,
        &[],
    );
    assert_eq!(status, 400, "an undeclared cell is not a cell");
    let v: serde_json::Value = serde_json::from_slice(&body).expect("json body");
    assert_eq!(v["error"]["type"], "capability_unsupported");
    assert_eq!(
        mock.requests().len(),
        1,
        "the refused request reached no upstream"
    );

    task.abort();
    let records = trace_records(&dir);
    let served = records
        .iter()
        .find(|r| r["result"]["status"] == 200)
        .expect("the served chat request's record");
    assert_eq!(served["protocol"]["protocol_in"], "chat");
    assert_eq!(
        served["protocol"]["protocol_out"], "chat",
        "the outbound wire is the inbound protocol, by construction"
    );
    assert_eq!(served["protocol"]["translated"], false);
}
