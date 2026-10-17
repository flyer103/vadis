//! CONF-108 (spec §4.20.1–§4.20.3, §4.15's read-site rule; DESIGN §12.8's row;
//! ADR-056): **a stream's silence is bounded by its own key, and a truncation
//! names its cause.**
//!
//! `server.stream_idle_timeout` bounds the gap between two forwarded body
//! bytes (default 30s, `1s..=300s`); the attempt knob keeps bounding *connect
//! and head arrival*; `server.request_timeout` keeps bounding the whole
//! request, and the relay's window stays `min(gap, time-until-deadline)`.
//! A truncation entry carries `details.stream_end` — `idle_timeout` (the
//! upstream went silent), `upstream_ended_early` (it closed before its wire's
//! terminal event) or `request_timeout` (our own outer bound) — beside
//! `relayed_bytes`, `window_ms` (present iff a bound tripped) and
//! `since_last_byte_ms`.
//!
//! Limbs (all through the real `serve` assembly against loopback mocks, with
//! `stream_idle_timeout: 400ms`, `upstream_attempt_timeout: 3s`,
//! `request_timeout: 8s`, so no limb rests on machine speed):
//!
//! - (a) **the silent stream is cut at the new key**: head + one event, then
//!   the socket is held open. The call returns in ≥ 400 ms and **< 3 s** (the
//!   attempt knob did not fire), `details.stream_end == "idle_timeout"`,
//!   `details.window_ms == 400`, `details.relayed_bytes` equals that event's
//!   bytes, `since_last_byte_ms ≥ 350`, `stream_completed: false`, no
//!   fabricated `[DONE]`. **Red at the base** (waits the 3 s knob, no cause
//!   word).
//! - (b) **a clean early end is a different cause**: the body ends after one
//!   event with no terminal event ⇒ `details.stream_end ==
//!   "upstream_ended_early"`, **no** `window_ms`, one `stream_truncated` entry.
//!   **Red at the base**, which writes this as a **complete** stream.
//! - (c) **the healthy stream, and the head is not this key's business**: the
//!   head is delayed 800 ms (under the 3 s attempt knob, over the 400 ms idle
//!   key), then six events 50 ms apart and `[DONE]`: the relayed body is
//!   byte-identical, `errors[]` empty, `stream_completed: true`, **no**
//!   `stream_end` on the record.
//! - (d) **reload reaches the next request only**: with the key at `4s`,
//!   request #1 drives a silent upstream while the pair is rewritten to
//!   `400ms`; #1 keeps `window_ms: 4000`, #2 trips at `window_ms: 400`, and the
//!   reload was **applied** (the digest moved) rather than refused.
//!
//! No network egress: loopback mocks only; the cache capability is not mounted.
#![forbid(unsafe_code)]

use serde_json::Value;
use vadis_conformance::testkit::{self, SseChunk};

/// The gap bound (spec §4.20.1) — the only key that bounds a body's silence.
const IDLE_MS: u64 = 400;
/// The attempt knob — head arrival and connect, never the body's gap.
const ATTEMPT: &str = "3s";
/// The whole-request bound, always the outer limit.
const REQUEST: &str = "8s";

