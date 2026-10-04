//! CONF-02 (DESIGN §10 conformance · fidelity): responses inbound →
//! `wire_api: responses` native passthrough; the upstream-visible body is
//! byte-identical to the client body minus vadis-owned top-level keys, with
//! the value of the top-level `model` member replaced by the resolved
//! provider-native id — spec §2 permits exactly those two mutations.
//! Proven over real HTTP against a loopback mock upstream (the same form
//! as CONF-01).

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, CannedResponse};

const CLIENT_BODY: &str = r#"{"model":"mock/rsp","input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"héllo 😀"}]}],"instructions":"be {exact}","tools":[],"store":false,"prompt_cache_key":"sess-42","vadis_meta":{"echo":true},"stream":false}
"#;

const EXPECTED_UPSTREAM_BODY: &str = r#"{"model":"rsp","input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"héllo 😀"}]}],"instructions":"be {exact}","tools":[],"store":false,"prompt_cache_key":"sess-42","stream":false}
"#;

const UPSTREAM_OK: &str = r#"{"id":"resp_1","object":"response","usage":{"input_tokens":1000,"input_tokens_details":{"cached_tokens":900},"output_tokens":50,"total_tokens":1050}}"#;

fn config_yaml(upstream_port: u16, listen_port: u16) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: mock
    urls:
      responses: http://127.0.0.1:{upstream_port}/v1/responses
    api_key_env: CONF02_MOCK_KEY
    wire_api: responses
    supports: [responses]
    models:
      - id: rsp
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
async fn conf_02_native_responses_passthrough() {
    let dir = testkit::tempdir("conf02");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    upstream.queue(CannedResponse::json(200, "OK", UPSTREAM_OK.as_bytes()));

    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(upstream.addr.port(), listen_port)).unwrap();

    std::env::set_var("CONF02_MOCK_KEY", "sk-conf02");

    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);

    let (status, body, _headers) =
        testkit::http_post(&listen_addr, "/v1/responses", CLIENT_BODY.as_bytes(), &[]);

    assert_eq!(status, 200, "vadis status");
    assert_eq!(
        body,
        UPSTREAM_OK.as_bytes(),
        "response bytes relayed verbatim"
    );

    let requests = upstream.requests();
    assert_eq!(requests.len(), 1, "exactly one upstream attempt");
    let req = &requests[0];
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, "/v1/responses");

    assert_eq!(
        req.body,
        EXPECTED_UPSTREAM_BODY.as_bytes(),
        "upstream-visible body must be byte-identical to the client body minus vadis_meta, with the native model id"
    );
    assert!(!String::from_utf8_lossy(&req.body).contains("vadis_meta"));
    // The session identity (prompt_cache_key) survives verbatim upstream.
    assert!(String::from_utf8_lossy(&req.body).contains("\"prompt_cache_key\":\"sess-42\""));

    assert_eq!(req.header("authorization"), Some("Bearer sk-conf02"));

    serve_task.abort();
    let _ = serve_task.await;
}
