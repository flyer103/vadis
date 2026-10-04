//! CONF-80 (spec §6 `route_changed` + spec §4.5's TTL unit rule; the
//! R27-1 freeze, note R6): **the binding's move arm is a measured value,
//! and the session TTL crosses ms → µs exactly once.**
//!
//! - `session.bound` is written when the binding is created **or moved**
//!   (DESIGN §12.10.5 row 4): a session whose client `model` string moves
//!   from `p1/m-x` to `p2/m-x` writes exactly one further row naming
//!   `p2/m-x`, that row moves the `sessions` projection, and its
//!   `turn_index` advances; a sticky hit on an **unchanged** route writes
//!   nothing (the R21/`CONF-66` arm that must not regress).
//! - The configured `session.ttl` (a `DurationVal` in **milliseconds**)
//!   reaches the store as **microseconds**: `ttl_us == configured_ms ×
//!   1_000`, once, at the resolution site — never × 1_000_000 (R21-F6:
//!   the shipped v0.1 did, so a configured `60s` lived ~16.7 h).
//!
//! Every leg observes the SHIPPED path — a real `serve` assembly over
//! loopback mocks — and the move legs run on **both media**, compared
//! element for element (the CONF-66 discipline: one value, both media).
//! At the pre-fix tree (`b9fd007`) legs (a)–(b) are red: the TTL read
//! `60_000_000_000` and a moved session wrote no second row (the
//! `route_changed` literal `false`); the unchanged-route control is
//! green on both trees.

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, CannedResponse, MockUpstream, SseChunk};
use vadis_core::store::{Query, QueryRow, Store as _};

/// One leg's fixture: two plain `api` providers, `m-x` on each, **no**
/// plan policy — the move is client-named (`p1/m-x` → `p2/m-x`), the
/// simplest arm of the rule. The TTL knob is the leg's own.
fn config_yaml(p1_port: u16, p2_port: u16, listen_port: u16, ttl: &str) -> String {
    let model = |src: &str| {
        format!(
            r#"      - id: m-x
        context: 128k
        price:
          input_miss: 0.001
          input_hit: 0.0001
          cache_write: 0.0
          output: 0.002
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "{src}""#
        )
    };
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: {ttl} }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: p1
    urls:
      chat: http://127.0.0.1:{p1_port}/v1/chat/completions
    api_key_env: CONF80_P1_KEY
    wire_api: chat
    supports: [chat]
    models:
{}
  - name: p2
    urls:
      chat: http://127.0.0.1:{p2_port}/v1/chat/completions
    api_key_env: CONF80_P2_KEY
    wire_api: chat
    supports: [chat]
    models:
{}

aliases: {{}}
plugins: []
fallback: []
"#,
        model("mock upstream (CONF-80 fixture)"),
        model("mock upstream (CONF-80 fixture)"),
    )
}

/// A running two-provider rig with the real `serve` assembly (the
/// CONF-25 precedent: `vadis-cli` is a dev-dependency, the spawn stays
/// in the test file).
struct Rig {
    p1: MockUpstream,
    p2: MockUpstream,
    listen_addr: String,
    dir: std::path::PathBuf,
    serve_task: tokio::task::JoinHandle<i32>,
}

async fn start_rig(tag: &str, ttl: &str) -> Rig {
    let dir = testkit::tempdir(tag);
    let p1 = MockUpstream::start().await.unwrap();
    let p2 = MockUpstream::start().await.unwrap();
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    std::fs::write(
        dir.join("config.yaml"),
        config_yaml(p1.addr.port(), p2.addr.port(), listen_port, ttl),
    )
    .unwrap();
    std::env::set_var("CONF80_P1_KEY", "sk-conf80-p1");
    std::env::set_var("CONF80_P2_KEY", "sk-conf80-p2");
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    Rig {
        p1,
        p2,
        listen_addr,
        dir,
        serve_task,
    }
}

