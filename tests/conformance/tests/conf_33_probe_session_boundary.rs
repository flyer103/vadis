//! CONF-33 (spec §4.6 hard rule 2 / ADR-014 item 3): **the probe lives at
//! the session boundary and nowhere else.** The two directions of the same
//! rule, made decisive against each other:
//!
//! - a session already on the overflow account does **not** probe mid-session
//!   (`turn_index == 2`): its next request goes where the state says, even
//!   with `cooldown: 0s` and the primary healthy again — the zero cooldown
//!   removes "the cooldown has not elapsed" as an alternative explanation;
//! - after the cooldown (0s here) **and** the ADR-011 provider demotion left
//!   by the 403 (shrunk to 1s via `Retry-After`) have passed, a **new**
//!   session's first request (`turn_index == 1`) is admitted as a probe on
//!   the primary; its success flips the family back and records
//!   `plan.switched { reason: primary_recovered, probe: true }`, and the
//!   already-spilled session is pulled back with it.

#![forbid(unsafe_code)]

use router_conformance::testkit::{self, PlanRig};

async fn rig(tag: &str) -> PlanRig {
    let (plan, api, dir, listen_addr) =
        testkit::plan_rig_parts(tag, "", testkit::PLAN_POLICY_DEFAULT).await;
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    PlanRig {
        plan,
        api,
        listen_addr,
        dir,
        serve_task,
    }
}

fn events(dir: &std::path::Path) -> Vec<(String, serde_json::Value)> {
    let store = router_store::SqliteStore::open(&dir.join("state/router.db")).unwrap();
    use router_core::store::{Query, QueryRow, Store as _};
    let QueryRow::Events(rows) = store.query(Query::AllEvents).unwrap() else {
        panic!("events query");
    };
    rows.into_iter().map(|r| (r.kind_raw, r.payload)).collect()
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

/// Rule 3 (mid-session never probes): a session that spilled on turn 2 is
/// served by the overflow account on turn 3 — the primary mock receives
/// nothing — although `cooldown: 0s` and the primary is answering 200s
/// again. The only reason it does not probe is `turn_index != 1`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_33_mid_session_never_probes() {
    let rig = rig("conf33-mid").await;
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_forbidden_403());
    // Would answer 200 if (wrongly) probed — the count would betray it.
    rig.plan.queue(testkit::plan_ok("recovered"));
    rig.api.queue(testkit::plan_ok("spilled"));

    let (s1, _b, _h) = rig.post(Some("S1"), 1);
    assert_eq!(s1, 200);
    let (s2, _b, _h) = rig.post(Some("S1"), 2);
    assert_eq!(s2, 200, "turn 2 spills to the metered account");
    let p_before = rig.plan.requests().len();

    let (s3, _b3, _h3) = rig.post(Some("S1"), 3);
    assert_eq!(s3, 200, "turn 3 is still served (by the overflow account)");
    assert_eq!(
        rig.plan.requests().len(),
        p_before,
        "the primary mock received NOTHING for the mid-session turn: \
         a probe happens only at a session boundary, and cooldown is 0s \
         so the cooldown cannot be the reason"
    );
    assert_eq!(
        rig.api.requests().len(),
        2,
        "the state routes turn 3 to overflow"
    );

    rig.stop();
}

/// Rule 4 (the session boundary is probeable): after the cooldown (0s) and
/// the 403's ADR-011 demotion (1s, via `Retry-After`) have both passed, a
/// new session's first request is admitted as a probe on the primary, its
/// success records `plan.switched { probe: true }` back to primary, and the
/// already-spilled session is pulled back on its next turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_33_new_session_first_turn_probes_and_pulls_the_family_back() {
    let rig = rig("conf33-probe").await;
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_forbidden_403());
    rig.plan.queue(testkit::plan_ok("probe-wins"));
    rig.plan.queue(testkit::plan_ok("s1-back"));
    rig.api.queue(testkit::plan_ok("spilled"));

    let (s1, _b, _h) = rig.post(Some("S1"), 1);
    assert_eq!(s1, 200);
    let (s2, _b, _h) = rig.post(Some("S1"), 2);
    assert_eq!(s2, 200, "turn 2 spills");
    assert_eq!(rig.api.requests().len(), 1);

    // Let the 403's provider demotion (Retry-After: 1) expire; the family's
    // own cooldown is already 0s. What remains is the probe predicate.
    tokio::time::sleep(std::time::Duration::from_millis(1300)).await;

    // A NEW session's first turn: the probe.
    let (sp, _bp, _hp) = rig.post(Some("S2"), 1);
    assert_eq!(sp, 200, "the probe is served by the primary");
    assert_eq!(
        rig.plan.requests().len(),
        3,
        "the probe reached the primary mock"
    );
    assert_eq!(
        rig.api.requests().len(),
        1,
        "the probe spent nothing metered"
    );

    // The spilled session is pulled back with the family (its binding was
    // re-pointed by the recovery transition).
    let (s3, _b3, _h3) = rig.post(Some("S1"), 3);
    assert_eq!(s3, 200);
    assert_eq!(
        rig.plan.requests().len(),
        4,
        "S1's next turn is back on the primary account"
    );
    assert_eq!(rig.api.requests().len(), 1, "no further metered attempts");

    let dir = rig.stop();

    // Two transitions, opposite directions; the return carries probe: true.
    let evs = events(&dir);
    let switches: Vec<&(String, serde_json::Value)> =
        evs.iter().filter(|(k, _)| k == "plan.switched").collect();
    assert_eq!(switches.len(), 2, "one row per transition");
    let (_k0, p0) = switches[0];
    assert_eq!(p0["reason"], "primary_exhausted");
    assert_eq!(p0["probe"], false);
    let (_k1, p1) = switches[1];
    assert_eq!(p1["from_account"], "overflow");
    assert_eq!(p1["to_account"], "primary");
    assert_eq!(p1["reason"], "primary_recovered");
    assert_eq!(p1["probe"], true, "the probe's success IS the transition");
    assert_eq!(
        p1["switch_cost_nano"], 0,
        "the way back costs 0: an in-plan destination's marginal price is 0"
    );

    // The probe request's own trace: served by the plan, nothing failed, no
    // displacement markers (the record of the move is the event + the state).
    let probe_rec = trace_records(&dir)
        .into_iter()
        .find(|r| r["identity"]["session"] == "S2" && r["identity"]["turn_index"] == 1)
        .expect("the probe's record");
    assert_eq!(probe_rec["decision"]["provider"], "p-plan");
    assert_eq!(
        probe_rec["result"]["failover_from"],
        serde_json::Value::Null
    );
    // And the pulled-back session's next turn is on the plan account too.
    // (Its `turn_index` is larger than the request count: each account move
    // re-points the binding with a `session.bound` write, and the counter
    // rides that write — the spec's own definition reads the projection.)
    let mut s1_records: Vec<serde_json::Value> = trace_records(&dir)
        .into_iter()
        .filter(|r| r["identity"]["session"] == "S1")
        .collect();
    s1_records.sort_by_key(|r| r["identity"]["turn_index"].as_u64().unwrap_or(0));
    let back_rec = s1_records.last().expect("S1's pulled-back record");
    assert!(
        back_rec["identity"]["turn_index"].as_u64().unwrap_or(0) > 2,
        "this is a later turn, not a rewrite of turn 2"
    );
    assert_eq!(back_rec["decision"]["provider"], "p-plan");
}
