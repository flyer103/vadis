//! CONF-82 (spec §4.2's classification-evidence paragraph + §4.6 rule 3,
//! DESIGN §12.10.3 R12 / §12.10.5 note R8, the R29-1 freeze §5): **a
//! failure head is classified on its own answer — the head's status and
//! headers AND the body bytes it carries — on both forwarding paths.**
//!
//! The stream relay's failure-head arm used to hand the classifier an
//! empty body (`stream_forward.rs`'s `body: b""` behind the body-less
//! `StreamHead::as_upstream_response`), so every body-dependent
//! classifier entry was dead on the streaming medium: a streamed
//! `403` carrying `insufficient_quota` classified `auth`, demoted
//! nothing, wrote no `plan.switched` and re-pointed no session —
//! spec §4.6 rule 3's account-moving verdict was unreachable for
//! every shipped client (both stream) — while the buffered path
//! spills and re-points on the identical bytes (`R28-F3`).
//!
//! Post-freeze rule (R12): the failure head's evidence is the
//! buffered path's own expression — the head's body is read to the
//! end through one provider-layer reader
//! (`StreamHead::into_upstream_response`) under §12.10.3 R4's
//! existing idle bound, no byte cap, and a read that ends short
//! simply leaves the classifier the bytes that arrived (the pre-fix
//! verdict when none did).
//!
//! Legs (the R29-1 freeze §5, all on the real `serve` assembly over
//! the loopback plan rig; witnesses read from the event log, the
//! projections and the trace JSONL — never internal structs; the
//! store is opened read-only only after the rig stops, the writer
//! lock is `serve`'s):
//!
//! - **(a) the buffered arm** — the reference: turn 1 served on the
//!   plan, turn 2's quota-worded `403` spills. Witness: the four
//!   facts (the `error.classified` row with `reason
//!   == "quota_exhausted"` and a non-null `demotion`, a cooldown row
//!   for `p-plan` seen live through `/health`, exactly one
//!   `plan.switched` row `primary → overflow`, and the session's
//!   `session.bound` rows `[create, move]` with the move naming
//!   turn 2's `request_id`). Green at `3e9d102` — it is the
//!   already-correct medium, re-asserted here as leg (b)'s divisor.
//! - **(b) the streamed arm** — the identical canned bytes answer a
//!   streamed session (turn 1 streamed and served, turn 2 streamed
//!   and meeting the `403`): the same four facts element for
//!   element, plus the comparator (the two arms' rows stripped to
//!   their comparable fields are equal). Red at `3e9d102` — four
//!   facts, one cause: `reason == "auth"`, `demotion == null`, no
//!   cooldown, zero `plan.switched` rows, one `session.bound`
//!   row instead of two. The client-visible side (200, the
//!   overflow's SSE bytes verbatim) is also asserted and is green on
//!   both trees — which is exactly why the defect hid.
//! - **(c) the discriminant** — a `403` whose body LACKS the quota
//!   wording on BOTH media: `reason == "auth"`, no demotion, no
//!   cooldown, zero `plan.switched` rows, the create row only,
//!   the turn served `200` from overflow with
//!   `failover_from = "p-plan/m1"` and `plan_switch == null`. Green
//!   at base on both media and it must STAY green — it is the
//!   assertion a "stream `403` ⇒ exhaustion" shortcut would break.
//! - **(d) the read's edge** — streamed only: a partial body
//!   (the quota wording in its first chunk, then the connection
//!   aborts) still classifies `quota_exhausted` and moves the
//!   account (red at base: the pre-fix tree reads no byte of it);
//!   a body that delivers NOTHING and aborts classifies on status
//!   and headers alone (`auth`), moving nothing — the pre-fix
//!   verdict, the E4 degradation rule's guard, green by
//!   construction.
//! - **(e) the forwarded-byte witness** — for every request of the
//!   scripted set, each mock's recorded body equals the client's own
//!   body modulo the two permitted byte-level mutations (AGENTS 1),
//!   asserted as a computed relation, never a snapshot. Green on
//!   both trees; a failure here is a stop-and-report.
//!
//! (The R29-1 freeze §5(d)'s optional idle-bound form reaches the
//! same `StreamRead::Failed` arm `read_chunk` already maps for
//! CONF-13/CONF-30's fixtures; the abort fixtures here drive both
//! `Failed` shapes — a mid-body read error (the socket cut) and a
//! short body — through the same arm, so the loop's break behaviour
//! is exercised on all its exits except the timed-out one, whose
//! mapping is `read_chunk`'s own frozen contract, unchanged.)

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, CannedResponse, PlanRig, RecordedRequest, SseChunk};
use vadis_core::store::{Query, QueryRow, Store as _};

