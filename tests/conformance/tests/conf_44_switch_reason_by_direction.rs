//! CONF-44 (spec §6's producer table row ii): **a
//! state-driven displacement's `plan_switch.reason` is decided by
//! direction — the destination account — never by the account state the
//! guard read before the request.**
//!
//! The produced case is the *third* turn of a spilled family: the state
//! is already `overflow`, the request is an ordinary one (nothing failed
//! in it, no probe admitted — mid-session), and the guard sends it where
//! the state says: the overflow route. Spec §6's producer table allows
//! exactly one label for a move whose destination is the overflow
//! account — `primary_exhausted`. The ternary this case pinned decided
//! by `state_before.account`, which in this arm can only ever answer
//! `primary_recovered`: a reason the contract pairs with a move *to*
//! the primary and nothing else.
//!
//! Both directions are witnessed in one run: the spill round itself
//! (turn 2 — the request whose 403 moved the account) keeps
//! `primary_exhausted` (CONF-32's assertion, coexisting here so the two
//! rows cannot drift), and the turn-3 displacement must say
//! `primary_exhausted` too, with `failover_from` null (nothing failed in
//! the request) and `probe: false` (no probe was admitted).

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, PlanRig};
use vadis_core::config::{
    CapUsdVal, DurationVal, OnPrimaryExhausted, PlanPolicyCfg, RecoveryMode, RouteSpec,
};
use vadis_core::cost::Nano;
use vadis_core::plan::{PlanAccount, PlanFirstRule, PlanRequest, PlanStateRow};

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

/// Every stored event as (kind_raw, payload), read after the server stopped.
fn events(dir: &std::path::Path) -> Vec<(String, serde_json::Value)> {
    let store = vadis_store::SqliteStore::open(&dir.join("state/router.db")).unwrap();
    use vadis_core::store::{Query, QueryRow, Store as _};
    let QueryRow::Events(rows) = store.query(Query::AllEvents).unwrap() else {
        panic!("events query");
    };
    rows.into_iter().map(|r| (r.kind_raw, r.payload)).collect()
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

/// The third turn of a spilled family is an ordinary request: the state
/// (already `overflow`) displaces it to the overflow route, and the
/// record must name the move by its destination — `primary_exhausted`,
/// never `primary_recovered` (a reason only a move *to* the primary may
/// carry). The spill round's own record keeps the same reason, so both
/// producer rows of §6 are witnessed against each other.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_44_turn3_displacement_reason_is_decided_by_destination() {
    let rig = rig("conf44-turn3").await;
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_forbidden_403());
    rig.api.queue(testkit::plan_ok("spilled"));
    rig.api.queue(testkit::plan_ok("spilled-2"));

    let (s1, _b, _h) = rig.post(Some("S1"), 1);
    assert_eq!(s1, 200);
    let (s2, _b2, _h2) = rig.post(Some("S1"), 2);
    assert_eq!(s2, 200, "turn 2 spills to the metered account");
    let (s3, _b3, _h3) = rig.post(Some("S1"), 3);
    assert_eq!(s3, 200, "turn 3 is still served (by the overflow account)");
    assert_eq!(rig.plan.requests().len(), 2, "turn 1 and the 403 only");
    assert_eq!(
        rig.api.requests().len(),
        2,
        "the overflow served turns 2 and 3"
    );

    let dir = rig.stop();

    // One transition only: the spill. The turn-3 displacement is
    // state-driven (nothing failed), so `plan.switched`'s only writer
    // never ran for it.
    let evs = events(&dir);
    let switches: Vec<&(String, serde_json::Value)> =
        evs.iter().filter(|(k, _)| k == "plan.switched").collect();
    assert_eq!(switches.len(), 1, "exactly one transition (the spill)");
    assert_eq!(switches[0].1["reason"], "primary_exhausted");

    // The turn-3 record: the displacement, named by its destination.
    // (The turn index of a spilled session's later turns can exceed the
    // request count — each account move re-points the binding and the
    // counter rides that write — so this is "the last S1 record", the
    // same reading rule spec §6 itself uses.)
    let mut s1_records: Vec<serde_json::Value> = trace_records(&dir)
        .into_iter()
        .filter(|r| r["identity"]["session"] == "S1")
        .collect();
    s1_records.sort_by_key(|r| r["identity"]["turn_index"].as_u64().unwrap_or(0));
    let turn3 = s1_records.last().expect("the turn-3 record");
    assert!(
        turn3["identity"]["turn_index"].as_u64().unwrap_or(0) > 2,
        "this is a later turn, not a rewrite of turn 2"
    );
    assert_eq!(turn3["decision"]["provider"], "p-api");
    assert_eq!(
        turn3["result"]["failover_from"],
        serde_json::Value::Null,
        "nothing failed in this request (spec §6 row ii)"
    );
    let ps = &turn3["result"]["plan_switch"];
    assert_eq!(
        ps["from"], "p-plan/m1",
        "the record exists: the state displaced the resolved primary"
    );
    assert_eq!(ps["to"], "p-api/m1");
    assert_eq!(
        ps["reason"], "primary_exhausted",
        "a move whose destination is the overflow account is an exhaustion \
         displacement — `primary_recovered` names a move TO the primary and \
         nothing else (spec §6's producer table)"
    );
    assert_eq!(ps["probe"], false, "mid-session: no probe was admitted");

    // The spill round's own record keeps its label (CONF-32's assertion,
    // coexisting here): the two rows are the double-direction evidence.
    let turn2 = s1_records
        .iter()
        .find(|r| r["identity"]["turn_index"] == 2)
        .expect("the turn-2 record");
    assert_eq!(turn2["decision"]["provider"], "p-api");
    assert_eq!(
        turn2["result"]["plan_switch"]["reason"],
        "primary_exhausted"
    );
    assert_eq!(turn2["result"]["plan_switch"]["probe"], false);
}

