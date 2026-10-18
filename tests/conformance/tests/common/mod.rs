//! The step-ledger rig (CONF-111…117, spec §9.4, ADR-057 §4), shared by
//! the seven cases. It lives here — `tests/common/`, a plain module the
//! case files include with `mod common;` — because it drives the real
//! `vadis_cli::serve` assembly, and `vadis-cli` is a dev-dependency of
//! this crate: only the mock upstream and the small blocking helpers
//! live in the lib target's `testkit`.
//!
//! The trace is GENERATED, never hand-written — a fixture asserts its
//! author's idea of the writer; this asserts the writer (R74-0e's
//! lesson, ADR-057 §4).
//!
//! The canned session (session key `STEPS_SESSION`), in order:
//!   1. [A]              on the plan route (account: coding_plan)
//!   2. [A,B]            append
//!   3. [A,B,C]          append
//!   4. [A,B,C]          byte-identical resend — repeat
//!   5. [A,B]            trimmed resend — shorten
//!   6. [A,X,C]          mid-prefix change — rewrite (at #1)
//!   7. [A,X,C,D]        append, answered WITHOUT a usage block
//!   8. [A,X,C,D] stream (only with `truncated_tail`): head, one
//!      event, then silence past the 400ms idle bound — trunc.
//! Answers 1-6 carry usage {100 prompt, 20 cached, 5 completion}.

#![forbid(unsafe_code)]

use serde_json::Value;
use vadis_conformance::testkit::{self, CannedResponse, SseChunk};

/// The canned session key (the client's `prompt_cache_key`).
pub const STEPS_SESSION: &str = "conf-steps-sess";

/// One canned 200 with the fixed usage the ledger reads back:
/// 100 prompt tokens (20 cached) + 5 completion.
pub fn steps_ok_with_usage() -> CannedResponse {
    CannedResponse::json(
        200,
        "OK",
        br#"{"id":"ok","choices":[{"index":0,"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":5,"total_tokens":105,"prompt_tokens_details":{"cached_tokens":20}}}"#,
    )
}

/// One canned 200 with NO usage member — the usage-less answer.
pub fn steps_ok_without_usage() -> CannedResponse {
    CannedResponse::json(
        200,
        "OK",
        br#"{"id":"ok","choices":[{"index":0,"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}]}"#,
    )
}

/// The rig's config: one metered provider (`mock`, model `glm`) and one
/// `account: coding_plan` provider (`p-plan`, model `m1`), both wired
/// to the SAME mock upstream; sticky off, so the rig's model choices —
/// not a session binding — decide who serves each request.
fn steps_config_yaml(mock_port: u16, listen_port: u16) -> String {
    format!(
        r#"
server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 3s, request_timeout: 8s, stream_idle_timeout: 400ms }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: false, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: p-plan
    urls:
      chat: http://127.0.0.1:{mock_port}/v1/chat/completions
    api_key_env: CONF_STEPS_PLAN_KEY
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
  - name: mock
    urls:
      chat: http://127.0.0.1:{mock_port}/v1/chat/completions
    api_key_env: CONF_STEPS_MOCK_KEY
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

/// The rig's message literals — one block each, byte-identical wherever
/// the same letter is used, so the block-hash relation is exactly the
/// script's diffs.
const STEPS_A: &str = r#"{"role":"user","content":"alpha one"}"#;
const STEPS_B: &str = r#"{"role":"assistant","content":"beta two"}"#;
const STEPS_C: &str = r#"{"role":"user","content":"gamma three"}"#;
const STEPS_X: &str = r#"{"role":"assistant","content":"beta TWO rewritten"}"#;
const STEPS_D: &str = r#"{"role":"user","content":"delta four"}"#;

fn steps_body(model: &str, messages: &str, stream: bool) -> String {
    format!(
        r#"{{"model":"{model}","messages":[{messages}],"stream":{stream},"prompt_cache_key":"{STEPS_SESSION}"}}"#
    )
}

/// Drive the canned session through a real `serve`, then stop it and
/// return `(config path, the session's trace records read back from the
/// files, in file order)`. With `truncated_tail` the eighth request (a
/// streamed silent-tail truncation) is appended.
pub async fn run_steps_session(
    tag: &str,
    truncated_tail: bool,
) -> (std::path::PathBuf, Vec<Value>) {
    let dir = testkit::tempdir(tag);
    let mock = testkit::MockUpstream::start().await.unwrap();
    for _ in 0..6 {
        mock.queue(steps_ok_with_usage());
    }
    mock.queue(steps_ok_without_usage());
    if truncated_tail {
        mock.queue(CannedResponse::sse(vec![
            SseChunk::event(b"data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\n"),
            // Silence past the 400ms idle bound: a declared truncation,
            // 2xx with errors[] carrying stream_truncated
            // (CONF-108/109's rig shape).
            SseChunk::event_after(b"data: never\n\n", 60_000),
        ]));
    }
    let listen_port = testkit::free_port();
    let addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(
        &config_path,
        steps_config_yaml(mock.addr.port(), listen_port),
    )
    .unwrap();
    std::env::set_var("CONF_STEPS_PLAN_KEY", "sk-steps-plan");
    std::env::set_var("CONF_STEPS_MOCK_KEY", "sk-steps-mock");
    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&addr);

    let posts = [
        ("p-plan/m1", format!("{STEPS_A}"), false),
        ("mock/glm", format!("{STEPS_A},{STEPS_B}"), false),
        ("mock/glm", format!("{STEPS_A},{STEPS_B},{STEPS_C}"), false),
        // The byte-identical resend of request 3 — repeat.
        ("mock/glm", format!("{STEPS_A},{STEPS_B},{STEPS_C}"), false),
        ("mock/glm", format!("{STEPS_A},{STEPS_B}"), false),
        ("mock/glm", format!("{STEPS_A},{STEPS_X},{STEPS_C}"), false),
        (
            "mock/glm",
            format!("{STEPS_A},{STEPS_X},{STEPS_C},{STEPS_D}"),
            false,
        ),
    ];
    for (model, messages, stream) in posts {
        let (status, _body, _h) = testkit::http_post(
            &addr,
            "/v1/chat/completions",
            steps_body(model, &messages, stream).as_bytes(),
            &[],
        );
        assert_eq!(status, 200, "canned step must be served ({model})");
    }
    if truncated_tail {
        let (status, _body, _h) = testkit::http_post(
            &addr,
            "/v1/chat/completions",
            steps_body(
                "mock/glm",
                &format!("{STEPS_A},{STEPS_X},{STEPS_C},{STEPS_D}"),
                true,
            )
            .as_bytes(),
            &[],
        );
        assert_eq!(status, 200, "the truncated stream's head was committed");
    }
    serve_task.abort();
    let _ = serve_task.await;

    // The records the ledger will read — from the files, in file order,
    // exactly as `vadis steps` finds them.
    let st = vadis_cli::steps::load_session(&config_path.to_string_lossy(), STEPS_SESSION)
        .expect("the session's records load");
    (config_path, st.records)
}
