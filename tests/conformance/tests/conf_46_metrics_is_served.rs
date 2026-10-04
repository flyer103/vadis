//! CONF-46 (spec §4.16 + §9.3, ADR-041 §5): **`GET /metrics` is served —
//! behind the token guard, in the Prometheus text exposition format, and
//! its refusal is §4.7's ordinary 401.** This file **replaces**
//! `conf_46_metrics_is_bare_404.rs` (the id is a contract and stays 46; a
//! file name is a description, and the old description is no longer
//! true). The protections the old case carried are conserved limb for
//! limb, re-pointed at the served surface:
//!
//! - **not unauthenticated** — the route sits behind §4.7's guard: no
//!   token → `401` with §8's body (`error.type = "unauthorized"`,
//!   `details.header`) and `X-Vadis-Request-Id` **present**; a wrong
//!   token → `401` naming the header it read;
//! - **not error-bodied on the admitted arm** — a `200` carries the
//!   exposition's own media type (`text/plain; version=0.0.4;
//!   charset=utf-8`), no JSON `error` member, and **no**
//!   `X-Vadis-Request-Id` (§8's always-on header exists only on
//!   responses that went through the error formatter);
//! - **the status set is closed** — admitted → `200`, refused → `401`;
//!   anything else (`404`, `501`, `500`, `503`) is a defect. The old
//!   case's `404`/`501` exclusion becomes this;
//! - **not request-shaped** — a GET carrying a canary body answers
//!   byte-identically to the body-less one, and the canary appears in no
//!   response byte and no trace file byte;
//! - **not unbounded** — an admitted scrape writes nothing (the trace
//!   dir's bytes are unchanged), while each refused arm adds exactly one
//!   record, the guard's boundary-class line with
//!   `protocol.protocol_in: "metrics"` and `errors[].kind:
//!   "unauthorized"`;
//! - **the key-absent control** — with no `server.auth_token_env`
//!   configured the same GET answers `200` with no token at all (§4.7's
//!   own control, CONF-45 ⑤'s shape);
//! - **the liveness control, kept verbatim** — `/health` answers `200`
//!   on the same run, so a refusal below is the guard's answer, never a
//!   dead socket's.
//!
//! Asserted against the real `serve` assembly over loopback HTTP.

#![forbid(unsafe_code)]

use vadis_conformance::testkit;

fn config_yaml(listen_port: u16, auth_line: &str) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s{auth_line} }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: mock
    urls:
      chat: http://127.0.0.1:9/v1/chat/completions
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

/// A GET (optionally with a body — the canary limb needs one) over raw
/// TCP, returning (status, body bytes, response headers).
fn http_get(
    addr: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> (u16, Vec<u8>, Vec<(String, String)>) {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(addr).expect("connect");
    let mut req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if let Some(b) = body {
        req.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    req.push_str("Connection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).unwrap();
    if let Some(b) = body {
        stream.write_all(b).unwrap();
    }
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    let split = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("head/body split");
    let head = String::from_utf8_lossy(&buf[..split]).into_owned();
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("status line");
    let headers = head
        .lines()
        .skip(1)
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    (status, buf[split + 4..].to_vec(), headers)
}

fn header<'h>(headers: &'h [(String, String)], name: &str) -> Option<&'h str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn trace_records(trace_dir: &std::path::Path) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(trace_dir) else {
        return out;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        for line in std::fs::read_to_string(path).unwrap().lines() {
            if !line.trim().is_empty() {
                out.push(serde_json::from_str(line).expect("record json"));
            }
        }
    }
    out
}

/// Every byte of the trace dir (names + contents, sorted) — the
/// "an admitted scrape writes nothing" limb compares this across one.
fn trace_dir_bytes(trace_dir: &std::path::Path) -> Vec<u8> {
    let mut files: Vec<_> = std::fs::read_dir(trace_dir)
        .expect("trace dir exists")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .collect();
    files.sort();
    let mut out = Vec::new();
    for f in files {
        out.extend_from_slice(f.file_name().unwrap().as_encoded_bytes());
        out.extend_from_slice(&std::fs::read(&f).unwrap());
    }
    out
}

const CANARY: &[u8] = b"CONF46-CANARY-9f3e2a-request-bytes-the-route-must-never-read";

