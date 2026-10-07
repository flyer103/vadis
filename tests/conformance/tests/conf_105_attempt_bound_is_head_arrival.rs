//! CONF-105 (DESIGN §12.8, ADR-053 — the attempt bound is
//! **head-arrival-only**; §4.2's classification evidence): **the two
//! bounds, and the fault the two media share.**
//!
//! `server.upstream_attempt_timeout` bounds how long the upstream may
//! take to produce a response HEAD (ADR-053 §2.1) and the per-read GAP
//! between body bytes (R4's original letter, restored); a stream that
//! never idles is bounded in duration only by
//! `server.request_timeout` (§3), now enforced on the stream path by
//! a single owner. This case witnesses the split through the real
//! `serve` assembly against loopback mocks, with the attempt knob at
//! **2s** and the whole-request bound at **8s** (the class values; all
//! rig delays sit in the hundreds of ms, so no limb rests on an
//! assumption about machine speed).
//!
//! Limbs:
//! - (a) **the busy stream — the change's own claim**: a stream whose
//!   TOTAL elapsed (2.7s) exceeds the attempt knob (2s) while EVERY
//!   inter-byte gap (300ms) stays under it is relayed byte-complete
//!   with its terminal event (`stream_completed: true`, `errors[]`
//!   empty, `[DONE]` last). **Red at the base** (ADR-044's
//!   total-elapsed clock truncated exactly this rig mid-body).
//! - (b) **the slow head — the conservative arm, and the cross-media
//!   class parity** (R71-0b limb-1 FAIL (d), qa's repair option 1 as
//!   the orchestrator recorded it): PARKED `#[ignore]` pending the
//!   owner's ruling on the buffered leg's rig. The streamed leg rigs
//!   a head that never arrives within the knob and expects the
//!   `unknown_outcome` refusal (502 / `upstream_error` / `stage:
//!   "unknown_outcome"` / `error_class: "timeout"`, no retry, no
//!   failover, exactly one record); the buffered leg rigs a
//!   MID-BODY STALL (head arrives, body stalls past the knob) and
//!   asserts the same class word and stage in the buffered path's own
//!   carriage. The shared-class-word assertion (§2.3's invariant) is
//!   what the limb exists for.
//! - (c) **the connect window stays classified**: a host that REFUSES
//!   is still `connect_failure` with its failover-eligible action,
//!   never `timeout`/`unknown_outcome` — the new Elapsed arm cannot
//!   absorb a fast refusal (CONF-29's shape, on the stream medium).
//! - (d) **the idle control, unchanged**: a stream that goes SILENT
//!   for longer than the knob mid-body is a DECLARED truncation
//!   (`stream_completed: false`, a `stream_truncated_reason` string,
//!   `error_class: "stream_truncated"`, no `[DONE]`) — the gap bound
//!   R4 always had stays asserted.
//!
//! No network egress: loopback mocks only; the cache capability is
//! not mounted (`plugins: []`).
#![forbid(unsafe_code)]

use serde_json::Value;
use vadis_conformance::testkit::{self, SseChunk};

/// The attempt knob (ADR-053 §2.1): bounds head arrival and the
/// per-read gap — never a busy stream's duration.
const ATTEMPT_BOUND: std::time::Duration = std::time::Duration::from_secs(2);
/// The whole-request bound (ADR-053 §3): the stream path's outer
/// limit, enforced by the single owner in the streaming driver.
const REQUEST_BOUND: std::time::Duration = std::time::Duration::from_secs(8);
/// Every inter-byte gap in the busy fixture — comfortably under the
/// knob (≈6× margin), so limb (a) never rests on machine speed.
const BUSY_GAP: u64 = 300;

fn config_yaml(upstream_port: u16, listen_port: u16) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 2s, request_timeout: 8s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: mock
    urls:
      chat: http://127.0.0.1:{upstream_port}/v1/chat/completions
    api_key_env: CONF105_MOCK_KEY
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

const STREAM_BODY: &str = r#"{"model":"mock/glm","messages":[{"role":"user","content":"stream me"}],"stream":true,"prompt_cache_key":"conf105-sess"}"#;
const BUFFERED_BODY: &str = r#"{"model":"mock/glm","messages":[{"role":"user","content":"hi"}],"prompt_cache_key":"conf105-sess"}"#;

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

/// Every stored event as (kind_raw, payload), read after the server
/// stopped.
fn read_events(dir: &std::path::Path) -> Vec<(String, Value)> {
    use vadis_core::store::{Query, QueryRow, Store as _};
    let store = vadis_store::SqliteStore::open(&dir.join("state/vadis.db")).unwrap();
    let QueryRow::Events(rows) = store.query(Query::AllEvents).unwrap() else {
        panic!("events query");
    };
    rows.into_iter().map(|r| (r.kind_raw, r.payload)).collect()
}

