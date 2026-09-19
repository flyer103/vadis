//! CONF-20 (§12.8): **ordered write invariant** — on one request run through
//! the pipeline with a recording store and a fake provider, the event row
//! with the highest `event_id` at the instant the attempt's request bytes are
//! handed to the wire is that attempt's `upstream.submitted` intent; nothing
//! is written between the intent commit and the attempt.
//!
//! The forwarding pipeline lands in R2-2d; this file drives the invariant at
//! the seam it is enforced on: `write_intent_then` (router-core) over the
//! real SQLite store (router-store), with the "fake provider" as the effect
//! closure that inspects the store at hand-off time. The pipeline case's
//! `#[ignore]`d twin is kept below until R2-2d wires the real proxy.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use router_core::store::{write_intent_then, EventKind, NewEvent, Query, QueryRow, Store};
use serde_json::json;

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "conf20-{}-{}-{tag}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn conf_20_ordered_write_invariant() {
    let store = router_store::SqliteStore::open(&tempdir("main").join("state/router.db")).unwrap();

    // The pipeline prefix, exactly DESIGN §12.10.5's rows 1–4.
    store
        .append(NewEvent {
            kind: EventKind::RequestReceived,
            request_id: Some("req-1"),
            session: Some("sess-1"),
            body_hash: Some("aaaaaaaaaaaaaaaa"),
            trace_ref: None,
            payload: json!({"protocol_in": "chat", "turn_index": 1}),
        })
        .unwrap();
    store
        .append(NewEvent {
            kind: EventKind::DecisionMade,
            request_id: Some("req-1"),
            session: Some("sess-1"),
            body_hash: None,
            trace_ref: None,
            payload: json!({"provider": "zai", "model": "glm-5.3", "selection_source": "explicit"}),
        })
        .unwrap();
    let intent_id = store
        .append(NewEvent {
            kind: EventKind::SessionBound,
            request_id: Some("req-1"),
            session: Some("sess-1"),
            body_hash: None,
            trace_ref: None,
            payload: json!({"session_key": "sess-1", "provider": "zai", "model": "glm-5.3", "ttl_us": 43_200_000_000_i64}),
        })
        .unwrap();

    // The fake provider: the effect closure is "the request bytes are handed
    // to the wire". At that instant the log's highest event_id MUST be the
    // intent row this effect is authorized by.
    let (intent_event, (observed_max, ())) = write_intent_then(
        &store,
        NewEvent {
            kind: EventKind::UpstreamSubmitted,
            request_id: Some("req-1"),
            session: Some("sess-1"),
            body_hash: Some("0123456789abcdef"),
            trace_ref: None,
            payload: json!({"route": "zai/glm-5.3", "attempt_index": 0, "attempt_id": "att-1"}),
        },
        |_| {
            let QueryRow::Events(events) = store.query(Query::AllEvents).unwrap() else {
                panic!("expected events");
            };
            let max = events.last().unwrap();
            assert_eq!(
                max.kind_raw,
                EventKind::UpstreamSubmitted.as_str(),
                "the last row at wire hand-off must be the intent, nothing after it"
            );
            (max.event_id, ())
        },
    )
    .unwrap();

    assert_eq!(observed_max, intent_event);
}

/// The full-pipeline twin: asserts the same invariant through the real proxy
/// + provider client once R2-2d lands them.
#[test]
#[ignore = "CONF-20: depends on the forwarding pipeline (R2-2d) and the fake ProviderClient double"]
fn conf_20_pipeline_ordered_write() {}
