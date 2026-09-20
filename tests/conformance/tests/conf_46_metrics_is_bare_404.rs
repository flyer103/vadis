//! CONF-46 (spec §9.3): **`GET /metrics` is a bare 404** — v0.1 does not
//! register the route, and an unrouted path does not go through §8's
//! error envelope.
//!
//! Asserted against the real `serve` assembly over loopback HTTP, with a
//! liveness control so the 404 cannot be "nothing listening":
//!
//! - `GET /health` on the same run answers `200` (the assembly is up);
//! - `GET /metrics` answers `404` — the status itself is the first layer:
//!   never `200` (a served metrics surface) and never `501` (a registered
//!   but unimplemented route), because an unregistered path has no handler
//!   to choose either;
//! - the response carries no §8 error body: no JSON `error` member, and
//!   no `X-Router-Request-Id` header (§8's always-on header exists only
//!   on responses that went through the error body's formatter).
//!
//! The negative layers are what make the case decisive: a future handler
//! that answers `200`, `501`, or a formatted error body all turn it red.

#![forbid(unsafe_code)]

use router_conformance::testkit;

fn config_yaml(listen_port: u16) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: mock
    base_url: http://127.0.0.1:9/v1
    api_key_env: CONF46_MOCK_KEY
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

/// Minimal blocking GET returning (status, body, response headers).
fn http_get(addr: &str, path: &str) -> (u16, Vec<u8>, Vec<(String, String)>) {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(addr).expect("connect");
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    let text = String::from_utf8_lossy(&buf).into_owned();
    let status: u16 = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("status line");
    let (head, body) = match text.split_once("\r\n\r\n") {
        Some((h, b)) => (h.to_string(), b.as_bytes().to_vec()),
        None => (text.clone(), Vec::new()),
    };
    let headers = head
        .lines()
        .skip(1)
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    (status, body, headers)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_46_metrics_is_a_bare_404() {
    let dir = testkit::tempdir("conf46-metrics");
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(listen_port)).unwrap();
    std::env::set_var("CONF46_MOCK_KEY", "sk-conf46");

    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);

    // Liveness control: the same run answers /health — the 404 below is
    // the router's answer, not a dead socket's.
    let (status, _body, _h) = http_get(&listen_addr, "/health");
    assert_eq!(status, 200, "control: the assembly is up (/health answers)");

    // The case proper: 404, which itself excludes both wrong answers a
    // handler could give — 200 (a served surface) and 501 (a registered
    // but unimplemented one). An unregistered path has no handler at all.
    let (status, body, headers) = http_get(&listen_addr, "/metrics");
    assert_eq!(status, 404, "GET /metrics must be a bare 404, got {status}");

    // No §8 error envelope: the body is not the formatter's JSON (an
    // unrouted path never reaches it), witnessed both by the body's own
    // shape and by the absence of §8's always-on request-id header.
    if let Some(v) = serde_json::from_slice::<serde_json::Value>(&body).ok() {
        assert!(
            v.get("error").is_none(),
            "an unrouted path must not go through §8's error body, got {v}"
        );
    }
    assert!(
        headers
            .iter()
            .all(|(k, _)| !k.eq_ignore_ascii_case("x-router-request-id")),
        "no X-Router-Request-Id on a bare 404 (§8's header exists only on \
         responses that went through the error body)"
    );

    serve_task.abort();
    let _ = serve_task.await;
}