async fn start_vadis(
    config_path: &std::path::Path,
    listen_port: u16,
) -> tokio::task::JoinHandle<()> {
    std::env::set_var("CONF105_MOCK_KEY", "sk-conf105");
    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move {
        let _ = vadis_cli::serve(&cfg).await;
    });
    testkit::wait_listening(&format!("127.0.0.1:{listen_port}"));
    serve_task
}

/// (a) The busy stream: total 2.7s > the 2s knob, every gap 300ms <
/// the knob — byte-complete, clean terminal rows. Red at the base
/// (the total-elapsed clock truncated this exact rig at 2s).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_105_a_busy_stream_completes_byte_clean() {
    let dir = testkit::tempdir("conf105a");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    // 9 events × 300ms = 2.7s total > ATTEMPT_BOUND; each gap 300ms
    // ≪ ATTEMPT_BOUND. The last event is the protocol's terminal.
    let mut chunks = Vec::new();
    let mut want = Vec::new();
    for i in 0..8 {
        let ev = format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"t{i}\"}}}}]}}\n\n");
        want.extend_from_slice(ev.as_bytes());
        chunks.push(SseChunk::event_after(ev.as_bytes(), BUSY_GAP));
    }
    want.extend_from_slice(b"data: [DONE]\n\n");
    chunks.push(SseChunk::event_after(b"data: [DONE]\n\n", BUSY_GAP));
    upstream.queue(testkit::CannedResponse::sse(chunks));

    let listen_port = testkit::free_port();
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(upstream.addr.port(), listen_port)).unwrap();
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

    assert_eq!(status, 200, "a busy stream is served, not refused");
    assert!(
        elapsed >= std::time::Duration::from_millis(2_400),
        "the rig really ran past the knob: {elapsed:?}"
    );
    let got = testkit::dechunk(&raw_body);
    assert_eq!(
        got,
        want,
        "byte-complete: the client receives the upstream's exact byte \
         sequence, [DONE] last (got {} of {} bytes)",
        got.len(),
        want.len()
    );
    assert!(got.ends_with(b"data: [DONE]\n\n"));

    // Exactly one attempt — no retry, no failover.
    assert_eq!(upstream.requests().len(), 1, "one upstream attempt");

    // Clean terminal rows: one record, errors[] empty; one
    // upstream.responded with stream_completed true and no truncation.
    let records = read_records(&dir.join("state/traces"));
    assert_eq!(records.len(), 1, "one record for the one request");
    let errors = records[0]
        .get("errors")
        .and_then(|e| e.as_array())
        .expect("errors array");
    assert!(
        errors.is_empty(),
        "a clean completion has no errors: {errors:?}"
    );
    let events = read_events(&dir);
    let responded: Vec<&(String, Value)> = events
        .iter()
        .filter(|(k, _)| k == "upstream.responded")
        .collect();
    assert_eq!(responded.len(), 1, "one terminal event");
    assert_eq!(responded[0].1["stream_completed"], true);
    assert_eq!(responded[0].1["stream_truncated_reason"], Value::Null);
}

/// (c) The connect window: a host that REFUSES stays
/// `connect_failure` (failover-eligible, CONF-29's shape) — the new
/// head-arrival Elapsed arm cannot absorb a fast refusal.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_105_c_refusing_host_stays_connect_failure() {
    let dir = testkit::tempdir("conf105c");
    // A port with no listener: bind then drop, so it is genuinely closed.
    let dead_port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let p = l.local_addr().unwrap().port();
        drop(l);
        p
    };
    let listen_port = testkit::free_port();
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(dead_port, listen_port)).unwrap();
    let serve_task = start_vadis(&config_path, listen_port).await;

    let (status, body, _) = testkit::http_post(
        &format!("127.0.0.1:{listen_port}"),
        "/v1/chat/completions",
        STREAM_BODY.as_bytes(),
        &[],
    );
    serve_task.abort();
    let _ = serve_task.await;

    assert_eq!(status, 502, "connect refused with an empty chain is a 502");
    let v: Value = serde_json::from_slice(&body).expect("error body is json");
    let d = &v["error"]["details"];
    assert_eq!(
        d["error_class"], "connect_failure",
        "a fast refusal is a classified connect failure, never a timeout: {body:?}"
    );
    assert_ne!(
        d["stage"], "unknown_outcome",
        "the new Elapsed arm must not absorb a fast refusal: {body:?}"
    );

    // The books agree: one record, connect_failure in errors[].
    let records = read_records(&dir.join("state/traces"));
    assert_eq!(records.len(), 1, "one record for the request");
    let errors = records[0]["errors"].as_array().expect("errors array");
    assert_eq!(errors[0]["details"]["error_class"], "connect_failure");
}