impl Rig {
    fn stop(self) -> std::path::PathBuf {
        let dir = self.dir.clone();
        self.serve_task.abort();
        drop(self.serve_task);
        dir
    }
}

/// One buffered chat request for the named route, with a session key.
fn post(rig: &Rig, model: &str, session: &str, turn: u32) -> u16 {
    let body = format!(
        r#"{{"model":"{model}","messages":[{{"role":"user","content":"turn {turn}"}}],"prompt_cache_key":"{session}","stream":false}}"#
    );
    let (status, _body, _h) = testkit::http_post(
        &rig.listen_addr,
        "/v1/chat/completions",
        body.as_bytes(),
        &[],
    );
    status
}

/// One streamed chat request for the named route (CONF-30's carrier
/// shape: the final data event carries usage so the relay's accounting
/// closes normally).
async fn post_stream(rig: &Rig, model: &str, session: &str, turn: u32) -> u16 {
    let body = format!(
        r#"{{"model":"{model}","messages":[{{"role":"user","content":"stream turn {turn}"}}],"prompt_cache_key":"{session}","stream":true,"stream_options":{{"include_usage":true}}}}"#
    );
    let (status, body, _h) = testkit::http_post(
        &rig.listen_addr,
        "/v1/chat/completions",
        body.as_bytes(),
        &[],
    );
    assert!(
        testkit::dechunk(&body).ends_with(b"data: [DONE]\n\n"),
        "the stream relayed to completion"
    );
    status
}

fn sse_ok(content: &str) -> CannedResponse {
    let body = format!(r#"{{"choices":[{{"index":0,"delta":{{"content":"{content}"}}}}]}}"#);
    let usage = r#"{"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":5,"total_tokens":105,"prompt_tokens_details":{"cached_tokens":20}}}"#;
    CannedResponse::sse(vec![
        SseChunk::event(format!("data: {body}\n\n").as_bytes()),
        SseChunk::event(format!("data: {usage}\n\n").as_bytes()),
        SseChunk::event(b"data: [DONE]\n\n"),
    ])
}

/// Queue the canned answer the named route's mock will serve next.
fn queue_ok(rig: &Rig, provider_is_p1: bool, tag: &str) {
    let mock = if provider_is_p1 { &rig.p1 } else { &rig.p2 };
    mock.queue(testkit::plan_ok(tag));
}

fn queue_ok_stream(rig: &Rig, provider_is_p1: bool, tag: &str) {
    let mock = if provider_is_p1 { &rig.p1 } else { &rig.p2 };
    mock.queue(sse_ok(tag));
}

fn trace_records(dir: &std::path::Path) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir.join("state/traces")).expect("trace dir") {
        let entry = entry.unwrap();
        if entry.path().extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        for line in std::fs::read_to_string(entry.path()).unwrap().lines() {
            out.push(serde_json::from_str(line).expect("record json"));
        }
    }
    out
}

/// The `session.bound` rows of one session, in event order:
/// `(provider, model, ttl_us, ts_us)` from the store's own event log.
fn bound_rows(dir: &std::path::Path, session: &str) -> Vec<(String, String, i64, i64)> {
    let store = vadis_store::SqliteStore::open_read_only(&dir.join("state/vadis.db")).unwrap();
    let QueryRow::Events(events) = store.query(Query::AllEvents).unwrap() else {
        panic!("events");
    };
    drop(store);
    events
        .iter()
        .filter(|e| e.kind_raw == "session.bound" && e.session.as_deref() == Some(session))
        .map(|e| {
            (
                e.payload["provider"]
                    .as_str()
                    .expect("provider")
                    .to_string(),
                e.payload["model"].as_str().expect("model").to_string(),
                e.payload["ttl_us"].as_i64().expect("ttl_us i64"),
                e.ts_us,
            )
        })
        .collect()
}