/// The probe's return trip is the probing request's OWN displacement
/// record (spec §6: "the family's account state returning to `primary` |
/// `failover_from`: null | `plan_switch`: set, `reason:
/// primary_recovered`"; §6's `probe` field IS "this switch was the return
/// trip of an admitted probe"). The event and the state were already
/// written by the recovery transition; this pins that the trace — the
/// product's only observation channel (ADR-005) and `vadis stats`'
/// source (§9.2) — carries it too. The old guard only recorded a
/// displacement when `g.route != primary`, and an admitted probe moves
/// the request TO the primary, so the return trip never landed.
///
/// Coexistence is asserted both ways: exactly one `overflow → primary`
/// `plan.switched` row (the fix must add the trace row, not delete the
/// event), and neither the spill row nor any displacement carries
/// `probe: true` (the negative controls; CONF-42 pins the same for
/// `primary_cooling_down`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_44_probe_return_trip_is_recorded_in_the_trace() {
    let rig = rig("conf44-probe").await;
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_forbidden_403());
    rig.plan.queue(testkit::plan_ok("probe-wins"));
    rig.api.queue(testkit::plan_ok("spilled"));

    let (s1, _b, _h) = rig.post(Some("S1"), 1);
    assert_eq!(s1, 200);
    let (s2, _b2, _h2) = rig.post(Some("S1"), 2);
    assert_eq!(s2, 200, "turn 2 spills to the metered account");

    // Let the 403's ADR-011 provider demotion (Retry-After: 1) expire;
    // the family's own cooldown is already 0s (CONF-33's recipe).
    tokio::time::sleep(std::time::Duration::from_millis(1300)).await;

    // A NEW session's first turn: the admitted probe, served by the
    // primary — the request whose success flips the family back.
    let (sp, _bp, _hp) = rig.post(Some("S2"), 1);
    assert_eq!(sp, 200, "the probe is served by the primary");
    assert_eq!(
        rig.plan.requests().len(),
        3,
        "the probe reached the primary"
    );

    let dir = rig.stop();

    // Exactly one recovery row in the event log (plus the spill): the
    // trace fix must not have come from removing the event.
    let evs = events(&dir);
    let switches: Vec<&(String, serde_json::Value)> =
        evs.iter().filter(|(k, _)| k == "plan.switched").collect();
    assert_eq!(
        switches.len(),
        2,
        "the spill and the recovery, nothing else"
    );
    let back = switches
        .iter()
        .find(|(_, p)| p["to_account"] == "primary")
        .expect("the recovery row");
    assert_eq!(back.1["from_account"], "overflow");
    assert_eq!(back.1["reason"], "primary_recovered");
    assert_eq!(
        back.1["probe"], true,
        "the probe's success IS the transition"
    );
    let spill = switches
        .iter()
        .find(|(_, p)| p["to_account"] == "overflow")
        .expect("the spill row");
    assert_eq!(
        spill.1["probe"], false,
        "negative control: the spill row is not a probe"
    );

    // The probe request's own trace record: the return trip, labelled by
    // direction with the probe flag set.
    let probe_rec = trace_records(&dir)
        .into_iter()
        .find(|r| r["identity"]["session"] == "S2" && r["identity"]["turn_index"] == 1)
        .expect("the probe's record");
    assert_eq!(probe_rec["decision"]["provider"], "p-plan");
    assert_eq!(
        probe_rec["result"]["failover_from"],
        serde_json::Value::Null,
        "nothing failed in the probe request (spec §6)"
    );
    let ps = &probe_rec["result"]["plan_switch"];
    assert_eq!(
        ps["from"], "p-api/m1",
        "the record exists: the probe displaced the state's overflow route"
    );
    assert_eq!(ps["to"], "p-plan/m1");
    assert_eq!(ps["reason"], "primary_recovered");
    assert_eq!(
        ps["probe"], true,
        "spec §6: this switch was the return trip"
    );
    assert_eq!(
        ps["switch_cost_nano"], 0,
        "the way back costs 0: an in-plan destination's marginal price is 0"
    );

    // The reporting surface follows (§9.2): `switches` counts requests
    // whose `result.plan_switch` is present, so the return trip must be
    // counted — verified against the run's own trace rows.
    let rep = vadis_cli::stats::report(&dir.join("config.yaml").to_string_lossy(), "24h")
        .expect("report computes");
    let recs = trace_records(&dir);
    let trace_switches = recs
        .iter()
        .filter(|r| {
            !r["result"]["plan_switch"].is_null() && r["usage_missing"] != serde_json::json!(true)
        })
        .count() as u64;
    assert!(
        trace_switches >= 2,
        "both the spill and the return trip carry a plan_switch record"
    );
    assert_eq!(
        rep.figures.switches, trace_switches,
        "stats counts the probe's return trip too"
    );
    assert_eq!(rep.figures.switches_without_usage, 0);
}