/// (d) The idle control: a mid-body gap LONGER than the knob is a
/// declared truncation — R4's original letter, restored and unmoved.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_105_d_idle_gap_still_truncates_declared() {
    let dir = testkit::tempdir("conf105d");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    // First event immediately; the next only after 3s — past the 2s
    // knob, well under the 8s whole-request bound, so it is the IDLE
    // arm that fires, not the outer bound.
    upstream.queue(testkit::CannedResponse::sse(vec![
        SseChunk::event(b"data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\n"),
        SseChunk::event_after(
            b"data: {\"choices\":[{\"delta\":{\"content\":\"llo\"}}]}\n\n",
            (ATTEMPT_BOUND.as_millis() as u64) + 1_000,
        ),
    ]));

    let listen_port = testkit::free_port();
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(upstream.addr.port(), listen_port)).unwrap();
    let serve_task = start_vadis(&config_path, listen_port).await;

    let (status, raw_body, _) = testkit::http_post(
        &format!("127.0.0.1:{listen_port}"),
        "/v1/chat/completions",
        STREAM_BODY.as_bytes(),
        &[],
    );
    serve_task.abort();
    let _ = serve_task.await;

    assert_eq!(status, 200, "the head was already committed");
    let got = testkit::dechunk(&raw_body);
    assert!(
        !got.ends_with(b"data: [DONE]\n\n"),
        "a truncated stream never gains a fabricated terminal"
    );
    assert_eq!(upstream.requests().len(), 1, "no retry after the head");

    let records = read_records(&dir.join("state/traces"));
    assert_eq!(records.len(), 1, "the truncated stream still left its line");
    let errors = records[0]["errors"].as_array().expect("errors array");
    assert!(
        errors
            .iter()
            .any(|e| e["details"]["error_class"] == "stream_truncated"),
        "the idle trip is a DECLARED truncation: {errors:?}"
    );
    let events = read_events(&dir);
    let responded: Vec<&(String, Value)> = events
        .iter()
        .filter(|(k, _)| k == "upstream.responded")
        .collect();
    assert_eq!(responded.len(), 1, "one terminal event");
    assert_eq!(responded[0].1["stream_completed"], false);
    assert!(
        responded[0].1["stream_truncated_reason"].is_string(),
        "the event carries a truncation reason string"
    );
}

/// A raw listener that accepts, reads the request, and never answers —
/// the slow-head fixture (the testkit's canned responses cannot delay
/// a head). Counts the connections it served.
async fn slow_head_listener() -> (u16, std::sync::Arc<std::sync::Mutex<u32>>) {
    use tokio::io::AsyncReadExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(0u32));
    let seen_clone = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            *seen_clone.lock().unwrap() += 1;
            tokio::spawn(async move {
                let mut buf = [0u8; 8192];
                // Hold the connection open, draining quietly.
                loop {
                    match tokio::time::timeout(std::time::Duration::from_secs(5), s.read(&mut buf))
                        .await
                    {
                        Ok(Ok(0)) | Err(_) | Ok(Err(_)) => return,
                        Ok(Ok(_)) => {}
                    }
                }
            });
        }
    });
    (port, seen)
}

/// A raw listener that answers the head plus a PARTIAL body, then
/// stalls — the buffered path's mid-body-stall fixture (the body read
/// inside `send` never finishes).
async fn midbody_stall_listener() -> u16 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let _ =
                    tokio::time::timeout(std::time::Duration::from_secs(2), s.read(&mut buf)).await;
                // content-length promises 100 bytes; only 10 arrive,
                // then the connection stalls (held open).
                let head = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 100\r\nconnection: close\r\n\r\n";
                if s.write_all(head.as_bytes()).await.is_err() {
                    return;
                }
                if s.write_all(&[b'x'; 10]).await.is_err() {
                    return;
                }
                let mut idle = [0u8; 64];
                let _ = tokio::time::timeout(std::time::Duration::from_secs(15), s.read(&mut idle))
                    .await;
            });
        }
    });
    port
}