/// The live binding the projection holds for the session (the store's
/// own `Query::SessionBinding` — the shipped read path's row).
fn binding(dir: &std::path::Path, session: &str) -> Option<vadis_core::store::SessionBindingRow> {
    let store = vadis_store::SqliteStore::open_read_only(&dir.join("state/vadis.db")).unwrap();
    let row = match store.query(Query::SessionBinding {
        session_key: session,
    }) {
        Ok(QueryRow::SessionBinding(row)) => row,
        other => panic!("session binding query: {other:?}"),
    };
    drop(store);
    row
}

/// The `(turn_index, sticky_hit)` sequence of one session's requests,
/// in request order (the trace record's own identity).
fn turn_sequence(records: &[serde_json::Value], session: &str) -> Vec<(u32, bool)> {
    let mut turns: Vec<(u32, bool, u128)> = records
        .iter()
        .filter(|r| r["identity"]["session"] == session)
        .filter_map(|r| {
            let t = r["identity"]["turn_index"].as_u64()? as u32;
            let s = r["state"]["sticky_hit"].as_bool()?;
            // The arrival order tiebreak: the record's own event_id.
            let e = r["identity"]["event_id"].as_u64()? as u128;
            Some((t, s, e))
        })
        .collect();
    turns.sort_by_key(|(_, _, e)| *e);
    turns.into_iter().map(|(t, s, _)| (t, s)).collect()
}

/// Leg (a) — the TTL unit on the shipped path, **both media**: a
/// configured `60s` reaches the store as `60_000ms × 1_000 = 60_000_000
/// µs`, the event payload and the projection's expiry agree, and the
/// relation is asserted (`ttl_us == configured_ms × 1_000`), never a
/// snapshot. At `b9fd007` this read `60_000_000_000` (× 1_000_000) on
/// both media — the red this leg pins (R21-F6).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_80_ttl_is_configured_ms_times_1000_on_both_media() {
    const CONFIGURED_MS: i64 = 60_000; // `60s` in DurationVal's milliseconds

    // Buffered medium.
    let rig = start_rig("conf80-ttl-buf", "60s").await;
    queue_ok(&rig, true, "t1");
    assert_eq!(post(&rig, "p1/m-x", "S-ttl", 1), 200);
    let dir = rig.stop();
    let buffered = bound_rows(&dir, "S-ttl");
    assert_eq!(
        buffered.len(),
        1,
        "one buffered request binds exactly once (the create arm)"
    );
    let (bp, bm, bttl, bts) = &buffered[0];
    assert_eq!((bp.as_str(), bm.as_str()), ("p1", "m-x"));
    assert_eq!(
        *bttl,
        CONFIGURED_MS * 1_000,
        "spec §4.5: ttl_us is the configured milliseconds × 1 000 (µs), \
         once, at the resolution site — pre-fix this read 60_000_000_000"
    );
    let b = binding(&dir, "S-ttl").expect("a live binding");
    assert_eq!(
        b.expires_at_us - *bts,
        *bttl,
        "the projection's expiry is the event's ts_us + the SAME ttl_us"
    );
    assert_eq!((b.provider.as_str(), b.model.as_str()), ("p1", "m-x"));

    // Streaming medium: the same single request, the same unit.
    let rig_s = start_rig("conf80-ttl-str", "60s").await;
    queue_ok_stream(&rig_s, true, "t1");
    assert_eq!(post_stream(&rig_s, "p1/m-x", "S-ttl", 1).await, 200);
    let dir = rig_s.stop();
    let streamed = bound_rows(&dir, "S-ttl");
    assert_eq!(streamed.len(), 1, "one streamed request binds exactly once");
    let (sp, sm, sttl, sts) = &streamed[0];
    assert_eq!(
        (sp, sm, sttl),
        (bp, bm, bttl),
        "the two media carry the same (provider, model, ttl_us) element \
         for element — the CONF-66 discipline applied to the TTL"
    );
    let s = binding(&dir, "S-ttl").expect("a live binding");
    assert_eq!(s.expires_at_us - *sts, *sttl);
    let _ = sts;
}

