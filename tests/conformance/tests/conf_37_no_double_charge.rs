//! CONF-37 (spec §4.6 / ADR-014 item 5): **the switch path charges the
//! allowance exactly once.** A turn that 403s on the primary and is served
//! by the overflow account walks the whole candidate chain, but the
//! request's usage is real work done once — `quota.charged` may appear for
//! the p-plan window at most once, and the overflow account's spend (the
//! `cost.computed` rows the cap sums) counts the single served response,
//! not one per attempt.
//!
//! Asserted through the event log's payload projections (the same rows
//! `vadis stats` reads), not an internal builder.

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, PlanRig};
use vadis_core::store::{Query, QueryRow, Store as _};

async fn rig(tag: &str) -> PlanRig {
    // p-plan declares a quota plan (generous — the allowance itself is not
    // under test here) so quota.charged rows exist to count. A request with
    // no declared plan charges no allowance row at all, and this case is
    // about the *count* on the switch path.
    let quota = "\n    quota:\n      - models: [m1]\n        window: monthly\n        tokens: 100000\n        reset_day: 1\n        over_quota: block\n        source: \"fixture: generous allowance for charge counting\"";
    let (plan, api, dir, listen_addr) =
        testkit::plan_rig_parts(tag, quota, testkit::PLAN_POLICY_DEFAULT).await;
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

fn events(dir: &std::path::Path) -> Vec<(String, serde_json::Value)> {
    let store = vadis_store::SqliteStore::open(&dir.join("state/router.db")).unwrap();
    let QueryRow::Events(rows) = store.query(Query::AllEvents).unwrap() else {
        panic!("events query");
    };
    rows.into_iter().map(|r| (r.kind_raw, r.payload)).collect()
}

/// The spill turn walks two upstreams (the 403 attempt, then the served
/// overflow attempt) yet books one charge for the work actually done: no
/// quota.charged row at all (the 403 attempt did no billable work), and
/// exactly one cost.computed row per request — the metered response's own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_37_the_switch_path_charges_the_allowance_once() {
    let rig = rig("conf37-once").await;
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_forbidden_403());
    rig.api.queue(testkit::plan_ok("spilled"));

    let (s1, _b, _h) = rig.post(Some("S1"), 1);
    assert_eq!(s1, 200);
    let (s2, _b2, _h2) = rig.post(Some("S1"), 2);
    assert_eq!(s2, 200);
    assert_eq!(rig.plan.requests().len(), 2);
    assert_eq!(rig.api.requests().len(), 1);

    let dir = rig.stop();
    let evs = events(&dir);

    // The switch path itself: the intent rows for both attempts, the
    // classified 403, the plan switch, the failover — the shape ADR-014
    // item 8 / ADR-011 describe, once each.
    let count = |k: &str| evs.iter().filter(|(kind, _)| kind == k).count();
    assert_eq!(
        count("upstream.submitted"),
        3,
        "two turns: 1 + the spill's 2 attempts"
    );
    assert_eq!(count("error.classified"), 1, "the 403, classified once");
    assert_eq!(count("plan.switched"), 1, "one account move");
    assert_eq!(count("failover.triggered"), 1, "one failover record");

    // The allowance: only plan-account requests charge the plan's window.
    // Turn 1 (served on p-plan) charges once; the 403 attempt did no
    // billable work; the spill turn is served by p-api, which declares no
    // allowance — so exactly one quota.charged row exists for the whole
    // exchange. A second row would mean the switch path double-charged.
    let charges: Vec<&serde_json::Value> = evs
        .iter()
        .filter(|(kind, _)| kind == "quota.charged")
        .map(|(_, p)| p)
        .collect();
    assert_eq!(
        charges.len(),
        1,
        "one charge for one plan-account request — the 403 attempt and the \
         metered response charge the plan's window nothing"
    );
    assert_eq!(charges[0]["provider"], "p-plan");
    assert_eq!(
        charges[0]["tokens"], 105,
        "the served response's tokens, once"
    );

    // The metered spend (what overflow_monthly_cap_usd sums): one
    // cost.computed row for the overflow route, carrying the single
    // response's total. Two turns -> two cost.computed rows total, one per
    // request; the spill turn's row is on the overflow route.
    let costs: Vec<&serde_json::Value> = evs
        .iter()
        .filter(|(kind, _)| kind == "cost.computed")
        .map(|(_, p)| p)
        .collect();
    assert_eq!(costs.len(), 2, "one cost row per served request");
    let overflow_costs: Vec<&serde_json::Value> = costs
        .iter()
        .filter(|p| p["route"] == "p-api/m1")
        .copied()
        .collect();
    assert_eq!(
        overflow_costs.len(),
        1,
        "the spill response is costed exactly once"
    );
    assert_eq!(
        overflow_costs[0]["total_nano"], 184_000,
        "80 miss x 2000 + 20 hit x 200 + 5 out x 4000 nano — the single response"
    );
}