/// The testkit plan rig's configured `session.ttl: 11h`, in µs
/// (CONF-80(a)'s relation, the same constant CONF-81 asserts).
const TTL_US: i64 = 11 * 3_600 * 1_000 * 1_000;

/// The quota-worded 403 body both media answer with — the fixture
/// `testkit::plan_forbidden_403()` serves, verbatim.
const QUOTA_403_BODY: &[u8] = br#"{"error":{"message":"You have exceeded your current quota","type":"insufficient_quota","code":"insufficient_quota"}}"#;

/// A 403 body that matches NO classifier pattern table (the probe
/// shape the failure-head evidence already used): status-and-headers
/// alone decide, on both media.
const AUTH_403_BODY: &[u8] =
    br#"{"error":{"type":"authentication_error","message":"Incorrect API key provided"}}"#;

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

/// One streamed chat request for the family's primary route
/// (CONF-81's carrier shape: `"stream": true` +
/// `stream_options.include_usage`). Panics unless the relay completed,
/// so a truncated stream cannot pass for a served one.
fn post_stream(rig: &PlanRig, session: &str, turn: u32) -> (u16, Vec<u8>) {
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
    let events = testkit::dechunk(&body);
    assert!(
        events.ends_with(b"data: [DONE]\n\n"),
        "the stream relayed to completion"
    );
    (status, events)
}

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

/// A FAILURE-status head delivered on the streaming wire: the status
/// line is the given one, the body arrives as chunked bytes (the
/// same framing a streamed error answer uses), optionally cut off
/// mid-body. `CannedResponse`'s fields are `pub` (the testkit's own
/// authoring rule, per the R29-1 freeze §5), so the case builds the
/// fixture directly: the queued chunks are the body's own bytes, and
/// `abort_after` closes the socket instead of finishing the chunked
/// body.
fn sse_status(status: u16, reason: &'static str, body: &[u8], abort_after: bool) -> CannedResponse {
    let mut chunks = vec![SseChunk::event(body)];
    if abort_after {
        chunks.push(SseChunk::abort_after(0));
    }
    CannedResponse {
        status,
        reason,
        headers: vec![("retry-after".into(), "1".into())],
        body: Vec::new(),
        sse_chunks: Some(chunks),
    }
}

/// A buffered `403` with the given body and `Retry-After: 1` (the
/// `plan_forbidden_403` shape with a caller-chosen body — the
/// discriminant's arm).
fn forbidden_403(body: &[u8]) -> CannedResponse {
    CannedResponse::json(403, "Forbidden", body).with_header("retry-after", "1")
}

// ---------------------------------------------------------------------
// The log/trace readers (log-level witnesses only, CONF-81's helpers;
// every store open happens only after the rig's serve task stopped —
// the state dir's writer lock is serve's)
// ---------------------------------------------------------------------

fn events(dir: &std::path::Path) -> Vec<vadis_core::store::StoredEvent> {
    let store = vadis_store::SqliteStore::open_read_only(&dir.join("state/vadis.db")).unwrap();
    let QueryRow::Events(events) = store.query(Query::AllEvents).unwrap() else {
        panic!("events");
    };
    events
}

/// The `error.classified` rows of one request, in event order.
fn classified(dir: &std::path::Path, request_id: &str) -> Vec<serde_json::Value> {
    events(dir)
        .into_iter()
        .filter(|e| e.kind_raw == "error.classified" && e.request_id.as_deref() == Some(request_id))
        .map(|e| e.payload)
        .collect()
}

/// The `plan.switched` rows of one run, payloads in event order.
fn plan_switches(dir: &std::path::Path) -> Vec<serde_json::Value> {
    events(dir)
        .into_iter()
        .filter(|e| e.kind_raw == "plan.switched")
        .map(|e| e.payload)
        .collect()
}