/// The guarded run: §4.7's four arms against `/metrics`, the canary
/// limb, and the writes-nothing limb — one rig.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_46_metrics_is_served_behind_the_guard() {
    let dir = testkit::tempdir("conf46-served");
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(
        &config_path,
        config_yaml(listen_port, ", auth_token_env: CONF46_GUARD_TOKEN"),
    )
    .unwrap();
    std::env::set_var("CONF46_MOCK_KEY", "sk-conf46");
    std::env::set_var("CONF46_GUARD_TOKEN", "tok-conf46-secret");

    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    let trace_dir = dir.join("state/traces");

    // The liveness control, kept verbatim from the old case: the same
    // run answers /health — and with no token, which also witnesses that
    // the exemption is /health's alone (spec §4.7).
    let (status, _b, _h) = http_get(&listen_addr, "/health", &[], None);
    assert_eq!(status, 200, "control: the assembly is up (/health answers)");

    // ① no token → 401: §8's body verbatim, X-Vadis-Request-Id present
    //    — the GUARD's refusal, not this surface's answer.
    let (status, body, headers) = http_get(&listen_addr, "/metrics", &[], None);
    assert_eq!(status, 401, "no token must be refused, got {status}");
    let v: serde_json::Value = serde_json::from_slice(&body).expect("§8 error body json");
    assert_eq!(v["error"]["type"], "unauthorized");
    assert_eq!(v["error"]["details"]["header"], serde_json::Value::Null);
    assert!(
        header(&headers, "x-vadis-request-id").is_some(),
        "a refusal went through §8's formatter: the always-on header is present"
    );

    // ② a wrong token → 401, naming the header it read.
    let (status, body, _h) = http_get(
        &listen_addr,
        "/metrics",
        &[("authorization", "Bearer tok-conf46-secreX")],
        None,
    );
    assert_eq!(status, 401, "a wrong token must be refused, got {status}");
    let v: serde_json::Value = serde_json::from_slice(&body).expect("§8 error body json");
    assert_eq!(v["error"]["type"], "unauthorized");
    assert_eq!(v["error"]["details"]["header"], "authorization");

    // The refused arms' trace lines: exactly one record each, the
    // guard's boundary class, with the endpoint's OWN protocol word.
    let records = trace_records(&trace_dir);
    assert_eq!(
        records.len(),
        2,
        "one record per refused scrape, nothing else"
    );
    for rec in &records {
        assert_eq!(rec["protocol"]["protocol_in"], "metrics");
        assert_eq!(rec["errors"][0]["kind"], "unauthorized");
        assert_eq!(rec["usage_missing"], true);
        assert_eq!(rec["result"]["status"], 401);
        assert_eq!(rec["identity"]["event_id"], 0);
    }

    // The writes-nothing baseline: the trace dir's bytes before any
    // admitted scrape.
    let before = trace_dir_bytes(&trace_dir);

    // ③ Bearer → 200: the exposition's own media type and shape.
    let (status, body_bearer, headers) = http_get(
        &listen_addr,
        "/metrics",
        &[("authorization", "Bearer tok-conf46-secret")],
        None,
    );
    assert_eq!(status, 200, "the right token is admitted, got {status}");
    assert_eq!(
        header(&headers, "content-type"),
        Some("text/plain; version=0.0.4; charset=utf-8"),
        "the Prometheus text exposition format, exactly"
    );
    let text = String::from_utf8(body_bearer.clone()).expect("the exposition is utf-8");
    assert!(text.contains("# HELP vadis_metrics_window_seconds"));
    assert!(text.contains("# TYPE vadis_metrics_window_seconds gauge"));
    assert!(
        text.contains("vadis_metrics_window_seconds 900"),
        "the window is stated in-band, always"
    );

    // ④ the admitted arm's two negative layers (the old case's own,
    //    re-pointed at 200): no §8 error body, no X-Vadis-Request-Id.
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body_bearer) {
        assert!(
            v.get("error").is_none(),
            "the admitted arm is never §8's error body, got {v}"
        );
    }
    assert!(
        header(&headers, "x-vadis-request-id").is_none(),
        "no X-Vadis-Request-Id on an admitted scrape (§8's header exists \
         only on responses that went through the error formatter)"
    );

    // ⑤ x-api-key → 200, byte-identical to the Bearer arm (the two
    //    accepted forms are one admission).
    let (status, body_apikey, _h) = http_get(
        &listen_addr,
        "/metrics",
        &[("x-api-key", "tok-conf46-secret")],
        None,
    );
    assert_eq!(status, 200);
    assert_eq!(
        body_apikey, body_bearer,
        "x-api-key and Bearer admit to the same byte-identical surface"
    );

    // The canary limb: a GET carrying a body the route must never read
    // answers byte-identically to the body-less one, and the canary
    // appears in no response byte.
    let (status, body_canary, _h) = http_get(
        &listen_addr,
        "/metrics",
        &[("authorization", "Bearer tok-conf46-secret")],
        Some(CANARY),
    );
    assert_eq!(status, 200);
    assert_eq!(
        body_canary, body_bearer,
        "a request body changes no response byte — the route reads nothing"
    );
    assert!(
        !body_canary.windows(CANARY.len()).any(|w| w == CANARY),
        "the canary appears in no response byte"
    );

    // An admitted scrape writes nothing: the trace dir's bytes are
    // exactly what they were before the three admitted arms.
    let after = trace_dir_bytes(&trace_dir);
    assert_eq!(
        before, after,
        "admitted scrapes add no record and change no trace byte"
    );
    let records = trace_records(&trace_dir);
    assert_eq!(
        records.len(),
        2,
        "still exactly the two refused-arm records after three admitted scrapes"
    );
    // And the canary is in no trace file byte either.
    let canary_in_trace = after.windows(CANARY.len()).any(|w| w == CANARY);
    assert!(!canary_in_trace, "the canary appears in no trace byte");

    serve_task.abort();
    let _ = serve_task.await;
}

/// The key-absent control (§4.7's own, CONF-45 ⑤'s shape): with no
/// `server.auth_token_env` the same GET answers `200` with no token at
/// all — a revision with no gate admits everything.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_46_metrics_keyless_admits_without_a_token() {
    let dir = testkit::tempdir("conf46-keyless");
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(listen_port, "")).unwrap();
    std::env::set_var("CONF46_MOCK_KEY", "sk-conf46");

    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);

    let (status, body, headers) = http_get(&listen_addr, "/metrics", &[], None);
    assert_eq!(
        status, 200,
        "no key configured ⇒ the scrape is admitted with no token, got {status}"
    );
    assert_eq!(
        header(&headers, "content-type"),
        Some("text/plain; version=0.0.4; charset=utf-8")
    );
    let text = String::from_utf8(body).expect("utf-8");
    assert!(text.contains("vadis_metrics_window_seconds 900"));
    assert!(
        header(&headers, "x-vadis-request-id").is_none(),
        "the admitted arm is not §8's answer here either"
    );

    serve_task.abort();
    let _ = serve_task.await;
}
