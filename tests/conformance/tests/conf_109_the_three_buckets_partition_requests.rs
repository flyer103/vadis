//! CONF-109 (spec §4.20.4, §6's metric definitions, §9.2's provenance table,
//! §4.16's single-owner rule; DESIGN §12.8's row; ADR-056): **one record lands
//! in exactly one bucket, and `usage_missing` is not one of them.**
//!
//! The partition: `failed` = `result.status` not 2xx; `stream_incomplete` =
//! 2xx **with** an `errors[]` entry whose `details.error_class` is
//! `stream_truncated`; `succeeded` = every other 2xx. The invariant is
//! `succeeded + failed + stream_incomplete == requests`. `usage_missing`
//! partitions the same records on a second, orthogonal axis and is **never**
//! added to the three.
//!
//! Limbs:
//! - (a) **the partition**, over three requests in one window on one process:
//!   a clean `[DONE]` stream (2xx), an explicit upstream failure (non-2xx) and
//!   a silent-head-then-body-cut stream (2xx with a truncation entry) ⇒
//!   `vadis_requests 3`, `succeeded 1`, `failed 1`, `stream_incomplete 1`, and
//!   the three sum to `requests`. **Red at the base**: there is no incomplete
//!   series and `succeeded` reads 2.
//! - (b) **`usage_missing` is orthogonal**: none of the three carried a
//!   measurement, so `vadis_requests_usage_missing` is 3 **and** the three
//!   buckets are unchanged; the truncated record is counted in **both** lines
//!   (its own record's `usage_missing: true` is asserted too). Red for a
//!   contract that makes `usage_missing` a bucket — the sum would then miss
//!   the truncated record — or that excludes those records from the buckets.
//! - (c) **`usage_missing` alone never moves the bucket**: a byte-complete
//!   stream with `[DONE]` and no usage carrier ⇒ `errors[]` empty,
//!   `usage_missing: true`, counted in `succeeded` **and** in `usage missing`,
//!   and **not** in `stream_incomplete`. Red for a bucket rule built on
//!   `usage_missing` or on "`errors[]` is non-empty" (a `transform_error` /
//!   `trace_write_failed` rides a success).
//!
//! The `vadis stats` half of the single-owner rule (the same figures from the
//! same derivation) is `CONF-87`'s, whose aggregator adopts this predicate under
//! ADR-056 §5's amendment A3. No network egress: loopback mocks only.
#![forbid(unsafe_code)]

use serde_json::Value;
use vadis_conformance::testkit::{self, SseChunk};

const IDLE: &str = "400ms";

fn config_yaml(upstream_port: u16, listen_port: u16) -> String {
    format!(
        r#"
server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 3s, request_timeout: 8s, stream_idle_timeout: {IDLE} }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: mock
    urls:
      chat: http://127.0.0.1:{upstream_port}/v1/chat/completions
    api_key_env: CONF109_MOCK_KEY
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

const STREAM_BODY: &str = r#"{"model":"mock/glm","messages":[{"role":"user","content":"hi"}],"stream":true,"prompt_cache_key":"conf109-sess"}"#;

fn read_records(trace_dir: &std::path::Path) -> Vec<Value> {
    let mut records = Vec::new();
    for entry in std::fs::read_dir(trace_dir).expect("trace dir exists") {
        let entry = entry.unwrap();
        if entry.path().extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        for line in std::fs::read_to_string(entry.path()).unwrap().lines() {
            records.push(serde_json::from_str(line).expect("record json"));
        }
    }
    records
}

/// One blocking `GET`, returning (status, body, headers).
fn http_get(addr: &str, path: &str) -> (u16, Vec<u8>, Vec<(String, String)>) {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nconnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("headers");
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let mut lines = head.lines();
    let status: u16 = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .expect("status line");
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| {
            l.split_once(':')
                .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        })
        .collect();
    (status, raw[split + 4..].to_vec(), headers)
}

/// The value of one exposition sample, matched on the **exact** series name
/// (`vadis_requests` must not match `vadis_requests_succeeded`).
fn sample(text: &str, name: &str) -> Option<f64> {
    for line in text.lines() {
        if line.starts_with('#') {
            continue;
        }
        let (left, value) = line.split_once(' ')?;
        if left.split('{').next() == Some(name) {
            return value.trim().parse().ok();
        }
    }
    None
}

async fn start_vadis(
    config_path: &std::path::Path,
    listen_port: u16,
) -> tokio::task::JoinHandle<()> {
    std::env::set_var("CONF109_MOCK_KEY", "sk-conf109");
    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move {
        let _ = vadis_cli::serve(&cfg).await;
    });
    testkit::wait_listening(&format!("127.0.0.1:{listen_port}"));
    serve_task
}

