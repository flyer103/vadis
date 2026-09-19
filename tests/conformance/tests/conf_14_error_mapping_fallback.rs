//! CONF-14 (DESIGN §10 conformance · robustness): upstream 4xx/5xx →
//! ADR-011 classification → the mapped action. 429/5xx/403-quota fail
//! over to the fallback provider and record `failover_from`; 403 with an
//! exhaustion keyword demotes the **whole provider** (the next request
//! skips it without an attempt); a deterministic 400 `format_error` is
//! never retried. Proven over real HTTP against loopback mock upstreams
//! (the same form as CONF-01; R2-2g re-verifies with a real client).

#![forbid(unsafe_code)]

use router_conformance::testkit::{self, CannedResponse};

fn config_yaml(a_port: u16, b_port: u16, listen_port: u16) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: mock-a
    base_url: http://127.0.0.1:{a_port}/v1
    api_key_env: CONF14_A_KEY
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
  - name: mock-b
    base_url: http://127.0.0.1:{b_port}/v1
    api_key_env: CONF14_B_KEY
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
fallback: [mock-b/glm]
"#
    )
}

const CLIENT_BODY: &str =
    r#"{"model":"mock-a/glm","messages":[{"role":"user","content":"hi"}],"stream":false}"#;

const B_OK: &str = r#"{"id":"b-1","choices":[{"index":0,"message":{"role":"assistant","content":"from-b"}}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;

struct Rig {
    a: testkit::MockUpstream,
    b: testkit::MockUpstream,
    listen_addr: String,
    serve_task: tokio::task::JoinHandle<i32>,
}

async fn rig(tag: &str, queue_a: Vec<CannedResponse>, queue_b: Vec<CannedResponse>) -> Rig {
    let dir = testkit::tempdir(tag);
    let a = testkit::MockUpstream::start().await.unwrap();
    let b = testkit::MockUpstream::start().await.unwrap();
    for r in queue_a {
        a.queue(r);
    }
    for r in queue_b {
        b.queue(r);
    }
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(
        &config_path,
        config_yaml(a.addr.port(), b.addr.port(), listen_port),
    )
    .unwrap();
    std::env::set_var("CONF14_A_KEY", "sk-a");
    std::env::set_var("CONF14_B_KEY", "sk-b");
    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    Rig {
        a,
        b,
        listen_addr,
        serve_task,
    }
}

/// 429 on the primary → classified `rate_limit` → failover to the fallback
/// provider → 200; the switch is recorded on the response
/// (`x-router-failover-from`), and each upstream saw exactly one request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_14_rate_limit_fails_over_to_fallback() {
    let rig = rig(
        "conf14-429",
        vec![CannedResponse::json(
            429,
            "Too Many Requests",
            br#"{"error":{"message":"rate limit exceeded","type":"rate_limit_error"}}"#,
        )],
        vec![CannedResponse::json(200, "OK", B_OK.as_bytes())],
    )
    .await;

    let (status, body, headers) = testkit::http_post(
        &rig.listen_addr,
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[],
    );

    assert_eq!(status, 200, "the fallback provider answered");
    assert_eq!(
        body,
        B_OK.as_bytes(),
        "the fallback's bytes relayed verbatim"
    );
    let from = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("x-router-failover-from"))
        .map(|(_, v)| v.as_str());
    assert_eq!(from, Some("mock-a/glm"), "failover_from is recorded");

    assert_eq!(rig.a.requests().len(), 1, "one attempt on the primary");
    assert_eq!(rig.b.requests().len(), 1, "one attempt on the fallback");

    rig.serve_task.abort();
    let _ = rig.serve_task.await;
}