/// Leg (a), effect not payload: `ttl: 2s` — the binding is live at
/// +1.4s (sticky hit) and gone at +2.6s (no sticky hit; the turn binds
/// afresh). The middle turn is also the unchanged-route control: its
/// sticky hit writes **no** second row. At `b9fd007` the row was still
/// live at +2.6s (2s had become 2_000s) — an independent red witness
/// that never reads `ttl_us`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_80_ttl_effect_live_before_the_deadline_gone_after_it() {
    let rig = start_rig("conf80-ttl-effect", "2s").await;
    queue_ok(&rig, true, "t1");
    queue_ok(&rig, true, "t2-live");
    queue_ok(&rig, true, "t3-expired");
    assert_eq!(post(&rig, "p1/m-x", "S-eff", 1), 200);

    // +1.4s: inside the 2s window — the binding is live (the control
    // observation rides on the post-stop reads below: turn 2's sticky
    // hit on the UNCHANGED route must have written no second row).
    tokio::time::sleep(std::time::Duration::from_millis(1_400)).await;
    assert_eq!(post(&rig, "p1/m-x", "S-eff", 2), 200);

    // +2.6s cumulative: past the deadline — the binding is gone, so the
    // turn is not a sticky hit and binds afresh.
    tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;
    assert_eq!(post(&rig, "p1/m-x", "S-eff", 3), 200);

    let dir = rig.stop();
    let records = trace_records(&dir);
    let seq = turn_sequence(&records, "S-eff");
    assert_eq!(
        seq,
        vec![(1, false), (2, true), (2, false)],
        "sticky_hit: false at the create, true inside the 2s window, \
         false again past it — pre-fix the third read true (2s lived as \
         2_000s); the middle turn's unchanged-route hit wrote nothing"
    );
    assert_eq!(
        bound_rows(&dir, "S-eff").len(),
        2,
        "create + the post-expiry rebind, and nothing else"
    );
}

/// Leg (b) — the move arm, buffered: one session whose client `model`
/// string moves `p1/m-x` → `p2/m-x` and stays. Exactly **two**
/// `session.bound` rows (create + move), the second carrying
/// `(p2, m-x)`; the projection follows it; the moved turn's
/// `turn_index` advances. At `b9fd007` the second row was never
/// written (the literal `false`) and the projection stayed `p1` — the
/// red this leg pins (R21-F5).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_80_moved_binding_writes_one_row_carrying_the_new_route() {
    let rig = start_rig("conf80-move-buf", "11h").await;
    queue_ok(&rig, true, "t1-p1");
    queue_ok(&rig, false, "t2-p2");
    queue_ok(&rig, false, "t3-p2");
    assert_eq!(post(&rig, "p1/m-x", "S-move", 1), 200);
    assert_eq!(post(&rig, "p2/m-x", "S-move", 2), 200);
    assert_eq!(post(&rig, "p2/m-x", "S-move", 3), 200);
    let dir = rig.stop();

    let rows = bound_rows(&dir, "S-move");
    assert_eq!(
        rows.iter()
            .map(|(p, m, _, _)| (p.as_str(), m.as_str()))
            .collect::<Vec<_>>(),
        vec![("p1", "m-x"), ("p2", "m-x")],
        "create then exactly ONE move row carrying (p2, m-x) — and the \
         unchanged third turn wrote none (the control arm, green on the \
         pre-fix tree too); pre-fix only the create row existed"
    );
    let b = binding(&dir, "S-move").expect("a live binding");
    assert_eq!(
        (b.provider.as_str(), b.model.as_str()),
        ("p2", "m-x"),
        "the projection follows the move row — pre-fix it stayed p1/m-x \
         while turns 2-3 ran on p2"
    );
    let records = trace_records(&dir);
    assert_eq!(
        turn_sequence(&records, "S-move"),
        vec![(1, false), (2, true), (3, true)],
        "turn_index: the moved turn's write is what advances the counter \
         (its turn_index is 2, the next turn reads 3) — pre-fix the moved \
         turn repeated its predecessor's index [1, 2, 2] because no write \
         happened; the unchanged third turn's sticky hit wrote nothing"
    );
}