/// (a)+(b) The partition, and `usage_missing` as an orthogonal axis.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "CONF-109: depends on R74-1"]
async fn conf_109_a_three_buckets_partition_requests() {
    let dir = testkit::tempdir("conf109a");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    // 1: a clean stream, no usage carrier (chat without `include_usage`).
    upstream.queue(testkit::CannedResponse::sse(vec![
        SseChunk::event(b"data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n"),
        SseChunk::event(b"data: [DONE]\n\n"),
    ]));
    // 2: an explicit upstream failure — a non-2xx client status.
    upstream.queue(testkit::CannedResponse::json(
        500,
        "Internal Server Error",
        b"{\"error\":\"explode\"}",
    ));
    // 3: a head, one event, then silence — 2xx with a truncation entry.
    upstream.queue(testkit::CannedResponse::sse(vec![
        SseChunk::event(b"data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\n"),
        SseChunk::event_after(b"data: never\n\n", 60_000),
    ]));

    let listen_port = testkit::free_port();
    let addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(upstream.addr.port(), listen_port)).unwrap();
    let serve_task = start_vadis(&config_path, listen_port).await;

    let (s1, _b1, _h1) =
        testkit::http_post(&addr, "/v1/chat/completions", STREAM_BODY.as_bytes(), &[]);
    let (s2, _b2, _h2) =
        testkit::http_post(&addr, "/v1/chat/completions", STREAM_BODY.as_bytes(), &[]);
    let (s3, _b3, _h3) =
        testkit::http_post(&addr, "/v1/chat/completions", STREAM_BODY.as_bytes(), &[]);

    let (m_status, m_body, _mh) = http_get(&addr, "/metrics");
    serve_task.abort();
    let _ = serve_task.await;

    assert_eq!(s1, 200, "the clean stream is served");
    assert_eq!(s2, 502, "the upstream failure is a refusal");
    assert_eq!(s3, 200, "the truncated stream's head was committed");
    assert_eq!(m_status, 200, "/metrics is served");

    let text = String::from_utf8_lossy(&m_body).to_string();
    let requests = sample(&text, "vadis_requests").expect("vadis_requests");
    let succeeded = sample(&text, "vadis_requests_succeeded").expect("succeeded");
    let failed = sample(&text, "vadis_requests_failed").expect("failed");
    let incomplete = sample(&text, "vadis_requests_stream_incomplete").expect("stream incomplete");
    assert_eq!(requests, 3.0, "three requests in the window:\n{text}");
    assert_eq!(succeeded, 1.0, "the clean stream, and only it:\n{text}");
    assert_eq!(failed, 1.0, "the refused attempt:\n{text}");
    assert_eq!(
        incomplete, 1.0,
        "the head-returned-body-cut record:\n{text}"
    );
    assert_eq!(
        succeeded + failed + incomplete,
        requests,
        "the three buckets PARTITION requests:\n{text}"
    );

    // (b) usage_missing is a second axis: all three lacked a measurement, and
    // that changes none of the three buckets.
    let usage_missing = sample(&text, "vadis_requests_usage_missing").expect("usage missing");
    assert_eq!(
        usage_missing, 3.0,
        "every record of this rig is usage-missing:\n{text}"
    );
    assert_eq!(
        succeeded + failed + incomplete,
        requests,
        "usage_missing is NOT a fourth bucket — the sum still holds:\n{text}"
    );

    // The truncated record is counted in BOTH lines: its own record says so.
    let records = read_records(&dir.join("state/traces"));
    assert_eq!(records.len(), 3, "one record per request");
    let truncated = records
        .iter()
        .find(|r| {
            r["result"]["status"] == 200
                && r["errors"]
                    .as_array()
                    .map(|es| {
                        es.iter()
                            .any(|e| e["details"]["error_class"] == "stream_truncated")
                    })
                    .unwrap_or(false)
        })
        .expect("the truncated record");
    assert_eq!(
        truncated["usage_missing"], true,
        "it is in the incomplete bucket AND in usage missing"
    );
}

/// (c) A complete stream with no usage carrier stays in `succeeded`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "CONF-109: depends on R74-1"]
async fn conf_109_c_a_complete_stream_without_usage_is_still_a_success() {
    let dir = testkit::tempdir("conf109c");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    // Byte-complete: an event and the wire's terminal, and no usage carrier
    // (chat carries usage only when the client asks for it).
    upstream.queue(testkit::CannedResponse::sse(vec![
        SseChunk::event(b"data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n"),
        SseChunk::event(b"data: [DONE]\n\n"),
    ]));

    let listen_port = testkit::free_port();
    let addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(upstream.addr.port(), listen_port)).unwrap();
    let serve_task = start_vadis(&config_path, listen_port).await;

    let (status, _b, _h) =
        testkit::http_post(&addr, "/v1/chat/completions", STREAM_BODY.as_bytes(), &[]);
    let (m_status, m_body, _mh) = http_get(&addr, "/metrics");
    serve_task.abort();
    let _ = serve_task.await;

    assert_eq!(status, 200);
    assert_eq!(m_status, 200);
    let text = String::from_utf8_lossy(&m_body).to_string();
    assert_eq!(sample(&text, "vadis_requests").expect("requests"), 1.0);
    assert_eq!(
        sample(&text, "vadis_requests_succeeded").expect("succeeded"),
        1.0,
        "a complete stream is a success even without a measurement:\n{text}"
    );
    assert_eq!(
        sample(&text, "vadis_requests_stream_incomplete").expect("incomplete"),
        0.0,
        "no truncation entry ⇒ not incomplete:\n{text}"
    );
    assert_eq!(
        sample(&text, "vadis_requests_usage_missing").expect("usage missing"),
        1.0,
        "…and it is usage-missing on the other axis:\n{text}"
    );

    let records = read_records(&dir.join("state/traces"));
    assert_eq!(records.len(), 1);
    let errors = records[0]["errors"].as_array().expect("errors array");
    assert!(
        errors.is_empty(),
        "`usage_missing` is not an error entry: {errors:?}"
    );
    assert_eq!(records[0]["usage_missing"], true);
}
