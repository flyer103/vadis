//! CONF-36 (spec §4.6 / ADR-014 item 7): **`on_primary_exhausted: block`
//! refuses instead of spending.** Once the primary account is exhausted, a
//! request that cannot be served on the primary is refused with the spec §8
//! `quota_exceeded` body (429) — a readable reason naming the family, never
//! a silent 200 from the metered account (which would defeat the mode), and
//! never a quietly downgraded request.
//!
//! The counterweight (the pair must move in both directions): `spill` under
//! the identical 403 keeps serving from the metered account — CONF-32 pins
//! that half; here the same first half of the exchange is replayed and only
//! the mode differs.

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, PlanRig};

/// The identical exchange as CONF-32's spill case, but the policy refuses:
/// `on_primary_exhausted: block`, `recover: none` (so nothing probes the
/// account back and the refusal is the steady state, not a race).
const POLICY_BLOCK: &str = "  family: m1\n  primary: p-plan/m1\n  overflow: p-api/m1\n  on_primary_exhausted: block\n  recover: none\n  cooldown: 0s";

async fn rig(tag: &str, policy: &str) -> PlanRig {
    let (plan, api, dir, listen_addr) = testkit::plan_rig_parts(tag, "", policy).await;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_36_block_refuses_readably_after_the_403() {
    let rig = rig("conf36-block", POLICY_BLOCK).await;
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_forbidden_403());
    // The api mock would answer if (wrongly) spilled — the count betrays it.
    rig.api.queue(testkit::plan_ok("never"));

    // Turn 1: healthy plan, block mode changes nothing yet.
    let (s1, _b, _h) = rig.post(Some("S1"), 1);
    assert_eq!(s1, 200);
    assert_eq!(rig.plan.requests().len(), 1);

    // Turn 2: the upstream 403 exhausts the primary mid-session.
    let (s2, body, _h2) = rig.post(Some("S1"), 2);
    assert_eq!(
        s2, 429,
        "block: the request is refused, not served from the metered account"
    );
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        parsed["error"]["type"], "quota_exceeded",
        "spec §8 error type"
    );
    let msg = parsed["error"]["message"].as_str().unwrap();
    assert!(
        msg.contains("m1") && msg.contains("block"),
        "the reason names the family and the mode: {msg}"
    );
    assert_eq!(
        rig.api.requests().len(),
        0,
        "the metered account was NEVER touched — no silent spend"
    );

    // The steady state: further family requests are refused on sight, no
    // attempt anywhere (recover: none, so no probe re-litigates this).
    rig.plan.queue(testkit::plan_ok("nope"));
    let p_before = rig.plan.requests().len();
    let (s3, _b3, _h3) = rig.post(Some("S2"), 1);
    assert_eq!(s3, 429, "a new session is refused the same way");
    assert_eq!(rig.plan.requests().len(), p_before);
    assert_eq!(rig.api.requests().len(), 0);

    rig.stop();
}

/// The counterweight: `spill` under the identical exchange answers 200 from
/// the metered account. One fixture, two policies, opposite outcomes — the
/// assertion cannot pass because the rig is accidentally generous.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_36_spill_counterweight_serves_the_same_403() {
    let rig = rig("conf36-spill", testkit::PLAN_POLICY_DEFAULT).await;
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_forbidden_403());
    rig.api.queue(testkit::plan_ok("spilled"));

    let (s1, _b, _h) = rig.post(Some("S1"), 1);
    assert_eq!(s1, 200);
    let (s2, _b2, _h2) = rig.post(Some("S1"), 2);
    assert_eq!(
        s2, 200,
        "spill: the identical 403 continues on the metered account"
    );
    assert_eq!(rig.api.requests().len(), 1);

    rig.stop();
}
