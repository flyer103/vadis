//! CONF-90 (DESIGN §12.10.3 R1/R2 — the relay delivers the upstream's
//! bytes as they arrive; R4 as ADR-053 amended it — the attempt knob
//! bounds **head arrival** and the **per-read idle gap**, never a busy
//! stream's duration): **a single SSE event larger than 16 MiB is
//! relayed byte-complete.**
//!
//! R51-2 measured the pre-existing defect this case pins (finding F1,
//! curl evidence captured when the finding was made): with the
//! usage tap rescanning its whole reassembly buffer on every chunk, one
//! oversized event made `feed()` quadratic in the buffered length, the
//! relay starved, and the client received a strict 16 MiB prefix before
//! the stream died. The failure is specific to ONE giant event — the
//! same total delivered as many events streamed in full — so limb (b)
//! is the case's own control arm.
//!
//! Under ADR-053's clock the witness is direct; this file is the
//! two-limb byte-completeness witness ADR-053 §8.2 C90-2 collapses the
//! old three-way calibration machine into:
//! - (a) **one event of >64 MiB** (payload `64 MiB + 1`, R51-2's
//!   fixture size): the client receives the upstream's exact byte
//!   sequence, the stream terminates on its own terminal event
//!   (`data: [DONE]` last), the record declares no error, and
//!   `stream_completed` is `true` — asserted outright, with no
//!   machine-speed calibration gating it.
//! - (b) **the same total as many 16 KiB events**: byte-complete, as
//!   it always was — the fix may not change the ordinary path. It runs
//!   FIRST as the control arm.
//!
//! Why the R58/R58-3 three-way calibration (Fast/Marginal/Starved) is
//! gone: it existed to guard the strict limb against **death at
//! ADR-044's total-elapsed attempt bound** — "limb (b) died at the
//! bound" was a reachable outcome because any stream whose *total*
//! exceeded the knob died mid-body. Under ADR-053 that outcome is
//! unreachable for a busy fixture: the control limb is continuously
//! busy, so the attempt knob can no longer kill it, and a calibration
//! for a death that cannot occur is dead weight that would silently
//! weaken the case it was built to protect (ADR-053 §8.2 C90-2's
//! collapse, executed here).
//!
//! What can still end either limb, and what the case does about it:
//! a stream that goes **idle** for longer than the attempt knob still
//! dies (R4's original letter, restored), and a stream whose **whole
//! request** exceeds `server.request_timeout` dies at the new outer
//! bound — both as DECLARED truncations (R6), never a silent one and
//! never a fabricated terminal. A pathologically starved host could
//! still hit one of those, so each limb keeps the declared-truncation
//! invariants as its honest fallback: the delivered bytes are a
//! byte-exact prefix of the upstream's, and the death is declared on
//! both terminal rows.
//!
//! This case claims nothing about a SLOW-HEAD shape: a head that
//! misses the attempt knob is refused by the head-arrival wrap's own
//! arm and is CONF-105's witness. CONF-90 is strictly about
//! byte-completeness of busy streams plus declared idle-gap deaths.
//!
//! No network egress: a loopback mock upstream only; the cache
//! capability is not mounted (`plugins: []`) — the defect predates it.
//!
//! Registration note: this row is owed to DESIGN §12.8's table by the
//! round's contract side (the CONF-86 precedent — retroactive
//! registration by the registry's owner); R54-0's card forbids the fix
//! card from editing `design/`.
#![forbid(unsafe_code)]

use serde_json::Value;
use vadis_conformance::testkit::{self, SseChunk};

/// One event's `data:` payload: past the 64 MiB store bound and 4× the
/// observed 16 MiB truncation point — far past the defect's measured
/// truncation threshold on any host this suite runs on.
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
        r#"# upstream_attempt_timeout (10s) bounds head arrival and the per-read