/// 403 with an account-exhaustion keyword → `quota_exhausted` → failover
/// AND a provider-wide demotion: the **next** request skips mock-a entirely
/// (no second attempt on it) while still being answered by mock-b.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_14_quota_exhausted_demotes_whole_provider() {
    let rig = rig(
        "conf14-quota",
        vec![CannedResponse::json(
            403,
            "Forbidden",
            br#"{"error":{"message":"You have insufficient_quota, billing hard limit reached"}}"#,
        )],
        vec![CannedResponse::json(200, "OK", B_OK.as_bytes())],
    )
    .await;

    // First request: 403-quota on A → failover to B → 200.
    let (status, _body, _) = testkit::http_post(
        &rig.listen_addr,
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[],
    );
    assert_eq!(status, 200, "the fallback provider answered");
    assert_eq!(rig.a.requests().len(), 1);
    assert_eq!(rig.b.requests().len(), 1);

    // Second request: mock-a is demoted provider-wide (ADR-011 item 4) —
    // no new attempt on it; mock-b answers directly.
    let (status2, body2, headers2) = testkit::http_post(
        &rig.listen_addr,
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[],
    );
    assert_eq!(status2, 200);
    assert_eq!(body2, B_OK.as_bytes());
    assert_eq!(
        rig.a.requests().len(),
        1,
        "the demoted provider is not attempted again"
    );
    assert_eq!(rig.b.requests().len(), 2);
    // No failover header on a direct hit.
    assert!(headers2
        .iter()
        .all(|(k, _)| !k.eq_ignore_ascii_case("x-router-failover-from")));

    rig.serve_task.abort();
    let _ = rig.serve_task.await;
}

/// A deterministic 400 `format_error` is classified and **not retried**:
/// the fallback provider receives nothing, and the client gets the
/// normalized error body carrying the class.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_14_format_error_is_not_retried() {
    let rig = rig(
        "conf14-400",
        vec![CannedResponse::json(
            400,
            "Bad Request",
            br#"{"error":{"message":"Unknown parameter: 'x'","type":"invalid_request_error"}}"#,
        )],
        vec![CannedResponse::json(200, "OK", B_OK.as_bytes())],
    )
    .await;

    let (status, body, _headers) = testkit::http_post(
        &rig.listen_addr,
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[],
    );

    assert_eq!(
        status, 502,
        "a deterministic upstream rejection maps to 502"
    );
    let v: serde_json::Value = serde_json::from_slice(&body).expect("normalized error json");
    assert_eq!(v["error"]["type"], "upstream_error", "error code mapping");
    assert_eq!(
        v["error"]["details"]["error_class"], "format_error",
        "the classification travels in details"
    );

    assert_eq!(rig.a.requests().len(), 1, "the primary was attempted once");
    assert_eq!(
        rig.b.requests().len(),
        0,
        "a deterministic rejection never reaches the fallback provider"
    );

    rig.serve_task.abort();
    let _ = rig.serve_task.await;
}

/// Both providers failing with retryable classes exhausts the chain: the
/// client gets one normalized 502 naming the last class; each provider
/// was attempted exactly once (provider exclusion, spec §4.2).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_14_exhausted_chain_reports_last_class() {
    let rig = rig(
        "conf14-exhausted",
        vec![
            CannedResponse::json(
                500,
                "Internal Server Error",
                br#"{"error":{"message":"boom"}}"#,
            ),
            CannedResponse::json(529, "Overloaded", br#"{"error":{"message":"overloaded"}}"#),
        ],
        vec![CannedResponse::json(
            500,
            "Internal Server Error",
            br#"{"error":{"message":"boom"}}"#,
        )],
    )
    .await;

    let (status, body, _headers) = testkit::http_post(
        &rig.listen_addr,
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[],
    );

    assert_eq!(status, 502, "an exhausted chain maps to 502");
    let v: serde_json::Value = serde_json::from_slice(&body).expect("normalized error json");
    assert_eq!(v["error"]["type"], "upstream_error");
    assert_eq!(
        v["error"]["details"]["error_class"], "server_error",
        "the last classification is reported"
    );

    assert_eq!(
        rig.a.requests().len(),
        1,
        "provider exclusion: one attempt per provider"
    );
    assert_eq!(rig.b.requests().len(), 1);

    rig.serve_task.abort();
    let _ = rig.serve_task.await;
}
