//! CONF-39 (spec §4.6 / ADR-014 item 10's testability clause): **the cooldown
//! is configurable to small values so the probe gate is testable.** The
//! duration grammar already accepts `ms` segments on a u64 millisecond
//! count (config.rs `parse_duration` — no sub-millisecond floor exists to
//! work around), so this case proves it end-to-end on the live path with a
//! `100ms` cooldown: while the window is open the boundary does NOT probe
//! (the request is still served by the overflow account), and once it has
//! passed the very same kind of boundary DOES probe and wins — both
//! directions, so neither half can pass vacuously.
//!
//! The ADR-011 demotion that follows the 403 is pinned to 1s by the
//! fixture's `Retry-After: 1` (route availability — deliberately
//! independent of this knob), which is why the observing sleep is 1.3s:
//! past the demotion, only the family cooldown can explain a blocked probe.

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, PlanRig};

/// A small but non-zero cooldown: 100ms. Distinct from the rig default
/// (`0s`) on purpose — a zero cooldown cannot show the "window open"
/// half of the assertion.
const POLICY_COOLDOWN_100MS: &str = "  family: m1\n  primary: p-plan/m1\n  overflow: p-api/m1\n  on_primary_exhausted: spill\n  recover: probe\n  cooldown: 100ms";

async fn rig(tag: &str) -> PlanRig {
    let (plan, api, dir, listen_addr) =
        testkit::plan_rig_parts(tag, "", POLICY_COOLDOWN_100MS).await;
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

/// Both halves of the cooldown gate, one rig: S1's 403 spills the family
/// at T; a new session's first turn inside the 100ms window stays on the
/// overflow account, and a new session's first turn after the window
/// (and the 1s demotion) probes the primary and wins.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_39_small_cooldown_gates_then_admits_the_probe() {
    let rig = rig("conf39-cool").await;
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_forbidden_403());
    // Would answer if (wrongly) probed early — the count betrays it.
    rig.plan.queue(testkit::plan_ok("probe"));
    rig.api.queue(testkit::plan_ok("spilled"));
    rig.api.queue(testkit::plan_ok("early-window"));

    let (s1, _b, _h) = rig.post(Some("S1"), 1);
    assert_eq!(s1, 200);
    let (s2, _b2, _h2) = rig.post(Some("S1"), 2);
    assert_eq!(s2, 200, "the 403 spills the family");
    assert_eq!(rig.api.requests().len(), 1);

    // Half 1 — window open: immediately (well inside the 100ms cooldown
    // and before the 1s demotion), a new session's boundary does not
    // reach the primary. The request itself is still served.
    let (se, _be, _he) = rig.post(Some("S2"), 1);
    assert_eq!(se, 200, "served — by the overflow account, not the primary");
    assert_eq!(
        rig.plan.requests().len(),
        2,
        "no probe while the cooldown window is open: the primary mock has \
         seen only t1 and the 403"
    );
    assert_eq!(rig.api.requests().len(), 2);

    // Half 2 — window passed: past the 100ms cooldown AND the 1s route
    // demotion, a new session's boundary IS the probe and the plan wins.
    tokio::time::sleep(std::time::Duration::from_millis(1300)).await;
    let (sp, _bp, _hp) = rig.post(Some("S3"), 1);
    assert_eq!(sp, 200);
    assert_eq!(
        rig.plan.requests().len(),
        3,
        "the probe reached the primary once the (small) cooldown passed"
    );
    assert_eq!(
        rig.api.requests().len(),
        2,
        "no metered spend for the probe"
    );

    rig.stop();
}

/// The knob is what made the difference above: with `cooldown: 0s` the
/// same boundary (a new session's first turn, demotion already passed)
/// probes immediately. Directional counterweight — the 100ms case's
/// "blocked" half cannot be an artifact of the rig.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_39_zero_cooldown_counterweight_probes_immediately() {
    let rig = rig_with("conf39-zero", testkit::PLAN_POLICY_DEFAULT).await;
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_forbidden_403());
    rig.plan.queue(testkit::plan_ok("probe"));
    rig.api.queue(testkit::plan_ok("spilled"));

    let (s1, _b, _h) = rig.post(Some("S1"), 1);
    assert_eq!(s1, 200);
    let (s2, _b2, _h2) = rig.post(Some("S1"), 2);
    assert_eq!(s2, 200);
    assert_eq!(rig.api.requests().len(), 1);

    // Only the 1s demotion left to wait out; the cooldown is 0.
    tokio::time::sleep(std::time::Duration::from_millis(1300)).await;
    let (sp, _bp, _hp) = rig.post(Some("S2"), 1);
    assert_eq!(sp, 200);
    assert_eq!(rig.plan.requests().len(), 3, "probed at the first boundary");
    assert_eq!(rig.api.requests().len(), 1);

    rig.stop();
}

async fn rig_with(tag: &str, policy: &str) -> PlanRig {
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
