//! CONF-01 (DESIGN §10 conformance · fidelity): chat inbound → `wire_api:
//! chat` native passthrough; the upstream-visible body is byte-identical to
//! the client body minus the vadis-owned top-level keys, with the value of
//! the top-level `model` member replaced by the resolved provider-native id —
//! spec §2 permits exactly those two mutations. Proven over real HTTP
//! against a loopback mock upstream that records exactly the bytes it received
//! (no real key, no network egress).

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, CannedResponse};

const CLIENT_BODY: &str = r#"{
  "model": "mock/glm",
  "messages": [{"role": "system", "content": "a{b}, \"quoted\" \\ backslash"}, {"role": "user", "content": "héllo 😀"}],
  "tools": [{"type": "function", "function": {"name": "f", "parameters": {"x": [1, 2, {"y": "brace } comma ,"}]}}}],
  "temperature": 1e-9,
  "router_meta": {"echo": true, "nested": [{"k": "v"}]},
  "stream": false
}
"#;

/// The same bytes minus the `router_meta` member and its leading comma, and
/// with the `model` value replaced by the route's native id (`mock/glm` → the
/// roster entry `glm`) — the two permitted rewrites (AGENTS constraint 1).
const EXPECTED_UPSTREAM_BODY: &str = r#"{
  "model": "glm",
  "messages": [{"role": "system", "content": "a{b}, \"quoted\" \\ backslash"}, {"role": "user", "content": "héllo 😀"}],
  "tools": [{"type": "function", "function": {"name": "f", "parameters": {"x": [1, 2, {"y": "brace } comma ,"}]}}}],
  "temperature": 1e-9,
  "stream": false
}
"#;

const UPSTREAM_OK: &str = r#"{"id":"resp-1","choices":[{"index":0,"message":{"role":"assistant","content":"ok"}}],"usage":{"prompt_tokens":14409,"completion_tokens":111,"total_tokens":14520}}"#;

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
    api_key_env: CONF01_MOCK_KEY
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_01_native_chat_passthrough() {
    let dir = testkit::tempdir("conf01");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    upstream.queue(CannedResponse::json(200, "OK", UPSTREAM_OK.as_bytes()));

    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(upstream.addr.port(), listen_port)).unwrap();

    // The api key exists for the transport wiring; its value never reaches
    // an assertion target other than the upstream's own header check.
    std::env::set_var("CONF01_MOCK_KEY", "sk-conf01");

    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);

    let (status, body, _headers) = testkit::http_post(
        &listen_addr,
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[],
    );

    // (a) 200 and the upstream's response bytes relayed verbatim.
    assert_eq!(status, 200, "vadis status");
    assert_eq!(
        body,
        UPSTREAM_OK.as_bytes(),
        "response body must be the upstream's bytes verbatim"
    );

    // (b) the mock upstream received exactly one request at the chat path.
    let requests = upstream.requests();
    assert_eq!(requests.len(), 1, "exactly one upstream attempt");
    let req = &requests[0];
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, "/v1/chat/completions");

    // (c) BYTE FIDELITY: what the upstream received equals the client's
    // bytes minus the vadis-owned member — not a reserialization.
    assert_eq!(
        req.body,
        EXPECTED_UPSTREAM_BODY.as_bytes(),
        "upstream-visible body must be byte-identical to the client body minus router_meta, with the native model id"
    );
    // The router_meta substring really is absent upstream.
    assert!(!String::from_utf8_lossy(&req.body).contains("router_meta"));

    // (d) auth traveled as the provider's bearer, not the client's anything.
    assert_eq!(req.header("authorization"), Some("Bearer sk-conf01"));
    assert_eq!(req.header("content-type"), Some("application/json"));

    serve_task.abort();
    let _ = serve_task.await;
}
