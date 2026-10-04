//! CONF-81 (spec §4.5's sticky-table bullets / §6's binding second-arm
//! paragraph; DESIGN §12.10.5 note R7, the R28-1 freeze): **an account
//! move is a binding move — the event log is the whole truth of a
//! session's binding, including when a plan switch moves it.**
//!
//! `record_plan_switch`'s re-point loop used to write the `sessions`
//! projection with **no** `session.bound` event row, anchored on the
//! `plan.switched` event id — so an independent rebuild of the log
//! silently undid the re-point (`requests_seen` live 3 vs rebuild 1;
//! spill-only, live `('p-api','m1',2)` vs rebuild `('p-plan','m1',1)` —
//! R27-3's committed measurement, `R27-F1`). The frozen shape (R28-1):
//! the re-point IS row 4's own move arm — one `session.bound` row per
//! re-pointed session, payload `{session_key, provider, model, ttl_us}`
//! (the same shape `Accountant::bind_session` writes), appended after
//! the `plan.switched` row, and the projection write rides on THAT row
//! (`last_event` = the move row's id, never the `plan.switched` id).
//! `rebuild_sessions` and its rule are untouched — the log gains the
//! rows the rebuild already reads.
//!
//! Legs (all on the real `serve` assembly over loopback mocks, a plan
//! family whose primary answers `403 quota_exhausted`, `session.ttl`
//! at a whole-hour granularity — the testkit plan rig's own `11h`):
//!
//! - **(a) the row** — a session live at the spill gains exactly one
//!   further `session.bound` row naming the overflow route, with
//!   `request_id` = the spilling request. Red at `9f2ed21`: it gains
//!   none (the re-point was projection-only).
//! - **(b) the relation** — `rebuild(Projection::Sessions)` over that
//!   store is a no-op element for element (`provider` / `model` /
//!   `requests_seen` / `expires_at_us`, whose only anchor is
//!   `last_event`'s own `ts_us` — CONF-21's property), and the expiry
//!   relation `expires_at_us − the move row's ts_us == ttl_us` holds
//!   for the re-pointed row too. Red at base: live
//!   `('p-api','m1',2)` vs the rebuild rule's `('p-plan','m1',1)`.
//! - **(c) the no-regression arm** — a turn resolving to the route the
//!   session is already on writes no row, including a turn the family
//!   serves from `overflow` to a session the handoff already
//!   re-pointed there (CONF-80(c)'s arm restated on a switched
//!   family). Green at base — the leg the fix must keep.
//! - **(d) the relay** — (a) and (b) hold element for element on the
//!   streamed path (the same `record_plan_switch`, `stream_forward`'s
//!   403 arm — a per-medium divergence is impossible by construction,
//!   asserted anyway per the CONF-66 discipline).

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, CannedResponse, PlanRig, SseChunk};
use vadis_core::store::{Projection, Query, QueryRow, Store as _};

/// The testkit plan rig's configured `session.ttl: 11h`, in the unit
/// rule's own terms: `11h` = 39_600_000 ms, and `ttl_us` is the
/// configured milliseconds × 1_000 (spec §4.5, CONF-80(a)'s relation —
/// asserted here for a re-pointed row too).
const CONFIGURED_MS: i64 = 11 * 3_600 * 1_000;
const TTL_US: i64 = CONFIGURED_MS * 1_000;

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

async fn rig(tag: &str) -> PlanRig {
    let (plan, api, dir, listen_addr) =
        testkit::plan_rig_parts(tag, "", testkit::PLAN_POLICY_DEFAULT).await;
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    PlanRig {
        plan,
        api,
        listen_addr,
        dir,
        serve_task,
    }
}

/// One buffered chat request for the family's primary route.
fn post(rig: &PlanRig, session: &str, turn: u32) -> u16 {
    let (status, _body, _h) = rig.post(Some(session), turn);
    status
}

/// One streamed chat request for the family's primary route (the
/// relay's carrier shape, CONF-66's streaming twin of `PlanRig::post`).
async fn post_stream(rig: &PlanRig, session: &str, turn: u32) -> u16 {
    let key = format!(r#","prompt_cache_key":"{session}""#);
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

/// The `session.bound` rows of one session, in event order:
/// `(provider, model, ttl_us, ts_us, request_id)`.
fn bound_rows(
    dir: &std::path::Path,
    session: &str,
) -> Vec<(String, String, i64, i64, Option<String>)> {
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
                e.request_id.clone(),
            )
        })
        .collect()
}

