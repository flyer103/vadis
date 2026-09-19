//! CONF-03 (DESIGN §10 conformance · fidelity): anthropic inbound →
//! `wire_api: anthropic` native passthrough; the upstream-visible body is
//! byte-identical to the client body minus router-owned top-level keys, with
//! the value of the top-level `model` member replaced by the resolved
//! provider-native id — spec §2 permits exactly those two mutations (R2G1).
//! Proven over real HTTP against a loopback mock upstream (the same form
//! as CONF-01). Anthropic auth travels as `x-api-key` + `anthropic-version`,
//! never a bearer.

#![forbid(unsafe_code)]

use router_conformance::testkit::{self, CannedResponse};

const CLIENT_BODY: &str = r#"{"model":"mock/clm","max_tokens":64,"system":"be {exact}","messages":[{"role":"user","content":"héllo 😀"}],"metadata":{"user_id":"u-1"},"router_meta":{"echo":true},"stream":false}
"#;

const EXPECTED_UPSTREAM_BODY: &str = r#"{"model":"clm","max_tokens":64,"system":"be {exact}","messages":[{"role":"user","content":"héllo 😀"}],"metadata":{"user_id":"u-1"},"stream":false}
"#;

const UPSTREAM_OK: &str = r#"{"id":"msg_1","content":[{"type":"text","text":"ok"}],"usage":{"input_tokens":2000,"output_tokens":32,"cache_creation_input_tokens":100,"cache_read_input_tokens":1800}}"#;

fn config_yaml(upstream_port: u16, listen_port: u16) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: mock
    base_url: http://127.0.0.1:{upstream_port}
    api_key_env: CONF03_MOCK_KEY
    wire_api: anthropic
    supports: [anthropic]
    models:
      - id: clm
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
async fn conf_03_native_anthropic_passthrough() {
    let dir = testkit::tempdir("conf03");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    upstream.queue(CannedResponse::json(200, "OK", UPSTREAM_OK.as_bytes()));

    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(upstream.addr.port(), listen_port)).unwrap();

    std::env::set_var("CONF03_MOCK_KEY", "sk-conf03");

    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);

    let (status, body, _headers) =
        testkit::http_post(&listen_addr, "/v1/messages", CLIENT_BODY.as_bytes(), &[]);

    assert_eq!(status, 200, "router status");
    assert_eq!(
        body,
        UPSTREAM_OK.as_bytes(),
        "response bytes relayed verbatim"
    );

    let requests = upstream.requests();
    assert_eq!(requests.len(), 1, "exactly one upstream attempt");
    let req = &requests[0];
    assert_eq!(req.method, "POST");
    // Anthropic path assembly: base without /v1 + /v1/messages.
    assert_eq!(req.path, "/v1/messages");

    assert_eq!(
        req.body,
        EXPECTED_UPSTREAM_BODY.as_bytes(),
        "upstream-visible body must be byte-identical to the client body minus router_meta, with the native model id"
    );
    assert!(!String::from_utf8_lossy(&req.body).contains("router_meta"));

    // Anthropic auth shape, never a bearer.
    assert_eq!(req.header("x-api-key"), Some("sk-conf03"));
    assert_eq!(req.header("anthropic-version"), Some("2023-06-01"));
    assert!(req.header("authorization").is_none());

    serve_task.abort();
    let _ = serve_task.await;
}
