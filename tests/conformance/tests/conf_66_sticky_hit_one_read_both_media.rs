//! CONF-66 (spec §6 `state.sticky_hit`, the R21-1 freeze / DESIGN §12.8's
//! allocation): **one per-request predicate, read once before any binding
//! write, identical on both media.** The value answers "did this session
//! already have a live binding row when the request arrived?" — fixed by
//! what the request *finds* in the store, never by the wire it travels.
//! A fresh session's first request records `false`, a later request of
//! the same session (binding still live) records `true`, `session: null`
//! records `false` (spec §6's own words: "read **once**, before any
//! binding write, and never recomputed within the request").
//!
//! The defect this case pins is the streaming half (R11-F2): before R21
//! the streaming path read the predicate *after its own row-4 binding
//! write* (`stream_forward.rs`'s `RelayCtx` read at the head accept), so
//! a fresh session's **first** streamed request already reported `true`
//! — the write it just made answered the question "what did you find?".
//! `conf_35`'s two existing assertions are the **buffered** flip witness;
//! this case adds the media comparison they do not make: the same
//! session history driven on both media must agree element for element.

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, CannedResponse, PlanRig, SseChunk};
use vadis_core::store::{Query, QueryRow, Store as _};

/// A streamed 200 whose final data event carries usage (CONF-30's
/// carrier shape) — so the relay's accounting closes normally.
fn sse_ok(content: &str) -> CannedResponse {
    let body = format!(r#"{{"choices":[{{"index":0,"delta":{{"content":"{content}"}}}}]}}"#);
    let usage = r#"{"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":5,"total_tokens":105,"prompt_tokens_details":{"cached_tokens":20}}}"#;
    CannedResponse::sse(vec![
        SseChunk::event(format!("data: {body}\n\n").as_bytes()),
        SseChunk::event(format!("data: {usage}\n\n").as_bytes()),
        SseChunk::event(b"data: [DONE]\n\n"),
    ])
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

/// The boolean `state.sticky_hit` sequence of one session's requests,
/// in the order the requests were made (joined on the trace record's
/// own identity).
fn sticky_sequence(records: &[serde_json::Value], session: &str) -> Vec<bool> {
    let mut turns: Vec<(u32, bool)> = records
        .iter()
        .filter(|r| r["identity"]["session"] == session)
        .map(|r| {
            (
                r["identity"]["turn_index"].as_u64().expect("turn_index") as u32,
                r["state"]["sticky_hit"].as_bool().expect("sticky_hit bool"),
            )
        })
        .collect();
    turns.sort_by_key(|(t, _)| *t);
    turns.into_iter().map(|(_, v)| v).collect()
}

/// One streamed chat request for the family's primary route, with or
/// without a session key — the streaming twin of `PlanRig::post`.
async fn post_stream(rig: &PlanRig, session: Option<&str>, turn: u32) -> u16 {
    let key = session
        .map(|s| format!(r#","prompt_cache_key":"{s}""#))
        .unwrap_or_default();
    let body = format!(
        r#"{{"model":"p-plan/m1","messages":[{{"role":"user","content":"stream turn {turn}"}}]{key},"stream":true,"stream_options":{{"include_usage":true}}}}"#
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

/// The streaming leg (the red half at HEAD): a fresh session's **first**
/// streamed request records `false` — the predicate answers what the
/// request *found*, and a fresh session found nothing — and the same
/// session's second streamed request (binding still live) records
/// `true`. `session: null` records `false` and writes no `session.bound`
/// event (the control leg).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_66_streaming_first_request_of_a_fresh_session_reads_false() {
    let (plan, api, dir, listen_addr) =
        testkit::plan_rig_parts("conf66-stream", "", testkit::PLAN_POLICY_DEFAULT).await;
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    let rig = PlanRig {
        plan,
        api,
        listen_addr,
        dir,
        serve_task,
    };

    rig.plan.queue(sse_ok("t1"));
    rig.plan.queue(sse_ok("t2"));

    let s1 = post_stream(&rig, Some("S-stream"), 1).await;
    assert_eq!(s1, 200);
    let s2 = post_stream(&rig, Some("S-stream"), 2).await;
    assert_eq!(s2, 200);

    let dir = rig.stop();
    let records = trace_records(&dir);
    let seq = sticky_sequence(&records, "S-stream");
    assert_eq!(
        seq,
        vec![false, true],
        "spec §6: a fresh session's first streamed request is false (it \
         found no binding), the second is true (the first wrote one) — \
         the element-for-element sequence, not a weak 'one is true'"
    );

    // Control leg, event side: the SAME one value decides the event row
    // (spec §6 / §4.5 row 4) — turn 1 (sticky_hit false) wrote exactly
    // one session.bound; turn 2 (sticky_hit true, route unchanged)
    // wrote none (bind_session's early return).
    let store = vadis_store::SqliteStore::open(&dir.join("state/router.db")).unwrap();
    let QueryRow::Events(events) = store.query(Query::AllEvents).unwrap() else {
        panic!("events");
    };
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind_raw == "session.bound" && e.session == Some("S-stream".to_string()))
            .count(),
        1,
        "the one value gates the event too: turn 1 bound, turn 2's sticky \
         hit on an unchanged route wrote nothing"
    );
    drop(store);
}

/// The buffered leg: the same session history on the buffered medium
/// produces the same element-for-element sequence (the buffered half was
/// already single-read; this is the comparison anchor).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_66_media_equality_the_same_history_agrees_element_for_element() {
    // Buffered medium.
    let (plan, api, dir, listen_addr) =
        testkit::plan_rig_parts("conf66-buf", "", testkit::PLAN_POLICY_DEFAULT).await;
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    let rig = PlanRig {
        plan,
        api,
        listen_addr,
        dir,
        serve_task,
    };
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_ok("t2"));
    let (s1, _b, _h) = rig.post(Some("S-x"), 1);
    assert_eq!(s1, 200);
    let (s2, _b2, _h2) = rig.post(Some("S-x"), 2);
    assert_eq!(s2, 200);
    let dir = rig.stop();
    let buffered = sticky_sequence(&trace_records(&dir), "S-x");

    // Streaming medium: the SAME history shape (same turns, same session
    // freshness) on the other medium.
    let (plan, api, dir, listen_addr) =
        testkit::plan_rig_parts("conf66-str", "", testkit::PLAN_POLICY_DEFAULT).await;
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    let rig = PlanRig {
        plan,
        api,
        listen_addr,
        dir,
        serve_task,
    };
    rig.plan.queue(sse_ok("t1"));
    rig.plan.queue(sse_ok("t2"));
    let s1 = post_stream(&rig, Some("S-x"), 1).await;
    assert_eq!(s1, 200);
    let s2 = post_stream(&rig, Some("S-x"), 2).await;
    assert_eq!(s2, 200);
    let dir = rig.stop();
    let streamed = sticky_sequence(&trace_records(&dir), "S-x");

    // The element-for-element comparison (spec §6: "the same value on
    // both forwarding media") — naming the expected sequence, not just
    // comparing the two media against each other.
    assert_eq!(
        buffered,
        vec![false, true],
        "buffered: fresh session turn 1 false, turn 2 true"
    );
    assert_eq!(
        streamed, buffered,
        "cross-media equality is element for element: {streamed:?} vs {buffered:?} \
         (before R21 the streaming medium read [true, true] — its read sat \
         after its own binding write, R11-F2)"
    );
}

/// Control leg: `session: null` ⇒ `false`, and **no `session.bound`
/// event** — a session-less request writes nothing (spec §4.5 row 4's
/// own rule, asserted on the store's own event rows).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_66_session_null_is_false_and_writes_no_binding_event() {
    let (plan, api, dir, listen_addr) =
        testkit::plan_rig_parts("conf66-null", "", testkit::PLAN_POLICY_DEFAULT).await;
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    let rig = PlanRig {
        plan,
        api,
        listen_addr,
        dir,
        serve_task,
    };
    rig.plan.queue(testkit::plan_ok("t1"));
    let (s, _b, _h) = rig.post(None, 1);
    assert_eq!(s, 200);

    let dir = rig.stop();
    let records = trace_records(&dir);
    let rec = records
        .iter()
        .find(|r| r["identity"]["session"].is_null())
        .expect("the session-less request's record");
    assert_eq!(
        rec["state"]["sticky_hit"], false,
        "session: null is always false (spec §6)"
    );

    let store = vadis_store::SqliteStore::open(&dir.join("state/router.db")).unwrap();
    let QueryRow::Events(events) = store.query(Query::AllEvents).unwrap() else {
        panic!("events");
    };
    assert!(
        !events.iter().any(|e| e.kind_raw == "session.bound"),
        "a session-less request writes no session.bound event (spec §4.5 row 4)"
    );
}