fn config_yaml(upstream_port: u16, listen_port: u16, idle: &str, request: &str) -> String {
    format!(
        r#"
server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: {ATTEMPT}, request_timeout: {request}, stream_idle_timeout: {idle} }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: mock
    urls:
      chat: http://127.0.0.1:{upstream_port}/v1/chat/completions
    api_key_env: CONF108_MOCK_KEY
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

const STREAM_BODY: &str = r#"{"model":"mock/glm","messages":[{"role":"user","content":"stream me"}],"stream":true,"prompt_cache_key":"conf108-sess"}"#;

/// Every JSONL line under the trace dir, parsed.
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

/// Every stored event as (kind_raw, payload, ts_us), read after the server
/// stopped (the store's writer lock is `serve`'s while it runs).
fn read_events(dir: &std::path::Path) -> Vec<(String, Value, i64)> {
    use vadis_core::store::{Query, QueryRow, Store as _};
    let store = vadis_store::SqliteStore::open(&dir.join("state/vadis.db")).unwrap();
    let QueryRow::Events(rows) = store.query(Query::AllEvents).unwrap() else {
        panic!("events query");
    };
    rows.into_iter()
        .map(|r| (r.kind_raw, r.payload, r.ts_us))
        .collect()
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

async fn start_vadis(
    config_path: &std::path::Path,
    listen_port: u16,
) -> tokio::task::JoinHandle<()> {
    std::env::set_var("CONF108_MOCK_KEY", "sk-conf108");
    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move {
        let _ = vadis_cli::serve(&cfg).await;
    });
    testkit::wait_listening(&format!("127.0.0.1:{listen_port}"));
    serve_task
}

/// The `/health` `config_digest`, the reload's own witness (CONF-96's
/// technique).
fn config_digest(addr: &str) -> String {
    let (status, body, _) = http_get(addr, "/health");
    assert_eq!(status, 200, "/health answers");
    let v: Value = serde_json::from_slice(&body).expect("/health is json");
    v["config"]["config_digest"]
        .as_str()
        .expect("config_digest is a string")
        .to_string()
}

/// The one truncation entry of a record, or a panic naming what was found.
fn truncation(rec: &Value) -> &Value {
    let errors = rec["errors"].as_array().expect("errors array");
    errors
        .iter()
        .find(|e| e["details"]["error_class"] == "stream_truncated")
        .unwrap_or_else(|| panic!("a truncation entry: {errors:?}"))
}

/// (a) The silent stream: cut at the new key, with its cause named.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_108_a_silence_is_cut_at_the_gap_bound() {
    let dir = testkit::tempdir("conf108a");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    // One event immediately, then a long delay with the socket held open —
    // the stalled-but-connected fixture.
    let event = b"data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\n";
    upstream.queue(testkit::CannedResponse::sse(vec![
        SseChunk::event(event),
        SseChunk::event_after(b"data: never\n\n", 60_000),
    ]));

    let listen_port = testkit::free_port();
    let config_path = dir.join("config.yaml");
    std::fs::write(
        &config_path,
        config_yaml(
            upstream.addr.port(),
            listen_port,
            &format!("{IDLE_MS}ms"),
            REQUEST,
        ),
    )
    .unwrap();
    let serve_task = start_vadis(&config_path, listen_port).await;

    let t0 = std::time::Instant::now();
    let (status, raw_body, _) = testkit::http_post(
        &format!("127.0.0.1:{listen_port}"),
        "/v1/chat/completions",
        STREAM_BODY.as_bytes(),
        &[],
    );
    let elapsed = t0.elapsed();
    serve_task.abort();
    let _ = serve_task.await;

    assert_eq!(status, 200, "the head was already committed");
    assert!(
        elapsed >= std::time::Duration::from_millis(IDLE_MS),
        "the gap bound really elapsed: {elapsed:?}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "the ATTEMPT knob (3s) did not fire — the gap bound did: {elapsed:?}"
    );
    let got = testkit::dechunk(&raw_body);
    assert_eq!(got, event, "the bytes that did arrive are relayed verbatim");
    assert!(
        !got.ends_with(b"data: [DONE]\n\n"),
        "a truncated stream never gains a fabricated terminal"
    );
    assert_eq!(upstream.requests().len(), 1, "no retry after the head");

    let records = read_records(&dir.join("state/traces"));
    assert_eq!(records.len(), 1, "one record for the one request");
    let e = truncation(&records[0]);
    assert_eq!(
        e["details"]["stream_end"], "idle_timeout",
        "the cause is named: {e:?}"
    );
    assert_eq!(
        e["details"]["window_ms"], IDLE_MS,
        "the window in force is the gap bound"
    );
    assert_eq!(
        e["details"]["relayed_bytes"],
        event.len(),
        "the count of bytes written through"
    );
    let since = e["details"]["since_last_byte_ms"]
        .as_u64()
        .expect("since_last_byte_ms is a count");
    assert!(
        since >= IDLE_MS - 50,
        "the interval from the last byte to the verdict is the bound: {since}"
    );
    assert_eq!(
        records[0]["result"]["status"], 200,
        "the client's status is untouched (spec §4.20.4)"
    );

    let events = read_events(&dir);
    let responded: Vec<&(String, Value, i64)> = events
        .iter()
        .filter(|(k, _, _)| k == "upstream.responded")
        .collect();
    assert_eq!(responded.len(), 1, "one terminal event");
    assert_eq!(responded[0].1["stream_completed"], false);
    assert_eq!(
        responded[0].1["stream_end"], "idle_timeout",
        "the event carries the same cause word"
    );
}

