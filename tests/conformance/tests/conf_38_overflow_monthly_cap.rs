//! CONF-38 (spec §4.6 / ADR-014 item 7's guardrail): **`overflow_monthly_cap_usd`
//! refuses the family's metered spend past the cap.** The cap is compared
//! against *measured* usage priced by the config table (the sum of the
//! family's overflow-route `cost.computed` rows in the UTC month — DESIGN
//! §12.10.8), evaluated before the attempt: the request that crosses the cap
//! is served (its cost is unknowable beforehand), later ones are refused
//! with `cost_cap_exceeded` (403, spec §8). The overshoot is bounded by one
//! request and nothing is estimated.
//!
//! Fixture arithmetic: one served overflow response costs 184000 nano-USD
//! (80 miss x 2000 + 20 hit x 200 + 5 out x 4000). A cap of exactly
//! 0.000184 USD therefore admits the first spill and refuses the second.

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, PlanRig};

/// Cap set to one served response's cost: 184000 nano = 0.000184 USD.
const POLICY_CAP_ONE_RESPONSE: &str = "  family: m1\n  primary: p-plan/m1\n  overflow: p-api/m1\n  on_primary_exhausted: spill\n  recover: none\n  cooldown: 0s\n  overflow_monthly_cap_usd: 0.000184";

/// The same cap, with probing enabled (case 2 needs the probe door open).
const POLICY_CAP_WITH_PROBE: &str = "  family: m1\n  primary: p-plan/m1\n  overflow: p-api/m1\n  on_primary_exhausted: spill\n  recover: probe\n  cooldown: 0s\n  overflow_monthly_cap_usd: 0.000184";

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

/// The cap admits the request that reaches it and refuses the next: after
/// the 403 spills the family, the first metered response (184000 nano)
/// brings the month's spend exactly to the cap; the second overflow
/// request is refused before any attempt, with the spec §8 body naming
/// the family and the cap.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_38_cap_reached_refuses_the_next_overflow_request() {
    let rig = rig("conf38-cap", POLICY_CAP_ONE_RESPONSE).await;
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_forbidden_403());
    rig.api.queue(testkit::plan_ok("crosses-the-cap"));
    // Would answer if (wrongly) attempted — the count betrays it.
    rig.api.queue(testkit::plan_ok("never"));

    let (s1, _b, _h) = rig.post(Some("S1"), 1);
    assert_eq!(s1, 200);
    let (s2, _b2, _h2) = rig.post(Some("S1"), 2);
    assert_eq!(
        s2, 200,
        "the request that reaches the cap is served — its cost is unknowable \
         beforehand (ADR-014 item 7)"
    );
    assert_eq!(rig.api.requests().len(), 1);

    // The month's measured spend now equals the cap: the next overflow
    // request is refused before any attempt.
    let (s3, body, _h3) = rig.post(Some("S1"), 3);
    assert_eq!(s3, 403, "cost_cap_exceeded is a 403 (spec §8)");
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        parsed["error"]["type"], "cost_cap_exceeded",
        "spec §8 error type"
    );
    let msg = parsed["error"]["message"].as_str().unwrap();
    assert!(
        msg.contains("m1") && msg.contains("cap"),
        "the reason names the family and the guardrail: {msg}"
    );
    assert_eq!(rig.api.requests().len(), 1, "no attempt was made");
    assert_eq!(
        rig.plan.requests().len(),
        2,
        "the primary was not re-tried either"
    );

    rig.stop();
}

/// Rule 1 beats rule 2 (ADR-014 item 9's deliberate order): with the cap
/// already reached, a session-boundary probe is still admitted — a probe
/// is an attempt on the *free* account and must not be refused by a cap on
/// the metered one. Counterweight pairs with the case above: same fixture,
/// the probe path is the one door the cap does not close.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_38_probe_is_admitted_even_with_the_cap_reached() {
    let rig = rig("conf38-probe", POLICY_CAP_WITH_PROBE).await;
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_forbidden_403());
    rig.api.queue(testkit::plan_ok("crosses-the-cap"));
    // The probe wins: the plan is healthy again.
    rig.plan.queue(testkit::plan_ok("probe-ok"));
    rig.api.queue(testkit::plan_ok("never"));

    let (s1, _b, _h) = rig.post(Some("S1"), 1);
    assert_eq!(s1, 200);
    let (s2, _b2, _h2) = rig.post(Some("S1"), 2);
    assert_eq!(s2, 200, "spills and reaches the cap exactly");
    assert_eq!(rig.api.requests().len(), 1);

    // Past the demotion (Retry-After: 1); cooldown is 0s.
    tokio::time::sleep(std::time::Duration::from_millis(1300)).await;

    // A new session's first turn is an admitted probe on the primary,
    // despite the cap: rule 1 precedes rule 2 by design.
    let (sp, body, _hp) = rig.post(Some("S2"), 1);
    assert_eq!(
        sp, 200,
        "the probe is served by the free account, not refused by the cap"
    );
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        parsed["choices"][0]["message"]["content"], "probe-ok",
        "the plan mock itself answered"
    );
    assert_eq!(rig.plan.requests().len(), 3, "t1, the 403, the probe");
    assert_eq!(rig.api.requests().len(), 1, "no further metered spend");

    rig.stop();
}
