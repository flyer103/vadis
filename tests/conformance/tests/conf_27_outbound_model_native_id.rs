//! CONF-27 (DESIGN §12.8 + §12.10.7, spec §2/§3/§6): **the upstream request
//! carries the provider-native model id.** The client's `model` string is a
//! *route name* — `provider/model` or an alias — and routing resolves before
//! anything leaves the process, so the bytes handed to the provider carry the
//! resolved roster entry's own id.
//!
//! Three claims, all against the mock upstream's recorded bytes:
//!
//!   (a) the client sends `mock/glm` → the upstream receives `"model":"glm"`;
//!   (b) the client sends the alias `fast`, which resolves to the same route →
//!       the upstream request is **byte-identical** to (a)'s, and the trace
//!       records native id / client string / `alias` (vs `explicit`);
//!   (c) every other byte is the client's — whitespace, escapes, multi-byte
//!       UTF-8 and the trailing newline after the closing brace included.

#![forbid(unsafe_code)]

use router_conformance::testkit::{self, CannedResponse};

/// The direct form: the client names the route `provider/model`.
const DIRECT_BODY: &str = r#"{
  "model": "mock/glm",
  "messages": [{"role": "user", "content": "héllo 😀 \"q\" \\ back"}],
  "temperature": 1e-9,
  "router_meta": {"echo": true},
  "stream": false
}
"#;

/// The alias form: byte-for-byte the same conversation, one route-name
/// spelling different. After resolution the two must be indistinguishable
/// upstream, which is the whole point of the case.
const ALIAS_BODY: &str = r#"{
  "model": "fast",
  "messages": [{"role": "user", "content": "héllo 😀 \"q\" \\ back"}],
  "temperature": 1e-9,
  "router_meta": {"echo": true},
  "stream": false
}
"#;

/// What the upstream must receive for **both** forms: the client's bytes minus
/// the router-owned member, with the `model` value replaced by the roster id
/// `glm` — and nothing else touched, trailing newline included.
const EXPECTED_UPSTREAM_BODY: &str = r#"{
  "model": "glm",
  "messages": [{"role": "user", "content": "héllo 😀 \"q\" \\ back"}],
  "temperature": 1e-9,
  "stream": false
}
"#;

const UPSTREAM_OK: &str = r#"{"id":"r1","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":10,"total_tokens":110,"prompt_tokens_details":{"cached_tokens":50}}}"#;

fn config_yaml(upstream_port: u16, listen_port: u16) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: mock
    base_url: http://127.0.0.1:{upstream_port}/v1
    api_key_env: CONF27_MOCK_KEY
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

aliases: {{ fast: mock/glm }}
plugins: []
fallback: []
"#
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_27_outbound_model_is_the_provider_native_id() {
    let dir = testkit::tempdir("conf27");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    // One canned response: the mock repeats the last one, so both requests
    // are answered by it.
    upstream.queue(CannedResponse::json(200, "OK", UPSTREAM_OK.as_bytes()));

    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(upstream.addr.port(), listen_port)).unwrap();

    std::env::set_var("CONF27_MOCK_KEY", "sk-conf27");

    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);

    let (status, body, _h) = testkit::http_post(
        &listen_addr,
        "/v1/chat/completions",
        DIRECT_BODY.as_bytes(),
        &[],
    );
    assert_eq!(status, 200, "direct route: router status, body {:?}", body);
    let (status, body, _h) = testkit::http_post(
        &listen_addr,
        "/v1/chat/completions",
        ALIAS_BODY.as_bytes(),
        &[],
    );
    assert_eq!(status, 200, "alias route: router status, body {:?}", body);

    let requests = upstream.requests();
    assert_eq!(requests.len(), 2, "one upstream attempt per request");

    // (a) + (c): the direct form reaches the provider as the native id, with
    // every other byte — whitespace, escapes, multi-byte UTF-8 and the
    // trailing newline — the client's.
    assert_eq!(
        requests[0].body,
        EXPECTED_UPSTREAM_BODY.as_bytes(),
        "direct route: the upstream body is the client's, with the model replaced by the native id"
    );
    assert!(
        !String::from_utf8_lossy(&requests[0].body).contains("router_meta"),
        "the router-owned member never reaches the upstream"
    );
    assert!(
        requests[0].body.ends_with(b"}\n"),
        "the trailing newline after the closing brace survives"
    );

    // (b) the alias produces a byte-identical upstream request: the client
    // never gets to pick the string the provider sees, and the route does not
    // leak the spelling it was addressed by.
    assert_eq!(
        requests[1].body, requests[0].body,
        "alias and direct route must produce byte-identical upstream requests"
    );

    // ...and the trace keeps both facts apart (spec §6): the native id that
    // was billed, the client's own string, and how it was addressed.
    let trace_dir = dir.join("state/traces");
    let mut records: Vec<serde_json::Value> = Vec::new();
    for entry in std::fs::read_dir(&trace_dir).expect("trace dir") {
        let entry = entry.unwrap();
        if entry.path().extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        for line in std::fs::read_to_string(entry.path()).unwrap().lines() {
            records.push(serde_json::from_str(line).expect("record json"));
        }
    }
    assert_eq!(records.len(), 2, "one decision record per request");

    let mut seen: Vec<(String, String, String)> = records
        .iter()
        .map(|r| {
            (
                r["decision"]["model"].as_str().expect("model").to_string(),
                r["decision"]["requested_model"]
                    .as_str()
                    .expect("requested_model")
                    .to_string(),
                r["decision"]["selection_source"]
                    .as_str()
                    .expect("selection_source")
                    .to_string(),
            )
        })
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        vec![
            ("glm".to_string(), "fast".to_string(), "alias".to_string()),
            (
                "glm".to_string(),
                "mock/glm".to_string(),
                "explicit".to_string()
            ),
        ],
        "the trace records the resolved native id, the client's own string and the selection source"
    );

    serve_task.abort();
    let _ = serve_task.await;
}