/// The live `sessions` row for the session, as the shipped read path
/// sees it — `(provider, model, requests_seen, expires_at_us)`.
fn live_binding(dir: &std::path::Path, session: &str) -> (String, String, i64, i64) {
    let store = vadis_store::SqliteStore::open_read_only(&dir.join("state/vadis.db")).unwrap();
    let row = match store.query(Query::SessionBinding {
        session_key: session,
    }) {
        Ok(QueryRow::SessionBinding(row)) => row.expect("a live binding row"),
        other => panic!("session binding query: {other:?}"),
    };
    drop(store);
    (
        row.provider,
        row.model,
        row.requests_seen,
        row.expires_at_us,
    )
}

/// Every trace record, in write order.
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

/// The `request_id` of the session's n-th turn's trace record (the
/// request that carried the evidence — what the move row must name).
fn turn_request_id(dir: &std::path::Path, session: &str, turn: u64) -> String {
    trace_records(dir)
        .into_iter()
        .find(|r| r["identity"]["session"] == session && r["identity"]["turn_index"] == turn)
        .expect("the turn's record")["identity"]["request_id"]
        .as_str()
        .expect("request_id str")
        .to_string()
}

/// Leg (b)'s engine, the 2-row form (create + one move).
fn rebuild_is_a_noop(dir: &std::path::Path, session: &str) {
    rebuild_is_a_noop_n(dir, session, 2);
}

/// Leg (b)'s engine, the round-trip form (create + spill move +
/// recovery move for the spilled session, plus the probing session's
/// own create row) — the shape leg (d) drives.
fn rebuild_is_a_noop_three(dir: &std::path::Path, session: &str) {
    rebuild_is_a_noop_n(dir, session, 4);
}

fn rebuild_is_a_noop_n(dir: &std::path::Path, session: &str, expect_rows: usize) {
    let before = live_binding(dir, session);
    let store = vadis_store::SqliteStore::open(&dir.join("state/vadis.db")).unwrap();
    let stats = store
        .rebuild(Projection::Sessions)
        .expect("rebuild computes");
    assert_eq!(
        stats.sessions, expect_rows,
        "the rebuild replayed every session.bound row (create + the moves)"
    );
    drop(store);
    let after = live_binding(dir, session);
    assert_eq!(
        before, after,
        "rebuild(Projection::Sessions) over a spilled store is a NO-OP: \\\n         \
         the incremental projection and an event-derived rebuild agree \\\n         \
         element for element — pre-fix the rebuild silently undid the \\\n         \
         re-point (live ('p-api','m1',2) vs rebuild ('p-plan','m1',1))"
    );
}

/// Legs (a) + (b), buffered: a session live at the spill gains exactly
/// one further `session.bound` row (the overflow route, `request_id` =
/// the spilling request), and an independent rebuild of the log is a
/// no-op. Red at `9f2ed21`: the re-point was projection-only, so the
/// session gained no row and the rebuild disagreed on provider, count
/// and expiry.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_81_spill_move_writes_the_row_and_rebuild_is_a_noop() {
    let rig = rig("conf81-buf").await;
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_forbidden_403());
    rig.api.queue(testkit::plan_ok("spilled"));
    assert_eq!(post(&rig, "S1", 1), 200, "turn 1 binds the primary");
    assert_eq!(post(&rig, "S1", 2), 200, "turn 2's 403 spills to overflow");
    let dir = rig.stop();

    // Leg (a): the row.
    let rows = bound_rows(&dir, "S1");
    let spill_req = turn_request_id(&dir, "S1", 2);
    assert_eq!(
        rows.iter()
            .map(|(p, m, _, _, _)| (p.as_str(), m.as_str()))
            .collect::<Vec<_>>(),
        vec![("p-plan", "m1"), ("p-api", "m1")],
        "create then exactly ONE move row carrying the overflow route \\\n         \
         (the account handoff writes row 4's own row — note R7); \\\n         \
         pre-fix only the create row existed (the re-point was \\\n         \
         projection-only, R27-F1)"
    );
    assert_eq!(
        rows[1].4.as_deref(),
        Some(spill_req.as_str()),
        "the move row's request_id is the request that carried the \\\n         \
         evidence (the spilling request), per the frozen shape"
    );
    assert_eq!(
        rows[1].2, TTL_US,
        "ttl_us is the resolution site's own value"
    );

    // Leg (b): the relation — live row, rebuild, no-op, and the TTL
    // relation for the re-pointed row.
    let live = live_binding(&dir, "S1");
    assert_eq!(
        (live.0.as_str(), live.1.as_str(), live.2),
        ("p-api", "m1", 2),
        "live projection: re-pointed, and a move counts (requests_seen \\\n         \
         == the session's session.bound count, DESIGN §12.10.5's rule)"
    );
    rebuild_is_a_noop(&dir, "S1");
    let after = live_binding(&dir, "S1");
    assert_eq!(
        after.3 - rows[1].3,
        TTL_US,
        "expires_at_us − the MOVE row's ts_us == configured 11h in µs \\\n         \
         (the anchor is the move's own session.bound row, never the \\\n         \
         plan.switched row — CONF-80(a)'s relation, now true of a \\\n         \
         re-pointed row too)"
    );
}