/// (b) A clean early end is a truncation with a *different* cause — and the
/// base tree calls it complete.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_108_b_clean_early_end_is_its_own_cause() {
    let dir = testkit::tempdir("conf108b");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    // One event, then the chunked body is terminated cleanly by the mock:
    // no terminal event, no transport error — an upstream that simply stopped.
    let event = b"data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\n";
    upstream.queue(testkit::CannedResponse::sse(vec![SseChunk::event(event)]));

    let listen_port = testkit::free_port();
    let config_path = dir.join("config.yaml");
    std::fs::write(
        &config_path,
        config_yaml(
            upstream.addr.port(),
            listen_port,
            &format!("{IDLE_MS}ms"),
            REQUEST,
        ),
    )
    .unwrap();
    let serve_task = start_vadis(&config_path, listen_port).await;

    let (status, raw_body, _) = testkit::http_post(
        &format!("127.0.0.1:{listen_port}"),
        "/v1/chat/completions",
        STREAM_BODY.as_bytes(),
        &[],
    );
    serve_task.abort();
    let _ = serve_task.await;

    assert_eq!(status, 200);
    assert_eq!(testkit::dechunk(&raw_body), event);

    let records = read_records(&dir.join("state/traces"));
    assert_eq!(records.len(), 1);
    let e = truncation(&records[0]);
    assert_eq!(
        e["details"]["stream_end"], "upstream_ended_early",
        "a clean early end is NOT silence: {e:?}"
    );
    assert!(
        e["details"].get("window_ms").is_none(),
        "no bound tripped, so no window is reported: {e:?}"
    );
    assert!(
        e["details"]["since_last_byte_ms"].is_u64(),
        "the interval is always present: {e:?}"
    );
    assert_eq!(
        e["details"]["relayed_bytes"],
        event.len(),
        "the bytes that did arrive are counted"
    );

    let events = read_events(&dir);
    let responded: Vec<&(String, Value, i64)> = events
        .iter()
        .filter(|(k, _, _)| k == "upstream.responded")
        .collect();
    assert_eq!(
        responded[0].1["stream_completed"], false,
        "the completeness predicate is the wire's terminal event, not the EOF"
    );
}

/// A listener that reads the request, waits `head_delay_ms`, then answers a
/// chunked SSE body with `events` (already framed) and terminates cleanly.
/// The canned mock cannot delay a *head*, so this limb needs its own rig.
async fn head_delayed_listener(head_delay_ms: u64, events: Vec<Vec<u8>>) -> u16 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            let events = events.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 8192];
                let _ =
                    tokio::time::timeout(std::time::Duration::from_secs(5), s.read(&mut buf)).await;
                tokio::time::sleep(std::time::Duration::from_millis(head_delay_ms)).await;
                let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                            transfer-encoding: chunked\r\nconnection: close\r\n\r\n";
                if s.write_all(head.as_bytes()).await.is_err() {
                    return;
                }
                let mut idle = [0u8; 64];
                for ev in &events {
                    if s.write_all(format!("{:x}\r\n", ev.len()).as_bytes())
                        .await
                        .is_err()
                        || s.write_all(ev).await.is_err()
                        || s.write_all(b"\r\n").await.is_err()
                    {
                        return;
                    }
                    // Small gaps between events: every one is under the 400 ms
                    // gap bound, so only the HEAD could ever trip it.
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_millis(50),
                        s.read(&mut idle),
                    )
                    .await;
                }
                let _ = s.write_all(b"0\r\n\r\n").await;
                let _ = s.shutdown().await;
            });
        }
    });
    port
}