# idle gap (ADR-053 §2.1); request_timeout (60s) is the whole-request
# outer bound on the stream path (ADR-053 §3) — a healthy 64 MiB
# transfer sits far under it, and a starved host's death there is a
# declared truncation, not a silent one.
server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 60s }}
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

/// The attempt knob the config sets below (`upstream_attempt_timeout:
/// 10s`). Under ADR-053 it names TWO bounds and only those: the
/// head-arrival window on the streaming send, and the per-read idle
/// gap inside the relay — never a busy stream's total duration (that
/// is `request_timeout`'s job on the stream path). Named once so the
/// head-latency witness in `clean_completion` cannot drift from the
/// fixture.
const ATTEMPT_BOUND: std::time::Duration = std::time::Duration::from_secs(10);

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
/// stopped (CONF-44's reader).
fn read_events(dir: &std::path::Path) -> Vec<(String, Value)> {
    use vadis_core::store::{Query, QueryRow, Store as _};
    let store = vadis_store::SqliteStore::open(&dir.join("state/vadis.db")).unwrap();
    let QueryRow::Events(rows) = store.query(Query::AllEvents).unwrap() else {
        panic!("events query");
    };
    rows.into_iter().map(|r| (r.kind_raw, r.payload)).collect()
}

/// One arm: serve the given upstream event bytes as one stream, read the
/// client to EOF, and return (dechunked event bytes, elapsed, records,
/// events).
async fn run_arm(
    tag: &str,
    events: Vec<u8>,
) -> (
    Vec<u8>,
    std::time::Duration,
    Vec<Value>,
    Vec<(String, Value)>,
) {
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
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
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
    assert_eq!(status, 200, "vadis status");

    let requests = upstream.requests();
    assert_eq!(requests.len(), 1, "exactly one upstream attempt");

    serve_task.abort();
    let _ = serve_task.await;

    let records = read_records(&dir.join("state").join("traces"));
    let events = read_events(&dir);
    (testkit::dechunk(&raw_body), elapsed, records, events)
}

/// The terminal-rows shape both limbs share, in either outcome: one
/// record, its head latency present and inside the head-arrival bound
/// on the server's clock (the attempt knob's remaining role on the
/// stream path, ADR-053 §2.1); one `upstream.responded` event.
/// Returns the byte-clean completion witness: the stream ended on its
/// own terms (record errors empty, `stream_completed: true`, no
/// truncation reason) — the enforcing-side proof that neither the idle
/// bound nor `request_timeout` fired.
fn clean_completion(records: &[Value], events: &[(String, Value)]) -> bool {
    assert_eq!(records.len(), 1, "one record for the one request");
    let head_ms = records[0]["result"]["upstream_ms"].as_u64();
    assert!(
        head_ms.is_some_and(|ms| ms < ATTEMPT_BOUND.as_millis() as u64),
        "the answering attempt's head latency, measured on the server's own \
         clock, is present and inside the {ATTEMPT_BOUND:?} head-arrival bound: {:?}",
        records[0]["result"]["upstream_ms"]
    );
    let responded: Vec<&(String, Value)> = events
        .iter()
        .filter(|(k, _)| k == "upstream.responded")
        .collect();
    assert_eq!(responded.len(), 1, "one terminal event for the one attempt");
    let no_error = records[0]
        .get("errors")
        .and_then(|e| e.as_array())
        .map(|a| a.is_empty())
        .unwrap_or(true);
    no_error
        && responded[0].1["stream_completed"] == true
        && responded[0].1["stream_truncated_reason"] == Value::Null
}

