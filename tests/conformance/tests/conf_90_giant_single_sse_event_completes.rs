//! CONF-90 (DESIGN §12.10.3 R1/R2 — the relay delivers the upstream's
//! bytes as they arrive; R4 — the idle bound is **not moved**): **a
//! single SSE event larger than 16 MiB is relayed byte-complete.**
//!
//! R51-2 measured the pre-existing defect this case pins (finding F1,
//! `autowork/harness/r51-2/logs/attack6-f1-curl-evidence.txt`): with the
//! usage tap rescanning its whole reassembly buffer on every chunk, one
//! oversized event made `feed()` quadratic in the buffered length, the
//! relay starved, and the client received a strict 16 MiB prefix before
//! the stream died at the 10s bound. The failure is specific to ONE
//! giant event — the same total delivered as many events streamed in
//! full — so limb (b) is the case's own control arm.
//!
//! Limbs:
//! - (a) **one event of >64 MiB** (payload `64 MiB + 1`, R51-2's fixture
//!   size): the client receives the upstream's exact byte sequence, the
//!   stream terminates normally (its record declares no error), and the
//!   transfer completes inside `server.upstream_attempt_timeout` — the
//!   defect's signature was death AT that bound. **Red before the fix**
//!   (R54-0's red control, `autowork/harness/r54-0/logs/`): the client
//!   received a strict 16 777 216-byte prefix at the bound.
//! - (b) **the same total as many 16 KiB events**: byte-complete, as it
//!   always was — the fix may not change the ordinary path.
//!
//! No network egress: a loopback mock upstream only; the cache capability
//! is not mounted (`plugins: []`) — the defect predates it.
//!
//! Registration note: this row is owed to DESIGN §12.8's table by the
//! round's contract side (the CONF-86 precedent — retroactive
//! registration by the registry's owner); R54-0's card forbids the fix
//! card from editing `design/`.
#![forbid(unsafe_code)]

use router_conformance::testkit::{self, SseChunk};
use serde_json::Value;

/// One event's `data:` payload: past the 64 MiB store bound and 4× the
/// observed 16 MiB truncation point — comfortably above any machine's
/// pre-fix crawl inside the 10s bound.
const GIANT_PAYLOAD: usize = (64 << 20) + 1;

fn giant_event() -> Vec<u8> {
    let mut v = Vec::with_capacity(6 + GIANT_PAYLOAD + 2);
    v.extend_from_slice(b"data: ");
    v.resize(6 + GIANT_PAYLOAD, b'x');
    v.extend_from_slice(b"\n\n");
    v
}

fn many_events() -> Vec<u8> {
    // 16 392 B per event, the same total payload class as limb (a).
    let mut ev = Vec::with_capacity(16392);
    ev.extend_from_slice(b"data: ");
    ev.resize(6 + 16384, b'y');
    ev.extend_from_slice(b"\n\n");
    let mut all = Vec::new();
    let mut sent = 0;
    while sent < GIANT_PAYLOAD {
        all.extend_from_slice(&ev);
        sent += ev.len();
    }
    all
}

fn config_yaml(upstream_port: u16, listen_port: u16) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 60s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: mock
    urls:
      chat: http://127.0.0.1:{upstream_port}/v1/chat/completions
    api_key_env: CONF90_MOCK_KEY
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

const CLIENT_BODY: &str = r#"{"model":"mock/glm","messages":[{"role":"user","content":"stream me"}],"stream":true,"stream_options":{"include_usage":true},"prompt_cache_key":"sess-90"}"#;

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

/// One arm: serve the given upstream event bytes as one stream, read the
/// client to EOF, and return (dechunked event bytes, elapsed, records).
async fn run_arm(tag: &str, events: Vec<u8>) -> (Vec<u8>, std::time::Duration, Vec<Value>) {
    let dir = testkit::tempdir(tag);
    let upstream = testkit::MockUpstream::start().await.unwrap();
    // The bytes travel as one canned chunk series; the mock may write the
    // giant event in one go — SSE events, not transport chunks, are the
    // unit under test.
    upstream.queue(testkit::CannedResponse::sse(vec![SseChunk::event(&events)]));

    let listen_port = testkit::free_port();
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(upstream.addr.port(), listen_port)).unwrap();
    std::env::set_var("CONF90_MOCK_KEY", "sk-conf90");
    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
    let listen_addr = format!("127.0.0.1:{listen_port}");
    testkit::wait_listening(&listen_addr);

    let t0 = std::time::Instant::now();
    let (status, raw_body, _) = testkit::http_post(
        &listen_addr,
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[],
    );
    let elapsed = t0.elapsed();
    assert_eq!(status, 200, "router status");

    let requests = upstream.requests();
    assert_eq!(requests.len(), 1, "exactly one upstream attempt");

    serve_task.abort();
    let _ = serve_task.await;

    let records = read_records(&dir.join("state").join("traces"));
    (testkit::dechunk(&raw_body), elapsed, records)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_90_giant_single_sse_event_completes() {
    // Limb (a): ONE event past 64 MiB, then [DONE].
    let mut want = giant_event();
    want.extend_from_slice(b"data: [DONE]\n\n");
    let (got, elapsed, records) = run_arm("conf90a", want.clone()).await;
    assert!(
        got == want,
        "the giant single event must arrive byte-complete: got {} of {} bytes",
        got.len(),
        want.len()
    );
    assert!(
        got.ends_with(b"data: [DONE]\n\n"),
        "the stream reached its terminal event (a truncated stream ends mid-event)"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "the transfer completes INSIDE the 10s bound — the defect's signature was death at it: {elapsed:?}"
    );
    // The stream ended on its own terms: its record declares no error
    // (the pre-fix record carried "stream truncated before its terminal
    // event").
    assert_eq!(records.len(), 1, "one record for the one request");
    assert!(
        records[0]
            .get("errors")
            .and_then(|e| e.as_array())
            .map(|a| a.is_empty())
            .unwrap_or(true),
        "the record declares no truncation: {}",
        records[0].get("errors").unwrap_or(&Value::Null)
    );

    // Limb (b): the same total as MANY events — the control that always
    // passed; the fix may not change it.
    let mut want_many = many_events();
    want_many.extend_from_slice(b"data: [DONE]\n\n");
    let (got_many, _, _) = run_arm("conf90b", want_many.clone()).await;
    assert!(
        got_many == want_many,
        "the many-events control stays byte-complete: got {} of {} bytes",
        got_many.len(),
        want_many.len()
    );
}