/// The `failover.triggered` rows of one request.
fn failovers(dir: &std::path::Path, request_id: &str) -> Vec<serde_json::Value> {
    events(dir)
        .into_iter()
        .filter(|e| {
            e.kind_raw == "failover.triggered" && e.request_id.as_deref() == Some(request_id)
        })
        .map(|e| e.payload)
        .collect()
}

/// The `session.bound` rows of one session, in event order:
/// `(provider, model, ttl_us, request_id)`.
fn bound_rows(dir: &std::path::Path, session: &str) -> Vec<(String, String, i64, Option<String>)> {
    events(dir)
        .into_iter()
        .filter(|e| e.kind_raw == "session.bound" && e.session.as_deref() == Some(session))
        .map(|e| {
            (
                e.payload["provider"]
                    .as_str()
                    .expect("provider")
                    .to_string(),
                e.payload["model"].as_str().expect("model").to_string(),
                e.payload["ttl_us"].as_i64().expect("ttl_us"),
                e.request_id.clone(),
            )
        })
        .collect()
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

/// The `request_id` of a session's n-th turn's trace record.
fn turn_request_id(dir: &std::path::Path, session: &str, turn: u64) -> String {
    trace_records(dir)
        .into_iter()
        .find(|r| r["identity"]["session"] == session && r["identity"]["turn_index"] == turn)
        .expect("the turn's record")["identity"]["request_id"]
        .as_str()
        .expect("request_id str")
        .to_string()
}

/// The trace record of a session's n-th turn.
fn turn_record(dir: &std::path::Path, session: &str, turn: u64) -> serde_json::Value {
    trace_records(dir)
        .into_iter()
        .find(|r| r["identity"]["session"] == session && r["identity"]["turn_index"] == turn)
        .expect("the turn's record")
}

/// The live `/health` plan section (CONF-77's timing witness for a
/// live demotion — read while the rig is up; it answers the
/// availability question through the same single owner the walk and
/// the probe gate consult).
fn http_get_health(addr: &str) -> serde_json::Value {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(addr).expect("connect");
    let req = format!("GET /health HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    let text = String::from_utf8_lossy(&buf).into_owned();
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("");
    serde_json::from_str(body.trim()).expect("health json")
}

// ---------------------------------------------------------------------
// The four facts, one function (legs (a), (b), (d) share the exact
// same witness; only the expected class words differ)
// ---------------------------------------------------------------------

/// The four facts of a spill attempt, read from one stopped run's
/// log: the classification row (reason/action/matched/demotion), the
/// one plan.switched row, the session's bound rows (create + the
/// move, the move naming the spilling request), and the failover
/// record. The demotion's cooldown is witnessed live through
/// `/health` at the spill moment (the rig's writer lock prevents a
/// second store reader while serve runs, and /health answers the
/// same availability question through the same single owner).
struct SpillFacts {
    reason: String,
    action: String,
    matched: String,
    demotion: serde_json::Value,
    switch: Option<(String, String, String, String, bool)>,
    bound: Vec<(String, String, i64, Option<String>)>,
    failover_from: serde_json::Value,
    failover_reason: String,
}

fn spill_facts(
    dir: &std::path::Path,
    session: &str,
    spill_turn: u64,
    expect_reason: &str,
    expect_matched: &str,
    expect_switch: bool,
) -> SpillFacts {
    let spill_req = turn_request_id(dir, session, spill_turn);

    // Fact 1 — the classification row and its demotion member.
    let rows = classified(dir, &spill_req);
    assert_eq!(rows.len(), 1, "exactly one error.classified row");
    let p = &rows[0];
    assert_eq!(
        p["reason"].as_str().unwrap(),
        expect_reason,
        "the failure head is classified on its own body (R12): a \
         quota-worded 403 is `{expect_reason}` on BOTH media"
    );
    let expect_action =
        if expect_reason == "format_error" || expect_reason == "content_policy_blocked" {
            "abort"
        } else {
            "fallback_provider"
        };
    assert_eq!(p["action"].as_str().unwrap(), expect_action);
    assert_eq!(p["matched"].as_str().unwrap(), expect_matched);
    let demotion = p["demotion"].clone();
    if expect_reason == "quota_exhausted" {
        assert_eq!(
            demotion,
            serde_json::json!({"scope": "provider", "ttl_s": 1}),
            "the Retry-After: 1 shrinks the demotion to one second"
        );
    } else {
        assert_eq!(demotion, serde_json::Value::Null, "no demotion");
    }

    // Fact 3 — the plan policy's account move.
    let switches = plan_switches(dir);
    let switch = if expect_switch {
        assert_eq!(switches.len(), 1, "exactly one plan.switched row");
        let s = &switches[0];
        let tuple = (
            s["from_account"].as_str().unwrap().to_string(),
            s["to_account"].as_str().unwrap().to_string(),
            s["reason"].as_str().unwrap().to_string(),
            s["from_route"].as_str().unwrap().to_string(),
            s["probe"].as_bool().unwrap(),
        );
        assert_eq!(
            tuple,
            (
                "primary".to_string(),
                "overflow".to_string(),
                "primary_exhausted".to_string(),
                "p-plan/m1".to_string(),
                false
            ),
            "the family moved primary -> overflow, reason primary_exhausted, not a probe"
        );
        Some(tuple)
    } else {
        assert!(
            switches.is_empty(),
            "zero plan.switched rows: the account never moved"
        );
        None
    };

    // Fact 4 — the binding move (CONF-81's shape).
    let bound = bound_rows(dir, session);
    if expect_switch {
        assert_eq!(
            bound
                .iter()
                .map(|(p, m, _, _)| (p.as_str(), m.as_str()))
                .collect::<Vec<_>>(),
            vec![("p-plan", "m1"), ("p-api", "m1")],
            "create then exactly ONE move row carrying the overflow route"
        );
        assert_eq!(
            bound[1].3.as_deref(),
            Some(spill_req.as_str()),
            "the move row names the spilling request"
        );
    } else {
        assert_eq!(
            bound
                .iter()
                .map(|(p, m, _, _)| (p.as_str(), m.as_str()))
                .collect::<Vec<_>>(),
            vec![("p-plan", "m1")],
            "the create row only: nothing re-pointed the session"
        );
    }

    // The failover record (leg (c)'s `failover_from` witness input).
    let fovs = failovers(dir, &spill_req);
    assert_eq!(fovs.len(), 1, "one failover.triggered row");
    let failover_from = fovs[0]["from"].clone();
    let failover_reason = fovs[0]["reason"].as_str().unwrap().to_string();

    SpillFacts {
        reason: expect_reason.to_string(),
        action: expect_action.to_string(),
        matched: expect_matched.to_string(),
        demotion,
        switch,
        bound,
        failover_from,
        failover_reason,
    }
}

// ---------------------------------------------------------------------
// Leg (e) — the forwarded-byte witness (AGENTS 1)
// ---------------------------------------------------------------------

/// The exact client bodies the scripted set sends (the byte witness
/// compares the mocks' recorded bytes against THESE strings, computed
/// once per turn so the cross-tree digest runs of R29-2's round
/// evidence reproduce the same set).
fn client_body(session: &str, turn: u32, stream: bool) -> String {
    let key = format!(r#","prompt_cache_key":"{session}""#);
    let opt = if stream {
        r#","stream_options":{"include_usage":true}"#
    } else {
        ""
    };
    format!(
        r#"{{"model":"p-plan/m1","messages":[{{"role":"user","content":"{}turn {turn}"}}]{key},"stream":{}{opt}}}"#,
        if stream { "stream " } else { "" },
        if stream { "true" } else { "false" }
    )
}

/// The two permitted mutations, computed as a relation on the mock's
/// own recorded bytes (never a snapshot): the recorded body equals
/// the client's body modulo (a) vadis-owned top-level fields removed
/// and (b) the top-level `model` value replaced by the destination's
/// provider-native id. This rig's clients send no vadis-owned field,
/// so the only legal difference is the model rewrite — asserted by
/// rewriting the expectation the same way and comparing bytes.
fn assert_forwarded_body(recorded: &[u8], client_body: &str) {
    let expected = client_body.replace("\"model\":\"p-plan/m1\"", "\"model\":\"m1\"");
    assert_eq!(
        recorded,
        expected.as_bytes(),
        "the upstream-visible body is the client's own bytes modulo the \
         two permitted mutations (AGENTS 1): mutation (b) rewrites the \
         top-level model value to the destination's native id (`m1` on \
         both mocks); nothing else may differ"
    );
}

/// Leg (e) over one rig's captured requests: every recorded request
/// body is the client's own body modulo the model rewrite for the
/// destination that mock serves.
fn assert_byte_witness(
    plan_reqs: &[RecordedRequest],
    api_reqs: &[RecordedRequest],
    plan_bodies: &[String],
    api_bodies: &[String],
) {
    assert_eq!(
        plan_reqs.len(),
        plan_bodies.len(),
        "the plan mock saw exactly the scripted requests"
    );
    assert_eq!(
        api_reqs.len(),
        api_bodies.len(),
        "the api mock saw exactly the scripted requests"
    );
    for (req, body) in plan_reqs.iter().zip(plan_bodies) {
        assert_forwarded_body(&req.body, body);
    }
    for (req, body) in api_reqs.iter().zip(api_bodies) {
        assert_forwarded_body(&req.body, body);
    }
}

// ---------------------------------------------------------------------
// Leg (a) — the buffered arm, the reference
// ---------------------------------------------------------------------

/// Leg (a): the already-correct medium, re-asserted as leg (b)'s
/// divisor. Green at `3e9d102` (no assertion of this leg is red
/// there); the leg exists to make (b)'s element-for-element
/// comparison meaningful.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_82_a_buffered_arm_classifies_the_quota_403_on_its_body() {
    let rig = rig("conf82-a").await;
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_forbidden_403());
    rig.api.queue(testkit::plan_ok("spilled"));
    assert_eq!(post(&rig, "S1", 1), 200, "turn 1 binds the primary");
    assert_eq!(post(&rig, "S1", 2), 200, "turn 2's 403 spills to overflow");
    // The demotion is live the moment the spill lands (the 1s
    // Retry-After) — the fact's second witness, read live.
    let h = http_get_health(&rig.listen_addr);
    assert_eq!(
        h["plan"]["probe"]["blocked_by"], "primary_cooling_down",
        "the provider demotion is live (the cooldown fact's live witness)"
    );
    // Leg (e)'s captures happen while the rig is live; the log reads
    // happen after stop (the writer lock is serve's).
    let plan_reqs = rig.plan.requests();
    let api_reqs = rig.api.requests();
    let plan_bodies = vec![client_body("S1", 1, false), client_body("S1", 2, false)];
    let api_bodies = vec![client_body("S1", 2, false)];
    let dir = rig.stop();

    let facts = spill_facts(&dir, "S1", 2, "quota_exhausted", "insufficient_quota", true);
    assert_eq!(facts.bound[0].2, TTL_US, "ttl_us is the configured 11h");
    assert_byte_witness(&plan_reqs, &api_reqs, &plan_bodies, &api_bodies);
}

// ---------------------------------------------------------------------
// Leg (b) — the streamed arm, the leg that runs red at 3e9d102
// ---------------------------------------------------------------------

/// Leg (b): the IDENTICAL canned bytes answer a streamed session —
/// turn 1 streamed and served on the plan, turn 2 streamed and
/// meeting the same quota-worded 403 (delivered on the streaming
/// wire, chunked, exactly as a real upstream answers a streamed
/// request it refuses). The same four facts must hold element for
/// element, and the two media's rows stripped to their comparable
/// fields must be equal. Red at `3e9d102`: `reason == "auth"`,
/// `demotion == null`, no cooldown, zero `plan.switched` rows,
/// one `session.bound` row instead of two — four facts, one cause
/// (the empty evidence body).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_82_b_streamed_arm_matches_the_buffered_element_for_element() {
    // The buffered reference, same scripted shape.
    let rig_b = rig("conf82-b-buf").await;
    rig_b.plan.queue(testkit::plan_ok("t1"));
    rig_b.plan.queue(testkit::plan_forbidden_403());
    rig_b.api.queue(testkit::plan_ok("spilled"));
    assert_eq!(post(&rig_b, "S1", 1), 200);
    assert_eq!(post(&rig_b, "S1", 2), 200, "buffered turn 2 spills");
    let h = http_get_health(&rig_b.listen_addr);
    assert_eq!(h["plan"]["probe"]["blocked_by"], "primary_cooling_down");
    let buf_plan_reqs = rig_b.plan.requests();
    let buf_api_reqs = rig_b.api.requests();
    let buf_plan_bodies = vec![client_body("S1", 1, false), client_body("S1", 2, false)];
    let buf_api_bodies = vec![client_body("S1", 2, false)];
    let dir_b = rig_b.stop();
    let buf_facts = spill_facts(
        &dir_b,
        "S1",
        2,
        "quota_exhausted",
        "insufficient_quota",
        true,
    );
    assert_byte_witness(
        &buf_plan_reqs,
        &buf_api_reqs,
        &buf_plan_bodies,
        &buf_api_bodies,
    );

    // The streamed run: the identical bytes answer a streamed session.
    let rig_s = rig("conf82-b-str").await;
    rig_s.plan.queue(sse_ok("t1"));
    rig_s
        .plan
        .queue(sse_status(403, "Forbidden", QUOTA_403_BODY, false));
    rig_s.api.queue(sse_ok("spilled"));
    let (s1, _b1) = post_stream(&rig_s, "S1", 1);
    assert_eq!(s1, 200, "turn 1 streamed and served");
    // The client-visible side of turn 2 — green on BOTH trees (the
    // reason the defect hid): the streamed turn is still served 200
    // from overflow with the overflow's SSE bytes verbatim.
    let (s2, b2) = post_stream(&rig_s, "S1", 2);
    assert_eq!(s2, 200, "turn 2 fails over and is served (either tree)");
    assert!(
        b2.windows(b"spilled".len()).any(|w| w == b"spilled"),
        "the overflow's own SSE bytes reach the client verbatim"
    );
    let h = http_get_health(&rig_s.listen_addr);
    assert_eq!(
        h["plan"]["probe"]["blocked_by"], "primary_cooling_down",
        "the demotion is live on the streamed medium too — the same \
         fact, witnessed live on the medium that used to miss it"
    );
    let str_plan_reqs = rig_s.plan.requests();
    let str_api_reqs = rig_s.api.requests();
    let str_plan_bodies = vec![client_body("S1", 1, true), client_body("S1", 2, true)];
    let str_api_bodies = vec![client_body("S1", 2, true)];
    let dir_s = rig_s.stop();
    let str_facts = spill_facts(
        &dir_s,
        "S1",
        2,
        "quota_exhausted",
        "insufficient_quota",
        true,
    );
    assert_byte_witness(
        &str_plan_reqs,
        &str_api_reqs,
        &str_plan_bodies,
        &str_api_bodies,
    );

    // The comparator: strip both arms' rows to their comparable
    // fields and assert equality.
    assert_eq!(
        (
            str_facts.reason.as_str(),
            str_facts.action.as_str(),
            str_facts.matched.as_str(),
            str_facts.demotion,
            str_facts.switch,
            str_facts.failover_from,
            str_facts.failover_reason.as_str(),
        ),
        (
            buf_facts.reason.as_str(),
            buf_facts.action.as_str(),
            buf_facts.matched.as_str(),
            buf_facts.demotion,
            buf_facts.switch,
            buf_facts.failover_from,
            buf_facts.failover_reason.as_str(),
        ),
        "the two media classify the identical upstream bytes identically"
    );
    let strip = |rows: &[(String, String, i64, Option<String>)]| {
        rows.iter()
            .map(|(p, m, ttl, _)| (p.clone(), m.clone(), *ttl))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        strip(&str_facts.bound),
        strip(&buf_facts.bound),
        "the same (provider, model, ttl_us) session.bound rows on both media"
    );
}