/// Leg (c), buffered: after the spill, the family serves the session
/// from `overflow` and the handoff has already re-pointed it there —
/// that turn is an unchanged sticky hit and must write **no** further
/// row (CONF-80(c)'s arm on a switched family; green at base — the leg
/// the fix must keep). The assertion is the row list across turn 3,
/// never its content: at base the list holds the create row only, at
/// HEAD create + move — either way turn 3 adds nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_81_turn_on_the_repointed_route_writes_no_row() {
    let rig = rig("conf81-ctrl").await;
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_forbidden_403());
    rig.api.queue(testkit::plan_ok("spilled"));
    rig.api.queue(testkit::plan_ok("t3-unchanged"));
    assert_eq!(post(&rig, "S1", 1), 200);
    assert_eq!(post(&rig, "S1", 2), 200, "turn 2 spills");
    assert_eq!(post(&rig, "S1", 3), 200, "turn 3 is served (from overflow)");
    assert_eq!(rig.plan.requests().len(), 2, "turn 1 and the 403 only");
    assert_eq!(rig.api.requests().len(), 2, "turns 2 and 3 on overflow");
    let dir = rig.stop();

    let rows = bound_rows(&dir, "S1");
    let turn3_req = turn_request_id(&dir, "S1", 3);
    assert!(
        rows.iter()
            .all(|(_, _, _, _, req)| req.as_deref() != Some(turn3_req.as_str())),
        "turn 3 wrote NO row of its own: the unchanged sticky hit on the \\\n         \
         re-pointed route stays silent (the arm that must not regress \\\n         \
         — green at base, where the list held the create row only, and \\\n         \
         at HEAD, where it holds create + the handoff's move row; neither \\\n         \
         names turn 3). Rows: {:?}",
        rows.iter()
            .map(|(p, m, _, _, r)| (p.clone(), m.clone(), r.clone()))
            .collect::<Vec<_>>()
    );
    let turn3 = trace_records(&dir)
        .into_iter()
        .find(|r| r["identity"]["session"] == "S1" && r["identity"]["turn_index"] == 3)
        .expect("turn 3 record");
    assert_eq!(
        turn3["state"]["sticky_hit"], true,
        "turn 3 found the re-pointed binding (the read is unchanged)"
    );
    assert_eq!(
        turn3["decision"]["provider"], "p-api",
        "the family served turn 3 from overflow"
    );
}