/// Leg (d) — the same move history on the streaming medium, compared
/// with the buffered one element for element (the CONF-66 discipline):
/// the same two rows, the same projection, the same `(turn_index,
/// sticky_hit)` sequence. Pre-fix both media fail the same way (one
/// row, stale projection) — the red is the absolute expectation, the
/// equality is the invariant that must hold on every tree.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_80_streaming_move_matches_the_buffered_element_for_element() {
    // Buffered medium (the anchor).
    let rig = start_rig("conf80-media-buf", "11h").await;
    queue_ok(&rig, true, "t1");
    queue_ok(&rig, false, "t2");
    queue_ok(&rig, false, "t3");
    assert_eq!(post(&rig, "p1/m-x", "S-x", 1), 200);
    assert_eq!(post(&rig, "p2/m-x", "S-x", 2), 200);
    assert_eq!(post(&rig, "p2/m-x", "S-x", 3), 200);
    let dir = rig.stop();
    let buffered_rows = bound_rows(&dir, "S-x");
    let buffered_binding = binding(&dir, "S-x").expect("a live binding");
    let buffered_turns = turn_sequence(&trace_records(&dir), "S-x");

    // Streaming medium: the SAME history shape.
    let rig_s = start_rig("conf80-media-str", "11h").await;
    queue_ok_stream(&rig_s, true, "t1");
    queue_ok_stream(&rig_s, false, "t2");
    queue_ok_stream(&rig_s, false, "t3");
    assert_eq!(post_stream(&rig_s, "p1/m-x", "S-x", 1).await, 200);
    assert_eq!(post_stream(&rig_s, "p2/m-x", "S-x", 2).await, 200);
    assert_eq!(post_stream(&rig_s, "p2/m-x", "S-x", 3).await, 200);
    let dir = rig_s.stop();
    let streamed_rows = bound_rows(&dir, "S-x");
    let streamed_binding = binding(&dir, "S-x").expect("a live binding");
    let streamed_turns = turn_sequence(&trace_records(&dir), "S-x");

    // The absolute expectation on the streamed medium first — the red
    // at b9fd007 is this, not the comparison.
    assert_eq!(
        streamed_rows
            .iter()
            .map(|(p, m, _, _)| (p.as_str(), m.as_str()))
            .collect::<Vec<_>>(),
        vec![("p1", "m-x"), ("p2", "m-x")],
        "streamed: create + exactly one move row carrying (p2, m-x)"
    );
    assert_eq!(
        (
            streamed_binding.provider.as_str(),
            streamed_binding.model.as_str()
        ),
        ("p2", "m-x"),
        "streamed: the projection follows the move"
    );
    // Then the element-for-element comparison: route identity, expiry
    // offset and turn sequence agree across the two media.
    let strip = |rows: &[(String, String, i64, i64)]| {
        rows.iter()
            .map(|(p, m, ttl, _)| (p.clone(), m.clone(), *ttl))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        strip(&streamed_rows),
        strip(&buffered_rows),
        "the two media write the same (provider, model, ttl_us) rows"
    );
    assert_eq!(
        streamed_binding.expires_at_us - streamed_rows[1].3,
        buffered_binding.expires_at_us - buffered_rows[1].3,
        "the expiry offset from the move row's ts_us is one value on both media"
    );
    assert_eq!(
        streamed_turns, buffered_turns,
        "the (turn_index, sticky_hit) sequence is element for element equal"
    );
}