/// (b) The slow head — the conservative arm, and the cross-media class
/// parity (qa repair option 1, as the orchestrator recorded it).
///
/// PARKED pending the owner's ruling (R71-0b limb-1 FAIL (d)): the
/// buffered leg's rig. Authored so the ruling only flips this ignore
/// off:
/// - streamed leg: a head that never arrives within the knob → the
///   `unknown_outcome` refusal — 502 / `upstream_error` / `stage:
///   "unknown_outcome"` / `error_class: "timeout"`, no retry, no
///   `failover.triggered`, exactly one record;
/// - buffered leg: a MID-BODY STALL (head arrives, body stalls past
///   the knob) → `WrittenNoResponse` — the same `stage:
///   "unknown_outcome"` and `error_class: "timeout"` class word in
///   the buffered medium's own carriage;
/// - the shared-class-word assertion (ADR-053 §2.3's invariant) is
///   the limb's point: one fault, one class, both media.
#[ignore = "CONF-105 limb (b): buffered-leg rig re-scope awaits owner ruling — R71-0b limb-1 FAIL (d), options in comment 631"]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_105_b_slow_head_is_conservative_unknown_outcome_on_both_media() {
    // -- streamed leg: the head never arrives within the knob ----------
    let dir = testkit::tempdir("conf105b-stream");
    let (slow_port, seen) = slow_head_listener().await;
    let listen_port = testkit::free_port();
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(slow_port, listen_port)).unwrap();
    let serve_task = start_vadis(&config_path, listen_port).await;

    let t0 = std::time::Instant::now();
    let (status, body, _) = testkit::http_post(
        &format!("127.0.0.1:{listen_port}"),
        "/v1/chat/completions",
        STREAM_BODY.as_bytes(),
        &[],
    );
    let streamed_elapsed = t0.elapsed();
    serve_task.abort();
    let _ = serve_task.await;

    assert_eq!(status, 502, "the streamed slow head is a 502 refusal");
    assert!(
        streamed_elapsed >= ATTEMPT_BOUND,
        "the head window really expired: {streamed_elapsed:?}"
    );
    assert!(
        streamed_elapsed < REQUEST_BOUND,
        "the attempt knob fired, not the whole-request bound: {streamed_elapsed:?}"
    );
    let v: Value = serde_json::from_slice(&body).expect("error body is json");
    let d = &v["error"]["details"];
    assert_eq!(d["stage"], "unknown_outcome", "streamed: {body:?}");
    assert_eq!(d["error_class"], "timeout", "streamed: {body:?}");

    // No retry, no failover: exactly one connection, one record.
    assert_eq!(*seen.lock().unwrap(), 1, "exactly one upstream attempt");
    let records = read_records(&dir.join("state/traces"));
    assert_eq!(records.len(), 1, "exactly one record");
    let events = read_events(&dir);
    assert!(
        !events.iter().any(|(k, _)| k == "failover.triggered"),
        "the conservative arm never fails over"
    );

    // -- buffered leg: the head arrives, the body stalls past the knob -
    let dir2 = testkit::tempdir("conf105b-buffered");
    let stall_port = midbody_stall_listener().await;
    let listen2 = testkit::free_port();
    let config2 = dir2.join("config.yaml");
    std::fs::write(&config2, config_yaml(stall_port, listen2)).unwrap();
    let serve2 = start_vadis(&config2, listen2).await;

    let (status2, body2, _) = testkit::http_post(
        &format!("127.0.0.1:{listen2}"),
        "/v1/chat/completions",
        BUFFERED_BODY.as_bytes(),
        &[],
    );
    serve2.abort();
    let _ = serve2.await;

    // OBSERVED (executed against this rig, R71-1): the buffered
    // mid-body stall answers 502 `upstream_error` with the shared
    // stage/class — NOT qa's predicted 504 `upstream_timeout`. The
    // 504 arm's `timed_out` branch requires `kind == Timeout`, but
    // the buffered body-read failure arm hardcodes
    // `TransportKind::Other` (vadis-providers/src/lib.rs:186-189),
    // so this fault cannot reach it. The status carriage is in fact
    // the SAME (502) on both media for this rig; the class word and
    // stage — the invariant §2.3 names — are what is shared and
    // asserted. Flagged for the owner's limb-(b) ruling.
    assert_eq!(
        status2, 502,
        "the buffered mid-body stall's observed carriage: {body2:?}"
    );
    let v2: Value = serde_json::from_slice(&body2).expect("error body is json");
    let d2 = &v2["error"]["details"];
    // The shared class word (ADR-053 §2.3): one fault, one class, on
    // both media. The buffered medium's own status carriage is
    // asserted alongside — the per-path shape §2.2's table records.
    assert_eq!(d2["stage"], "unknown_outcome", "buffered: {body2:?}");
    assert_eq!(
        d2["error_class"], "timeout",
        "the class word is shared across both media: {body2:?}"
    );
    let records2 = read_records(&dir2.join("state/traces"));
    assert_eq!(records2.len(), 1, "exactly one record on the buffered leg");
}
