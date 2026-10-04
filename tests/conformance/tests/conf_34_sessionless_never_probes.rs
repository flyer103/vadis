//! CONF-34 (spec §4.6 / ADR-014 item 3): **a request with no session never
//! probes.** It has no boundary to be admitted at (`turn_index` is 1 for
//! every sessionless request by definition, so "first request" does not
//! discriminate), and probing on every sessionless request is the
//! per-request flip item 1 forbids. A sessionless request follows the
//! current account state and no more — overflow while the family has
//! spilled, primary again only after the upstream (not the request itself)
//! moved the family back.
//!
//! `cooldown: 0s` throughout: wherever a sessionless request must NOT reach
//! the primary, the cooldown cannot be the reason.

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, PlanRig};

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

/// Rule 5: after a sessionless request's 403 spills the family, further
/// sessionless requests are served by the overflow account with **zero**
/// attempts on the primary — even though the primary mock is answering
/// 200s, the cooldown is 0s and the demotion (Retry-After: 1) has long
/// passed. Only the account state moves a sessionless request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_34_sessionless_never_probes_follows_the_state() {
    let rig = rig("conf34-nosess").await;
    // Sessionless #1: healthy plan -> primary.
    rig.plan.queue(testkit::plan_ok("s1"));
    // Sessionless #2: the 403 that spills the family.
    rig.plan.queue(testkit::plan_forbidden_403());
    // Would answer 200 if (wrongly) probed — the count would betray it.
    rig.plan.queue(testkit::plan_ok("recovered"));
    rig.api.queue(testkit::plan_ok("spilled"));

    let (s1, _b, _h) = rig.post(None, 1);
    assert_eq!(s1, 200);
    assert_eq!(
        rig.plan.requests().len(),
        1,
        "the state was primary: served by the plan"
    );

    let (s2, _b2, _h2) = rig.post(None, 1);
    assert_eq!(s2, 200, "spilled: the metered account answers");
    assert_eq!(rig.plan.requests().len(), 2);
    assert_eq!(rig.api.requests().len(), 1);

    // Past every clock the policy owns (cooldown 0s; demotion 1s).
    tokio::time::sleep(std::time::Duration::from_millis(1300)).await;

    let p_before = rig.plan.requests().len();
    let (s3, _b3, _h3) = rig.post(None, 1);
    assert_eq!(s3, 200, "still served (by the overflow account)");
    assert_eq!(
        rig.plan.requests().len(),
        p_before,
        "the primary mock received NOTHING: a sessionless request never \
         probes — it follows the current account state and no more"
    );
    assert_eq!(rig.api.requests().len(), 2);

    rig.stop();
}

/// The counterweight (the pair must be movable in both directions): once a
/// **session's** probe has moved the family back to primary — the upstream's
/// own verdict, not the sessionless request's initiative — the next
/// sessionless request is served by the plan account again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_34_sessionless_follows_the_state_back_after_a_real_probe() {
    let rig = rig("conf34-back").await;
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_forbidden_403());
    rig.plan.queue(testkit::plan_ok("probe"));
    rig.plan.queue(testkit::plan_ok("nosess-after"));
    rig.api.queue(testkit::plan_ok("spilled"));

    // A session spills on its second turn.
    let (s1, _b, _h) = rig.post(Some("S1"), 1);
    assert_eq!(s1, 200);
    let (s2, _b2, _h2) = rig.post(Some("S1"), 2);
    assert_eq!(s2, 200);
    assert_eq!(rig.api.requests().len(), 1);

    tokio::time::sleep(std::time::Duration::from_millis(1300)).await;

    // A NEW session's first turn is the admitted probe; it wins.
    let (sp, _bp, _hp) = rig.post(Some("S2"), 1);
    assert_eq!(sp, 200);
    assert_eq!(
        rig.plan.requests().len(),
        3,
        "the probe reached the primary"
    );

    // Now the sessionless request follows the recovered state.
    let (sn, _bn, _hn) = rig.post(None, 1);
    assert_eq!(sn, 200);
    assert_eq!(
        rig.plan.requests().len(),
        4,
        "the sessionless request is on the plan account again — the state \
         moved it, not a probe of its own"
    );
    assert_eq!(rig.api.requests().len(), 1);

    rig.stop();
}