// ---------------------------------------------------------------------
// Leg (c) — the discriminant, both media, green at base and it must
// stay green
// ---------------------------------------------------------------------

/// One discriminant run on one medium: a 403 whose body lacks the
/// quota wording classifies `auth`, demotes nothing, moves nothing,
/// and the turn is still served 200 from overflow.
async fn discriminant(tag: &str, stream: bool) {
    let rig = rig(tag).await;
    if stream {
        rig.plan.queue(sse_ok("t1"));
        rig.plan
            .queue(sse_status(403, "Forbidden", AUTH_403_BODY, false));
        rig.api.queue(sse_ok("spilled"));
        let (s1, _b1) = post_stream(&rig, "S1", 1);
        assert_eq!(s1, 200);
        let (s2, _b2) = post_stream(&rig, "S1", 2);
        assert_eq!(s2, 200, "auth also fails over: the turn is served");
    } else {
        rig.plan.queue(testkit::plan_ok("t1"));
        rig.plan.queue(forbidden_403(AUTH_403_BODY));
        rig.api.queue(testkit::plan_ok("spilled"));
        assert_eq!(post(&rig, "S1", 1), 200);
        assert_eq!(post(&rig, "S1", 2), 200, "the turn is served from overflow");
    }
    let h = http_get_health(&rig.listen_addr);
    assert_eq!(
        h["plan"]["probe"]["blocked_by"],
        serde_json::Value::Null,
        "no demotion: an auth-class 403 demotes nothing (live witness)"
    );
    let dir = rig.stop();

    let facts = spill_facts(&dir, "S1", 2, "auth", "403", false);
    assert_eq!(
        facts.failover_from,
        serde_json::json!("p-plan/m1"),
        "the turn was served by failover from the primary"
    );
    let t2 = turn_record(&dir, "S1", 2);
    assert_eq!(
        t2["result"]["failover_from"],
        serde_json::json!("p-plan/m1")
    );
    assert_eq!(
        t2["result"]["plan_switch"],
        serde_json::Value::Null,
        "no account move: the plan policy did not displace this request"
    );
    assert_eq!(
        t2["decision"]["provider"], "p-api",
        "the family served the turn from overflow"
    );
}