/// The guard and the surface judge the SAME clock: a request
/// arriving just past the deadline must be admitted the way `/health`
/// says it is. The guard used to rebuild its instant from the truncated
/// whole-seconds word (`now_epoch_s * 1e6`), so for up to ~1s after the
/// true deadline the surface reported `admitted: true` while the guard
/// still answered `Cooldown` (the clock-granularity flake). The window is driven
/// deterministically: the 403 carries `Retry-After: 0` (the demotion
/// expires the moment it is written) and the family's cooldown is 0s,
/// so the deadline IS the spill's own µs timestamp — and the boundary
/// request is posted after it, which a µs guard must always admit.
///
/// R58 — the discrimination moved off the wall clock (the R56-1 load
/// red at :328 — the self-diagnosing "indeterminate: the second
/// boundary was crossed; re-run" abort, red on byte-identical input,
/// the same class R56 fixed in conf_72/conf_73). The old text sampled
/// the same-second window *probabilistically*: the live arm could tell
/// a second-granular guard from the µs one only while the boundary
/// request landed in the spill's whole second, so it aborted red
/// whenever 700ms of scheduling had passed. The fixed case splits the
/// two properties the arm conflated:
///
/// - **The µs-word discrimination** is pinned UNCONDITIONALLY by pure
///   `PlanFirstRule` calls at instants the test controls (below): with
///   the deadline at a mid-second instant `S`, the gate refuses at
///   `S − 1µs`, lifts exactly at `S`, and — the decisive row — admits
///   at `S + 1µs`, an instant a second-granular guard computes as
///   `S − 500ms` and refuses. No wall clock, no scheduling, fires on
///   every run.
/// - **The live agreement** (the surface's word and the guard's word
///   for the same configuration) needs no window at all: with a
///   zero-length deadline EVERY instant after the spill judges alike,
///   so "the request is admitted the way the surface said" holds
///   unconditionally — the live arm now asserts it on every run, where
///   the old text asserted nothing at all on the runs it aborted.
///
/// The replacement is strictly stronger per run, and the caller-side
/// regression it was written for (a guard clock fed by the truncated
/// seconds word) still reds the live arm on any run whose boundary
/// request lands in the spill's whole second — demonstrated by the
/// sabotage control captured when this case was made.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_44_guard_and_surface_judge_the_same_instant() {
    // The deterministic pin (no wall clock): with a zero-length
    // cooldown the deadline IS the spill's own µs timestamp. `S` is
    // mid-second on purpose — 2027-01-15T08:00:00.500000Z: a guard
    // whose instant is rebuilt from the truncated whole-seconds word
    // computes now' = S − 500_000µs < S at now = S + 1µs and REFUSES;
    // the µs guard admits. This is the same-second discrimination the
    // live arm below can only sample; here it fires on every run.
    let p = policy();
    const S: i64 = 1_800_000_000_500_000;
    assert_eq!(
        guard_word(&p, S, S - 1, true),
        Some("cooldown".into()),
        "strictly before the zero-length deadline the cooldown arm stands \
         (the deadline IS the spill's own µs timestamp)"
    );
    assert_eq!(
        guard_word(&p, S, S, true),
        None,
        "the gate lifts exactly at the deadline"
    );
    assert_eq!(
        guard_word(&p, S, S + 1, true),
        None,
        "1µs past the deadline, still inside the same whole second: the µs \
         word admits — a second-granular guard refuses here"
    );
    assert_eq!(
        guard_word(&p, S, S + 1, false),
        Some("primary_cooling_down".into()),
        "with the demotion live the same instant refuses: the admission \
         depends on the demotion being dead, not on a toothless gate"
    );

    let rig = rig("conf44-clock").await;
    rig.plan.queue(testkit::plan_ok("t1"));
    // The spill's 403 with a ZERO Retry-After: the provider demotion
    // dies immediately, so the only thing that can block the next probe
    // is the guard's own clock.
    rig.plan.queue(
        vadis_conformance::testkit::CannedResponse::json(
            403,
            "Forbidden",
            br#"{"error":{"message":"You have exceeded your current quota","type":"insufficient_quota","code":"insufficient_quota"}}"#,
        )
        .with_header("retry-after", "0"),
    );
    rig.plan.queue(testkit::plan_ok("probe-wins"));
    rig.api.queue(testkit::plan_ok("spilled"));

    let (s1, _b, _h) = rig.post(Some("S1"), 1);
    assert_eq!(s1, 200);
    let (s2, _b2, _h2) = rig.post(Some("S1"), 2);
    assert_eq!(s2, 200, "turn 2 spills to the metered account");

    // The surface's verdict at its own µs clock, taken immediately: the
    // family is on overflow and nothing blocks the probe anymore.
    // Timing-safe by construction — the zero-TTL demotion died at its
    // write instant and the family cooldown is 0s, so at ANY instant
    // after the spill the surface admits.
    let h = http_get(&rig.listen_addr, "/health");
    assert_eq!(h["plan"]["account"], "overflow");
    assert_eq!(
        h["plan"]["probe"]["admitted"], true,
        "cooldown 0s and a zero-TTL demotion: the surface admits"
    );
    assert_eq!(h["plan"]["probe"]["blocked_by"], serde_json::Value::Null);

    // The boundary request: the guard must admit the way the surface
    // did. No window is sampled — for this configuration (the deadline
    // IS the spill's own instant) every later instant judges alike, so
    // the agreement holds on every run, loaded or not.
    let (sp, _bp, _hp) = rig.post(Some("S2"), 1);
    assert_eq!(sp, 200);
    assert_eq!(
        rig.plan.requests().len(),
        3,
        "the request was admitted the way the surface said: it probed \
         the primary, whose 200 flips the family back"
    );

    let dir = rig.stop();
    let evs = events(&dir);
    let switches: Vec<&(String, serde_json::Value)> =
        evs.iter().filter(|(k, _)| k == "plan.switched").collect();
    assert_eq!(switches.len(), 2, "spill + recovery");
    assert_eq!(switches[1].1["to_account"], "primary");
    assert_eq!(switches[1].1["reason"], "primary_recovered");
    assert_eq!(switches[1].1["probe"], true);
}

