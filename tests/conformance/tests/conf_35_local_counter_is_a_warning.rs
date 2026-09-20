//! CONF-35 (spec §4.6 hard rule 3 / ADR-014 item 2, GAP-Q16): **the local
//! counter is a warning, never the authority.** Its denominator
//! (`quota.tokens`) may be an operator placeholder (GAP-Q1), so it may
//! neither `Reject` a request nor force a spill, and its one honest use is
//! deferring a probe until the plan's declared window boundary.
//!
//! The fixture gives p-plan a plan of exactly one request's chargeable
//! tokens (100 input + 5 output = 105) with `over_quota: block`, so after
//! one served request the local counter reads exhausted — while the mock
//! upstream keeps answering 200s, proving the two authorities apart.

#![forbid(unsafe_code)]

use router_conformance::testkit::{self, PlanRig};

/// A plan whose allowance is exactly one request (105 chargeable tokens),
/// refusing on exhaustion — the most aggressive local verdict available.
const QUOTA_ONE_REQUEST: &str = "\n    quota:\n      - models: [m1]\n        window: monthly\n        tokens: 105\n        reset_day: 1\n        over_quota: block\n        source: \"fixture: exactly one request's chargeable tokens\"";

async fn rig(tag: &str) -> PlanRig {
    let (plan, api, dir, listen_addr) =
        testkit::plan_rig_parts(tag, QUOTA_ONE_REQUEST, testkit::PLAN_POLICY_DEFAULT).await;
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

/// The counter never refuses and never forces a spill: after it reads
/// exhausted (105/105, `over_quota: block`), the next request is **still
/// served by the primary** with a 200 — the upstream said nothing wrong —
/// and the exhaustion is visible only as the recorded warning
/// (`cost.quota_after.verdict: "blocked"` on a successful record).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_35_local_counter_neither_rejects_nor_forces_a_spill() {
    let rig = rig("conf35-warn").await;
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_ok("t2-despite-counter"));
    rig.api.queue(testkit::plan_ok("never-needed"));

    let (s1, _b, _h) = rig.post(Some("S1"), 1);
    assert_eq!(s1, 200);
    assert_eq!(rig.plan.requests().len(), 1);

    // The counter now reads 105/105 with over_quota: block — the most
    // aggressive local verdict there is.
    let (s2, _b2, _h2) = rig.post(Some("S1"), 2);
    assert_eq!(
        s2, 200,
        "no 429: a placeholder allowance may not refuse work (GAP-Q16)"
    );
    assert_eq!(
        rig.plan.requests().len(),
        2,
        "the request STILL went to the primary: the counter may not force a spill"
    );
    assert_eq!(
        rig.api.requests().len(),
        0,
        "the metered account was never touched"
    );

    let dir = rig.stop();

    // The warning is recorded, not acted on: turn 2's own successful record
    // carries the exhausted local verdict.
    let records = trace_records(&dir);
    let turn = |n: u32| {
        records
            .iter()
            .find(|r| r["identity"]["session"] == "S1" && r["identity"]["turn_index"] == n)
            .unwrap_or_else(|| panic!("turn {n} record"))
            .clone()
    };
    let rec = turn(2);
    let qa = &rec["cost"]["quota_after"];
    assert_eq!(qa["verdict"], "blocked", "the local verdict is visible");
    assert_eq!(qa["tokens_used"], 105);
    assert_eq!(qa["tokens_limit"], 105);
    assert_eq!(rec["result"]["status"], 200, "…on a request that succeeded");

    // Flip witness for the SessionBinding SQL fix (fa9f078): before it,
    // every same-session record read sticky_hit: false because the read
    // errored on 'expires_at_usFROM sessions' — turn 2 of a bound session
    // must be a sticky hit.
    assert_eq!(turn(1)["state"]["sticky_hit"], false, "turn 1 binds");
    assert_eq!(
        turn(2)["state"]["sticky_hit"],
        true,
        "turn 2 hits the binding"
    );
}

/// The counter's one honest power (spec §4.6 rule 3): it may **defer the
/// probe** until the plan's declared window boundary. With the counter
/// exhausted and the window (monthly, reset_day 1) not yet passed, a new
/// session's first turn — otherwise a perfectly admitted probe — stays on
/// the overflow account; the request itself is still served.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_35_exhausted_counter_defers_the_probe_not_the_request() {
    let rig = rig("conf35-defer").await;
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_forbidden_403());
    // Would answer 200 if (wrongly) probed — the count would betray it.
    rig.plan.queue(testkit::plan_ok("recovered"));
    rig.api.queue(testkit::plan_ok("spilled"));
    rig.api.queue(testkit::plan_ok("s2-deferred"));

    let (s1, _b, _h) = rig.post(Some("S1"), 1);
    assert_eq!(s1, 200); // charges 105/105: the counter is exhausted
    let (s2, _b2, _h2) = rig.post(Some("S1"), 2);
    assert_eq!(s2, 200); // the upstream's 403 spills the family
    assert_eq!(rig.api.requests().len(), 1);

    // Past every clock the policy owns: cooldown 0s, demotion 1s.
    tokio::time::sleep(std::time::Duration::from_millis(1300)).await;

    let p_before = rig.plan.requests().len();
    let (sp, _bp, _hp) = rig.post(Some("S2"), 1);
    assert_eq!(sp, 200, "the request itself is served (never blocked)");
    assert_eq!(
        rig.plan.requests().len(),
        p_before,
        "the probe was deferred: the plan's own window boundary has not \
         passed and the local counter reads exhausted, so the experiment \
         waits — S2's first turn went to the overflow account"
    );
    assert_eq!(rig.api.requests().len(), 2);

    rig.stop();
}