/// A healthy stream is untouched — and the new key does not bound the
/// head.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_108_c_healthy_stream_and_head_are_unaffected() {
    let dir = testkit::tempdir("conf108c");
    // The head is written only after 800 ms: over the 400 ms gap bound (which
    // must NOT bound it) and under the 3 s attempt knob (which must).
    let mut events = Vec::new();
    let mut want = Vec::new();
    for i in 0..6 {
        let ev = format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"t{i}\"}}}}]}}\n\n");
        want.extend_from_slice(ev.as_bytes());
        events.push(ev.into_bytes());
    }
    want.extend_from_slice(b"data: [DONE]\n\n");
    events.push(b"data: [DONE]\n\n".to_vec());
    let upstream_port = head_delayed_listener(800, events).await;

    let listen_port = testkit::free_port();
    let config_path = dir.join("config.yaml");
    std::fs::write(
        &config_path,
        config_yaml(upstream_port, listen_port, &format!("{IDLE_MS}ms"), REQUEST),
    )
    .unwrap();
    let serve_task = start_vadis(&config_path, listen_port).await;

    let (status, raw_body, _) = testkit::http_post(
        &format!("127.0.0.1:{listen_port}"),
        "/v1/chat/completions",
        STREAM_BODY.as_bytes(),
        &[],
    );
    serve_task.abort();
    let _ = serve_task.await;

    assert_eq!(status, 200, "the slow head is inside the attempt knob");
    assert_eq!(
        testkit::dechunk(&raw_body),
        want,
        "byte-identical relay, [DONE] last"
    );

    let records = read_records(&dir.join("state/traces"));
    assert_eq!(records.len(), 1);
    let errors = records[0]["errors"].as_array().expect("errors array");
    assert!(
        errors.is_empty(),
        "a healthy stream carries no entry (and no stream_end): {errors:?}"
    );
    let events = read_events(&dir);
    let responded: Vec<&(String, Value, i64)> = events
        .iter()
        .filter(|(k, _, _)| k == "upstream.responded")
        .collect();
    assert_eq!(responded[0].1["stream_completed"], true);
    assert!(
        responded[0].1.get("stream_end").is_none(),
        "a complete stream carries no cause word"
    );
}

/// (d) Reload reaches the next request, never the one in flight.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_108_d_reload_applies_to_the_next_request_only() {
    let dir = testkit::tempdir("conf108d");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    // Two silent streams: one before the reload, one after.
    for _ in 0..2 {
        upstream.queue(testkit::CannedResponse::sse(vec![
            SseChunk::event(b"data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n"),
            SseChunk::event_after(b"data: never\n\n", 60_000),
        ]));
    }

    let listen_port = testkit::free_port();
    let addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(
        &config_path,
        config_yaml(upstream.addr.port(), listen_port, "4s", "8s"),
    )
    .unwrap();
    let serve_task = start_vadis(&config_path, listen_port).await;
    let digest_before = config_digest(&addr);

    // Request #1, in flight while the pair is rewritten.
    let addr1 = addr.clone();
    let first = std::thread::spawn(move || {
        let t0 = std::time::Instant::now();
        let (status, _body, _h) =
            testkit::http_post(&addr1, "/v1/chat/completions", STREAM_BODY.as_bytes(), &[]);
        (status, t0.elapsed())
    });

    // Give #1 time to enter the driver, then publish a narrower window.
    std::thread::sleep(std::time::Duration::from_millis(300));
    std::fs::write(
        &config_path,
        config_yaml(
            upstream.addr.port(),
            listen_port,
            &format!("{IDLE_MS}ms"),
            REQUEST,
        ),
    )
    .unwrap();

    // The reload is applied (not refused): the digest moves, no restart.
    let mut applied = false;
    for _ in 0..100 {
        if config_digest(&addr) != digest_before {
            applied = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(
        applied,
        "the new key is RELOADABLE (spec §4.15): the digest must move"
    );

    let (status1, elapsed1) = first.join().expect("request #1 finished");
    assert_eq!(status1, 200);

    // Request #2 rides the new revision.
    let t0 = std::time::Instant::now();
    let (status2, _b2, _h2) =
        testkit::http_post(&addr, "/v1/chat/completions", STREAM_BODY.as_bytes(), &[]);
    let elapsed2 = t0.elapsed();
    serve_task.abort();
    let _ = serve_task.await;

    assert_eq!(status2, 200);
    assert!(
        elapsed1 >= std::time::Duration::from_secs(4),
        "#1 kept the window it entered with (4s): {elapsed1:?}"
    );
    assert!(
        elapsed2 < std::time::Duration::from_secs(2),
        "#2 drives the new revision (400ms), not the old 4s: {elapsed2:?}"
    );

    let records = read_records(&dir.join("state/traces"));
    assert_eq!(records.len(), 2, "one record per request");
    let w1 = truncation(&records[0])["details"]["window_ms"]
        .as_u64()
        .expect("window_ms on #1");
    let w2 = truncation(&records[1])["details"]["window_ms"]
        .as_u64()
        .expect("window_ms on #2");
    assert_eq!(w1, 4_000, "the in-flight request kept its own window");
    assert_eq!(w2, IDLE_MS, "the next request drives the new window");
}