/// This case's plan policy as a value (PLAN_POLICY_DEFAULT: spill,
/// probe recovery, cooldown 0s) — for the pure `PlanFirstRule` pins.
fn policy() -> PlanPolicyCfg {
    PlanPolicyCfg {
        family: "m1".into(),
        primary: RouteSpec {
            provider: "p-plan".into(),
            model: "m1".into(),
        },
        overflow: RouteSpec {
            provider: "p-api".into(),
            model: "m1".into(),
        },
        on_primary_exhausted: OnPrimaryExhausted::Spill,
        recover: RecoveryMode::Probe,
        cooldown: DurationVal(0),
        overflow_monthly_cap_usd: Some(CapUsdVal(20.0)),
    }
}

/// The guard's word for a boundary request at `now_us` (independent
/// evaluation — the same `PlanFirstRule` type the request path runs,
/// called here as a pure function of test-controlled instants).
/// `primary_allowed` is the availability input: `true` keeps the
/// ADR-011 demotion arm out of the call, so the zero-length gate is
/// exercised unmasked (CONF-72's helper shape).
fn guard_word(
    p: &PlanPolicyCfg,
    since_us: i64,
    now_us: i64,
    primary_allowed: bool,
) -> Option<String> {
    let rule = PlanFirstRule::new(p.clone());
    let req = PlanRequest {
        session: Some("conf-44"),
        turn_index: 1,
        state: PlanStateRow {
            account: PlanAccount::Overflow,
            since_us,
        },
        now_us,
        primary_allowed,
        deferred_by_window: false,
        overflow_spend: Nano(0),
    };
    rule.probe_admitted(&req)
        .err()
        .and_then(|e| e.blocked_by_surface_word())
        .map(str::to_string)
}

/// Minimal blocking GET (the CONF-25/41 style).
fn http_get(addr: &str, path: &str) -> serde_json::Value {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(addr).expect("connect");
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    let text = String::from_utf8_lossy(&buf).into_owned();
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("");
    serde_json::from_str(body.trim()).expect("health json")
}