/// Leg (d), streamed: legs (a) and (b) hold element for element on the
/// relay. The stream medium's *reachable* account-move arm is the
/// streamed probe's success (`plan_probe_succeeded`, stream_forward's
/// 2xx-head arm — the relay's 403 arm cannot fire in v0.1 because the
/// failure-head classification never sees the error body, registered
/// in the card): a buffered spill moves the family, then a streamed
/// boundary probe's success re-points the spilled session and must
/// write the move row (red at base: the same shared loop wrote none),
/// with the rebuild agreeing element for element and the two media
/// compared on the same recovery shape.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_81_stream_relay_move_writes_the_row_and_rebuild_agrees() {
    // Buffered anchor: the spill, then the recovery via a buffered
    // boundary probe (the same history the streamed leg replays).
    let rig_b = rig("conf81-media-buf").await;
    rig_b.plan.queue(testkit::plan_ok("t1"));
    rig_b.plan.queue(testkit::plan_forbidden_403());
    rig_b.plan.queue(testkit::plan_ok("probe-buf"));
    rig_b.api.queue(testkit::plan_ok("spilled"));
    assert_eq!(post(&rig_b, "S1", 1), 200);
    assert_eq!(post(&rig_b, "S1", 2), 200, "turn 2's 403 spills");
    // Past the 403's provider demotion (Retry-After: 1); the family's
    // own cooldown is 0s (CONF-33's recipe).
    tokio::time::sleep(std::time::Duration::from_millis(1_300)).await;
    assert_eq!(post(&rig_b, "S2", 1), 200, "the buffered probe recovers");
    let dir_b = rig_b.stop();
    let buffered_rows = bound_rows(&dir_b, "S1");
    let buffered_live = live_binding(&dir_b, "S1");
    assert_eq!(
        buffered_rows
            .iter()
            .map(|(p, m, _, _, _)| (p.as_str(), m.as_str()))
            .collect::<Vec<_>>(),
        vec![("p-plan", "m1"), ("p-api", "m1"), ("p-plan", "m1")],
        "buffered anchor: create + the spill's move + the recovery's move"
    );
    rebuild_is_a_noop_three(&dir_b, "S1");

    // Streamed: the spill is buffered (turns 1-2), then the streamed
    // boundary probe's success IS the recovery — the relay's 2xx-head
    // arm calls the same `record_plan_switch`, whose re-point loop must
    // write S1's move row back to the primary.
    let rig_s = rig("conf81-media-str").await;
    rig_s.plan.queue(testkit::plan_ok("t1"));
    rig_s.plan.queue(testkit::plan_forbidden_403());
    rig_s.plan.queue(sse_ok("probe-stream"));
    rig_s.api.queue(testkit::plan_ok("spilled"));
    assert_eq!(post(&rig_s, "S1", 1), 200);
    assert_eq!(post(&rig_s, "S1", 2), 200, "turn 2's 403 spills (buffered)");
    tokio::time::sleep(std::time::Duration::from_millis(1_300)).await;
    assert_eq!(
        post_stream(&rig_s, "S2", 1).await,
        200,
        "the streamed probe recovers"
    );
    let dir_s = rig_s.stop();

    // Leg (a) on the relay, absolute first (the red at base is this).
    let rows = bound_rows(&dir_s, "S1");
    let probe_req = turn_request_id(&dir_s, "S2", 1);
    assert_eq!(
        rows.iter()
            .map(|(p, m, _, _, _)| (p.as_str(), m.as_str()))
            .collect::<Vec<_>>(),
        vec![("p-plan", "m1"), ("p-api", "m1"), ("p-plan", "m1")],
        "streamed: create + the spill's move + the recovery's move row \\\n         \
         (the shared re-point loop writes it — pre-fix only the create \\\n         \
         row existed)"
    );
    assert_eq!(
        rows[2].4.as_deref(),
        Some(probe_req.as_str()),
        "streamed: the recovery move row names the probing request"
    );

    // Leg (b) on the relay.
    let live = live_binding(&dir_s, "S1");
    assert_eq!(
        (live.0.as_str(), live.1.as_str(), live.2),
        ("p-plan", "m1", 3),
        "streamed: re-pointed back, a move counts each time"
    );
    rebuild_is_a_noop_three(&dir_s, "S1");
    let after = live_binding(&dir_s, "S1");
    assert_eq!(
        after.3 - rows[2].3,
        TTL_US,
        "streamed: the move row anchors"
    );

    // And the two media agree element for element: the same
    // (provider, model, ttl_us) rows and the same live projection row.
    let strip = |rows: &[(String, String, i64, i64, Option<String>)]| {
        rows.iter()
            .map(|(p, m, ttl, _, _)| (p.clone(), m.clone(), *ttl))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        strip(&rows),
        strip(&buffered_rows),
        "the two media write the same (provider, model, ttl_us) rows"
    );
    assert_eq!(
        (live.0, live.1, live.2),
        (buffered_live.0, buffered_live.1, buffered_live.2),
        "the two media hold the same live projection row"
    );
}
