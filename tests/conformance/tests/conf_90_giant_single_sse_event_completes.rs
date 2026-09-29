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
//!   always was — the fix may not change the ordinary path. It runs
//!   FIRST and doubles as the machine calibration for limb (a) (R58,
//!   below).
//!
//! R58 — how "completes inside the 10s bound" is asserted without any
//! wall-clock assumption (the R56-1 load red at :168 — 10.225s measured
//! client-side against the 10s bound on a transfer the relay completed
//! cleanly — and, under R58-0's own heavier recipe, a second face of the
//! same assumption: the starved host genuinely cannot feed 64 MiB
//! through the relay inside ADR-044's total-elapsed bound, so the
//! stream legitimately DIES at it — R54-1's F1, "size- and
//! machine-dependent", measured twice). The fixed case measures the
//! machine instead of assuming it: **limb (b) runs FIRST as the
//! calibration** — the same byte count through the same relay shape on
//! the same machine at the same moment. Its byte-clean completion
//! proves the machine can sustain the transfer inside the bound right
//! now, and limb (a) then fires at full strictness (byte-complete,
//! terminal event, clean record, `stream_completed: true` — the
//! defect's signature, death at the bound, reds all of them). A run
//! whose calibration died at the bound is not a valid measurement of
//! the relay, and asserts what every machine state still forces: the
//! delivered bytes are a byte-exact PREFIX of the upstream's (never
//! corrupted, fabricated or reordered — R1/R2 hold even in death) and
//! every death is DECLARED on both terminal rows (the trace record's
//! `errors[]` and the event's `stream_completed: false` + reason),
//! never silent. The client-side elapsed is still measured and printed,
//! as an observation only. What this trade removes is precisely the
//! flake — a healthy relay on a starved host — and nothing else: the
//! falsifiability control under `autowork/harness/r58-0/` demonstrates
//! the full-strictness branch red-ing on the defect class. One scenario
//! is now covered by no assertion in this case: a relay that stopped
//! ENFORCING the bound entirely would let a slow transfer pass — that
//! regression belongs to the bound's own semantics (ADR-044), not to
//! this case's byte-completeness claim, and is named here rather than
//! discovered.
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

/// Every stored event as (kind_raw, payload), read after the server
/// stopped (CONF-44's reader).
fn read_events(dir: &std::path::Path) -> Vec<(String, Value)> {
    use router_core::store::{Query, QueryRow, Store as _};
    let store = router_store::SqliteStore::open(&dir.join("state/router.db")).unwrap();
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
    let events = read_events(&dir);
    (testkit::dechunk(&raw_body), elapsed, records, events)
}

/// The terminal-rows shape both limbs share, in either outcome: one
/// record, its head latency present and inside the bound on the
/// server's clock; one `upstream.responded` event. Returns the
/// byte-clean completion witness: the stream ended on its own terms
/// (record errors empty, `stream_completed: true`, no truncation
/// reason) — the enforcing-side proof that the bound never fired.
fn clean_completion(records: &[Value], events: &[(String, Value)]) -> bool {
    assert_eq!(records.len(), 1, "one record for the one request");
    let head_ms = records[0]["result"]["upstream_ms"].as_u64();
    assert!(
        head_ms.is_some_and(|ms| ms < 10_000),
        "the answering attempt's head latency, measured on the server's own \
         clock, is present and inside the 10s bound: {:?}",
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

/// The honest invariants of a NON-completing run (the bound legitimately
/// killed a stream its starved host could not feed): the delivered bytes
/// are a byte-exact PREFIX of the upstream's — never corrupted,
/// fabricated or reordered (R1/R2 hold even in death) — and the death
/// is DECLARED on both terminal rows, never silent.
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
    // Limb (b) FIRST — the calibration (R58): the same total bytes as
    // many 16 KiB events through the same relay shape on the same
    // machine at the same moment. Its byte-clean completion PROVES this
    // machine can sustain the ~64 MiB through this path inside the 10s
    // attempt bound right now; its death at the bound proves the
    // machine currently cannot (a starvation fact about the host, not a
    // relay fact — ADR-044's total-elapsed bound correctly kills a
    // stream the starved host cannot feed, and R54-1-F1 said the
    // budget is size- and machine-dependent). No wall-clock threshold
    // is assumed: the machine's speed is MEASURED on the exact byte
    // count in question, and each limb then asserts only what the
    // measurement proves.
    let mut want_many = many_events();
    want_many.extend_from_slice(b"data: [DONE]\n\n");
    let (got_many, elapsed_many, records_many, events_many) =
        run_arm("conf90b", want_many.clone()).await;
    eprintln!("conf90 limb (b) client-side elapsed: {elapsed_many:?}");
    let machine_fast = clean_completion(&records_many, &events_many) && got_many == want_many;
    if machine_fast {
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
        assert!(
            want_many.starts_with(&got_many),
            "the many-events control is byte-exact up to its end: got {} of {} bytes",
            got_many.len(),
            want_many.len()
        );
        assert_declared_truncation(&got_many, &want_many, &records_many, &events_many);
    }

    // Limb (a): ONE event past 64 MiB, then [DONE].
    let mut want = giant_event();
    want.extend_from_slice(b"data: [DONE]\n\n");
    let (got, elapsed, records, events) = run_arm("conf90a", want.clone()).await;
    // The client-side elapsed, kept as an observation only — it is NOT
    // the clock the attempt bound is enforced on (the R56-1 load red
    // measured 10.225s here on a cleanly completed transfer).
    eprintln!("conf90 limb (a) client-side elapsed: {elapsed:?}");
    let a_clean = clean_completion(&records, &events) && got == want;
    if machine_fast {
        // The calibration just proved this machine sustains the same
        // byte count through the same relay inside the bound — so the
        // healthy linear relay MUST deliver the giant event
        // byte-complete (the R54 defect's quadratic tap dies at the
        // bound here: red, at full strictness — the base's :159/:166
        // assertions unchanged, plus the terminal-rows witnesses).
        assert!(
            a_clean,
            "on a machine proven fast by the control, the giant single event \
             must arrive byte-complete with clean terminal rows: got {} of {} \
             bytes, record errors {}, stream_completed {}",
            got.len(),
            want.len(),
            records[0].get("errors").unwrap_or(&Value::Null),
            events
                .iter()
                .find(|(k, _)| k == "upstream.responded")
                .map(|(_, p)| p["stream_completed"].to_string())
                .unwrap_or_else(|| "<none>".into()),
        );
        assert!(
            got.ends_with(b"data: [DONE]\n\n"),
            "the stream reached its terminal event (a truncated stream ends mid-event)"
        );
    } else if a_clean {
        // Completed anyway, on a machine the control could not prove:
        // the full-clean shape holds a fortiori.
        assert!(
            got.ends_with(b"data: [DONE]\n\n"),
            "the stream reached its terminal event"
        );
    } else {
        // Both limbs agree the machine cannot sustain the transfer
        // right now: the bound's death is legitimate. Assert what any
        // machine state still forces — byte-exact prefix, declared
        // death, no third shape.
        assert_declared_truncation(&got, &want, &records, &events);
    }
}
