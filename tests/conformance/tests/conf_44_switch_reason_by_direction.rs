//! CONF-44 (spec §6's producer table row ii / R5-5's F2): **a
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

/// Every stored event as (kind_raw, payload), read after the server stopped.
fn events(dir: &std::path::Path) -> Vec<(String, serde_json::Value)> {
    let store = router_store::SqliteStore::open(&dir.join("state/router.db")).unwrap();
    use router_core::store::{Query, QueryRow, Store as _};
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
    assert_eq!(rig.api.requests().len(), 2, "the overflow served turns 2 and 3");

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
    assert_eq!(turn2["result"]["plan_switch"]["reason"], "primary_exhausted");
    assert_eq!(turn2["result"]["plan_switch"]["probe"], false);
}