/// Leg (c), buffered medium: green at `3e9d102` and at HEAD — the
/// assertion that keeps the fix from being a "stream 403 ⇒
/// exhaustion" special case.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_82_c_discriminant_403_without_quota_wording_moves_nothing_buffered() {
    discriminant("conf82-c-buf", false).await;
}

/// Leg (c), streamed medium: the same body on the streaming wire —
/// `auth`, no demotion, no switch, no re-point, on BOTH media.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_82_c_discriminant_403_without_quota_wording_moves_nothing_streamed() {
    discriminant("conf82-c-str", true).await;
}

// ---------------------------------------------------------------------
// Leg (d) — the read's edge, streamed only
// ---------------------------------------------------------------------

/// Leg (d), the partial body: the quota wording arrives in the head's
/// first body chunk and the connection then aborts — what arrived is
/// evidence, so the classification (and the account move) is the
/// complete-body verdict. Red at `3e9d102`: the pre-fix tree reads no
/// byte of it and classifies `auth`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_82_d_partial_body_is_evidence_the_stream_reads() {
    let rig = rig("conf82-d-par").await;
    rig.plan.queue(sse_ok("t1"));
    rig.plan
        .queue(sse_status(403, "Forbidden", QUOTA_403_BODY, true));
    rig.api.queue(sse_ok("spilled"));
    let (s1, _b1) = post_stream(&rig, "S1", 1);
    assert_eq!(s1, 200);
    let (s2, _b2) = post_stream(&rig, "S1", 2);
    assert_eq!(s2, 200, "the turn still fails over and is served");
    let h = http_get_health(&rig.listen_addr);
    assert_eq!(
        h["plan"]["probe"]["blocked_by"], "primary_cooling_down",
        "the demotion is live: the partial body moved the account"
    );
    let dir = rig.stop();
    let facts = spill_facts(&dir, "S1", 2, "quota_exhausted", "insufficient_quota", true);
    assert_eq!(
        facts.bound.len(),
        2,
        "the partial body moved the account exactly as a complete one would"
    );
}

/// Leg (d), the empty body: a failure head whose body delivers
/// nothing and aborts classifies exactly as the pre-fix tree does —
/// status and headers alone (`auth`), no demotion, no move. This IS
/// the E4 degradation rule's guard: a read that ends short is the
/// pre-fix verdict, not a new outcome class. Green at base by
/// construction.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_82_d_no_body_classifies_on_status_and_headers_alone() {
    let rig = rig("conf82-d-non").await;
    rig.plan.queue(sse_ok("t1"));
    rig.plan.queue(sse_status(403, "Forbidden", b"", true));
    rig.api.queue(sse_ok("spilled"));
    let (s1, _b1) = post_stream(&rig, "S1", 1);
    assert_eq!(s1, 200);
    let (s2, _b2) = post_stream(&rig, "S1", 2);
    assert_eq!(s2, 200, "auth fails over: the turn is served");
    let h = http_get_health(&rig.listen_addr);
    assert_eq!(
        h["plan"]["probe"]["blocked_by"],
        serde_json::Value::Null,
        "no demotion: no byte of a body arrived, and none was invented"
    );
    let dir = rig.stop();
    spill_facts(&dir, "S1", 2, "auth", "403", false);
}