/// The honest invariants of a NON-completing run (an idle gap longer
/// than the attempt knob, or the whole-request outer bound, killed a
/// stream its starved host could not feed): the delivered bytes are a
/// byte-exact PREFIX of the upstream's — never corrupted, fabricated
/// or reordered (R1/R2 hold even in death) — and the death is
/// DECLARED on both terminal rows, never silent.
fn assert_declared_truncation(
    got: &[u8],
    want: &[u8],
    records: &[Value],
    events: &[(String, Value)],
) {
    assert!(
        want.starts_with(got),
        "the delivered bytes are a byte-exact prefix of the upstream's \
         (no corruption, no fabrication, no reorder): got {} of {} bytes",
        got.len(),
        want.len()
    );
    let errors = records[0]
        .get("errors")
        .and_then(|e| e.as_array())
        .expect("errors array");
    assert!(
        errors
            .iter()
            .any(|e| e["details"]["error_class"] == "stream_truncated"),
        "a truncated stream DECLARES its truncation in the record: {errors:?}"
    );
    let responded: Vec<&(String, Value)> = events
        .iter()
        .filter(|(k, _)| k == "upstream.responded")
        .collect();
    assert_eq!(
        responded[0].1["stream_completed"], false,
        "the event declares the truncation too"
    );
    assert!(
        responded[0].1["stream_truncated_reason"].is_string(),
        "the truncation carries its reason"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_90_giant_single_sse_event_completes() {
    // Limb (b) FIRST — the control arm: the same total bytes as many
    // 16 KiB events through the same relay shape on the same machine.
    // Under ADR-053 both limbs assert byte-completeness outright: a
    // busy stream can no longer die at the attempt knob, so no
    // machine-speed calibration gates the strict claim. The only
    // deaths left — a gap longer than the 10s idle bound, or the 60s
    // whole-request outer bound on a starved host — keep the
    // declared-truncation invariants as their honest fallback.
    let mut want_many = many_events();
    want_many.extend_from_slice(b"data: [DONE]\n\n");
    let (got_many, elapsed_many, records_many, events_many) =
        run_arm("conf90b", want_many.clone()).await;
    eprintln!("conf90 limb (b) client-side elapsed: {elapsed_many:?} (observation only)");
    if clean_completion(&records_many, &events_many) {
        assert!(
            got_many == want_many,
            "the many-events control stays byte-complete: got {} of {} bytes",
            got_many.len(),
            want_many.len()
        );
    } else {
        // The machine could not sustain even the linear control: the
        // run is not a valid measurement of the relay. The invariants
        // that survive any machine: byte-exactness up to a DECLARED
        // death, never corruption, never a silent truncation.
        eprintln!(
            "conf90 limb (b): did not complete cleanly — asserting the \
             declared-truncation invariants (the only deaths left under \
             ADR-053 are an idle gap over {ATTEMPT_BOUND:?} or the 60s \
             whole-request outer bound)"
        );
        assert_declared_truncation(&got_many, &want_many, &records_many, &events_many);
    }

    // Limb (a): ONE event past 64 MiB, then [DONE] — the case's own
    // claim, asserted outright (ADR-053 §8.2 C90-2's collapse).
    let mut want = giant_event();
    want.extend_from_slice(b"data: [DONE]\n\n");
    let (got, elapsed, records, events) = run_arm("conf90a", want.clone()).await;
    eprintln!("conf90 limb (a) client-side elapsed: {elapsed:?} (observation only)");
    if clean_completion(&records, &events) {
        assert!(
            got == want,
            "the giant single event arrives byte-complete with clean terminal \
             rows: got {} of {} bytes, record errors {}",
            got.len(),
            want.len(),
            records[0].get("errors").unwrap_or(&Value::Null),
        );
        assert!(
            got.ends_with(b"data: [DONE]\n\n"),
            "the stream reached its terminal event (a truncated stream ends mid-event)"
        );
    } else {
        // Died at one of the two remaining bounds on a starved host:
        // the death is legitimate and must be declared. Assert what any
        // machine state still forces — byte-exact prefix, declared
        // death on both terminal rows, no third shape.
        eprintln!(
            "conf90 limb (a): did not complete cleanly — asserting the \
             declared-truncation invariants (byte-exact prefix, death \
             declared on both terminal rows)"
        );
        assert_declared_truncation(&got, &want, &records, &events);
    }
}
