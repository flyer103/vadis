//! CONF-10: after removing vadis-owned fields, all remaining bytes are
//! byte-identical to the client's; the `router_meta` echo never reaches the
//! upstream.
//!
//! Two levels, both really executed here: the byte-level removal semantics
//! driving `vadis_core::RawBody::remove_top_level_keys` (ADR-007 single-pass
//! span scan; DESIGN §12.3.1), and the **proxy-chain-level** case below, which
//! proves over real HTTP that the removal is enforced end to end on every turn
//! of a session.

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, CannedResponse};
use vadis_core::{RawBody, VADIS_OWNED_TOP_LEVEL_KEYS};

/// Main case: a realistic client request (escapes, multi-byte UTF-8, nested
/// structure, trailing newline); after deleting the whitelisted key, the
/// output is **byte-identical** except for the deleted member and its
/// separator comma.
#[tokio::test]
async fn conf_10_byte_exact_after_vadis_field_removal() {
    // The trailing \n stays in the literal on purpose: trailing newlines
    // must pass through verbatim.
    let input = "{\
        \"model\": \"provider/model\",\
        \"messages\": [{\"role\": \"system\", \"content\": \"a{b},\\\"c\\\\\\\"\\\\ud83d\\\\ude00\"}],\
        \"tools\": [{\"x\": [1, {\"y\": \"},]\"}]}],\
        \"temperature\": 1e-9,\
        \"router_meta\": {\"echo\": true, \"nested\": [{\"k\": \"v\"}]},\
        \"stream\": true\
    }\n";
    let raw = RawBody::new(input.as_bytes().to_vec());

    let out = raw
        .remove_top_level_keys(VADIS_OWNED_TOP_LEVEL_KEYS)
        .expect("well-formed body must succeed");

    // Expected = the same input minus the router_meta member + its leading
    // comma; every other byte untouched.
    let expected = "{\
        \"model\": \"provider/model\",\
        \"messages\": [{\"role\": \"system\", \"content\": \"a{b},\\\"c\\\\\\\"\\\\ud83d\\\\ude00\"}],\
        \"tools\": [{\"x\": [1, {\"y\": \"},]\"}]}],\
        \"temperature\": 1e-9,\
        \"stream\": true\
    }\n";
    assert_eq!(out.as_bytes(), expected.as_bytes());

    // Semantic cross-check: router_meta is gone, every other key is present.
    let v: serde_json::Value = serde_json::from_slice(out.as_bytes()).unwrap();
    assert!(v.get("router_meta").is_none());
    assert_eq!(v["model"], "provider/model");
    assert_eq!(v["stream"], true);
}

/// With no matching key it is a strict no-op: output **byte-identical** to
/// input (including all whitespace).
#[tokio::test]
async fn conf_10_noop_when_no_vadis_fields_present() {
    let input = "{\n  \"model\": \"m\",\n  \"n\": [1, 2, {\"deep\": \"},\"}]\n}\r\n";
    let raw = RawBody::new(input.as_bytes().to_vec());
    let out = raw
        .remove_top_level_keys(VADIS_OWNED_TOP_LEVEL_KEYS)
        .expect("well-formed body must succeed");
    assert_eq!(out.as_bytes(), input.as_bytes());
}

/// Idempotent: two consecutive removals == one removal (a corollary of
/// AGENTS hard constraint 2's content determinism).
#[tokio::test]
async fn conf_10_removal_is_idempotent() {
    let input = "{\"a\":1,\"router_meta\":{\"b\":[2,{\"c\":\"},\"}],\"d\":null},\"e\":true}";
    let raw = RawBody::new(input.as_bytes().to_vec());
    let once = raw
        .remove_top_level_keys(VADIS_OWNED_TOP_LEVEL_KEYS)
        .unwrap();
    let twice = once
        .remove_top_level_keys(VADIS_OWNED_TOP_LEVEL_KEYS)
        .unwrap();
    assert_eq!(once.as_bytes(), twice.as_bytes());
    assert_eq!(once.as_bytes(), b"{\"a\":1,\"e\":true}");
}

/// The proxy-chain-level case: a client that keeps sending a `router_meta`
/// member — which is what a client does once the vadis has echoed one back to
/// it — must still have **every** turn's bytes reach the upstream without that
/// member: the removal is enforced on the chain, not only inside `RawBody`.
///
/// Scope, stated so this case is not read as proving more than it does: the
/// response-side *echo injection* itself (DESIGN §12.10.5) is not implemented in
/// v0.1 — `router_meta` appears in no forwarding source — so what is asserted
/// is the enforcement that keeps an echo out of the upstream request, on both
/// turns of one sticky session. The echo remains a tracked gap.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_10_router_meta_echo_never_reaches_upstream() {
    // Turn 1 and turn 2 of one session: an agent resends the conversation and
    // keeps carrying the vadis-owned echo field. The key of the session is
    // `prompt_cache_key`, which must itself pass through byte-identically.
    const TURN_1: &str = r#"{
  "model": "mock/glm",
  "messages": [{"role": "user", "content": "a{b}, \"quoted\" \\ backslash"}],
  "prompt_cache_key": "conf10-session",
  "router_meta": {"echo": true, "nested": [{"k": "v"}]},
  "stream": false
}
"#;
    const TURN_1_EXPECTED: &str = r#"{
  "model": "glm",
  "messages": [{"role": "user", "content": "a{b}, \"quoted\" \\ backslash"}],
  "prompt_cache_key": "conf10-session",
  "stream": false
}
"#;
    const TURN_2: &str = r#"{
  "model": "mock/glm",
  "messages": [{"role": "user", "content": "a{b}, \"quoted\" \\ backslash"}, {"role": "assistant", "content": "ok"}, {"role": "user", "content": "again"}],
  "prompt_cache_key": "conf10-session",
  "router_meta": {"echo": true, "nested": [{"k": "v"}]},
  "stream": false
}
"#;
    const TURN_2_EXPECTED: &str = r#"{
  "model": "glm",
  "messages": [{"role": "user", "content": "a{b}, \"quoted\" \\ backslash"}, {"role": "assistant", "content": "ok"}, {"role": "user", "content": "again"}],
  "prompt_cache_key": "conf10-session",
  "stream": false
}
"#;
    const UPSTREAM_OK: &str = r#"{"id":"resp-1","choices":[{"index":0,"message":{"role":"assistant","content":"ok"}}],"usage":{"prompt_tokens":100,"completion_tokens":5,"total_tokens":105}}"#;

    let dir = testkit::tempdir("conf10");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    upstream.queue(CannedResponse::json(200, "OK", UPSTREAM_OK.as_bytes()));
    upstream.queue(CannedResponse::json(200, "OK", UPSTREAM_OK.as_bytes()));

    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let config = format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: mock
    urls:
      chat: http://127.0.0.1:{upstream_port}/v1/chat/completions
    api_key_env: CONF10_MOCK_KEY
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
        upstream_port = upstream.addr.port()
    );
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config).unwrap();
    std::env::set_var("CONF10_MOCK_KEY", "sk-conf10");

    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);

    for (turn, body) in [(1, TURN_1), (2, TURN_2)] {
        let (status, _, _) =
            testkit::http_post(&listen_addr, "/v1/chat/completions", body.as_bytes(), &[]);
        assert_eq!(status, 200, "turn {turn}: vadis status");
    }

    let requests = upstream.requests();
    assert_eq!(requests.len(), 2, "one upstream attempt per turn");

    for (i, (req, expected)) in requests
        .iter()
        .zip([TURN_1_EXPECTED, TURN_2_EXPECTED])
        .enumerate()
    {
        let turn = i + 1;
        // The vadis-owned member is absent, in bytes and as a substring.
        assert!(
            !String::from_utf8_lossy(&req.body).contains("router_meta"),
            "turn {turn}: router_meta must never reach the upstream"
        );
        // Everything else is byte-identical: not a reserialization. (The
        // `model` value is the native id — mutation (b), CONF-27's domain —
        // so the expected bodies above already carry it.)
        assert_eq!(
            req.body,
            expected.as_bytes(),
            "turn {turn}: upstream-visible body must be the client body minus router_meta, with the native model id"
        );
        // The session key the sticky table uses passed through unchanged.
        assert!(
            String::from_utf8_lossy(&req.body).contains("\"prompt_cache_key\": \"conf10-session\""),
            "turn {turn}: the session key must pass through verbatim"
        );
    }

    serve_task.abort();
    let _ = serve_task.await;
}
