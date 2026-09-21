//! CONF-74 (ADR-016 §13.3 L1a, fixed this round; spec §4.6 rule 3): **the
//! `window_not_reset` arm, end-to-end with request traffic** — the one
//! arm whose inputs CONF-71 seeds synthetically (a projection row the
//! test wrote) and whose real producer is the accounting path's own
//! `quota.charged` projection. Here the exhaustion is **earned**: one
//! served in-plan request charges its tokens against a 100-token plan
//! (CONF-35's fixture), the family spills for real on the next turn, and
//! the fresh session's probe is deferred by the window — visible as
//! `blocked_by: "window_not_reset"` on the live `/health`, agreed by an
//! independent guard evaluation on the state the run itself produced.
//!
//! The control is the same rig without the quota declaration: no plan,
//! no counter, no deferral — `admitted: true` (the arm's value is the
//! counter's, not the clock's or the spill's). And the deferral is
//! honest in the other direction too: the deferred request is still
//! **served** by the overflow account (a warning never blocks, §4.6
//! rule 3 — CONF-35's rule, witnessed here at the boundary).

#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::net::TcpStream;

use router_conformance::testkit::{self, PlanRig};
use router_core::config::{
    CapUsdVal, DurationVal, OnPrimaryExhausted, PlanPolicyCfg, RecoveryMode, RouteSpec,
};
use router_core::cost::Nano;
use router_core::plan::{PlanAccount, PlanFirstRule, PlanRequest, PlanStateRow};

/// A plan whose allowance is exactly one request (105 chargeable tokens),
/// so one served in-plan request exhausts it (CONF-35's fixture).
const QUOTA_ONE_REQUEST: &str = "\n    quota:\n      - models: [m1]\n        window: monthly\n        tokens: 105\n        reset_day: 1\n        over_quota: block\n        source: \"fixture: exactly one request's chargeable tokens\"";

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

/// The guard's (admitted, blocked_by) for the surface's reduced request,
/// evaluated independently on the state the test supplies.
fn guard_answer(
    p: &PlanPolicyCfg,
    since_us: i64,
    deferred_by_window: bool,
) -> (bool, Option<String>) {
    let rule = PlanFirstRule::new(p.clone());
    let req = PlanRequest {
        session: Some("conf-74"),
        turn_index: 1,
        state: PlanStateRow {
            account: PlanAccount::Overflow,
            since_us,
        },
        now_us: i64::MAX / 2, // far past any cooldown; the arm under test is the window's
        primary_allowed: true,
        deferred_by_window,
        overflow_spend: Nano(0),
    };
    match rule.probe_admitted(&req) {
        Ok(()) => (true, None),
        Err(e) => (false, e.blocked_by_surface_word().map(str::to_string)),
    }
}

async fn rig(tag: &str, quota_yaml: &str) -> PlanRig {
    let (plan, api, dir, listen_addr) =
        testkit::plan_rig_parts(tag, quota_yaml, testkit::PLAN_POLICY_DEFAULT).await;
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

fn http_get(addr: &str, path: &str) -> serde_json::Value {
    let mut stream = TcpStream::connect(addr).expect("connect");
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    let text = String::from_utf8_lossy(&buf).into_owned();
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("");
    serde_json::from_str(body.trim()).expect("health json")
}

fn parse_ms(ts: &str) -> i64 {
    let (date, rest) = ts.split_once('T').expect("T");
    let mut d = date.split('-');
    let y: i64 = d.next().unwrap().parse().unwrap();
    let mo: u32 = d.next().unwrap().parse().unwrap();
    let day: u32 = d.next().unwrap().parse().unwrap();
    let (time, frac) = rest.split_once('.').unwrap_or((rest, ""));
    let mut t = time.split(':');
    let h: i64 = t.next().unwrap().parse().unwrap();
    let mi: i64 = t.next().unwrap().parse().unwrap();
    let s: i64 = t.next().unwrap().parse().unwrap();
    let ms: i64 = if frac.is_empty() {
        0
    } else {
        let mut f = frac.trim_end_matches('Z').to_string();
        while f.len() < 3 {
            f.push('0');
        }
        f[..3].parse().unwrap()
    };
    router_core::peak::utc_midnight_epoch(y, mo, day) as i64 * 1_000
        + h * 3_600_000
        + mi * 60_000
        + s * 1_000
        + ms
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_74_window_arm_earned_by_traffic_and_agreed_by_the_guard() {
    let rig = rig("conf74-window", QUOTA_ONE_REQUEST).await;
    let p = policy();

    // Turn 1 in-plan (charges 105 against the 105-token plan — the window
    // is exhausted by the run's own accounting), turn 2 meets the 403 and
    // spills. Queue the probe's would-be answer anyway: if the deferral
    // is wrong the count betrays it.
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_forbidden_403());
    rig.plan.queue(testkit::plan_ok("never-probed"));
    rig.api.queue(testkit::plan_ok("spilled"));
    rig.api.queue(testkit::plan_ok("deferred-but-served"));

    let (s1, _b, _h) = rig.post(Some("S1"), 1);
    assert_eq!(s1, 200);
    let (s2, _b2, _h2) = rig.post(Some("S1"), 2);
    assert_eq!(s2, 200, "the 403 spills the family to overflow");

    // The spill's 403 also demoted p-plan for the fixture's 1s
    // (Retry-After: 1); `primary_cooling_down` outranks the window arm in
    // the guard's order, so the section is read only once the demotion
    // has passed — leaving the window as the one honest blocker.
    tokio::time::sleep(std::time::Duration::from_millis(1_300)).await;

    // The live surface: the window has not reset, so the probe is
    // deferred — and the guard, evaluated independently on the state the
    // run produced (overflow at the spill's since; window exhausted),
    // gives the same word.
    let h = http_get(&rig.listen_addr, "/health");
    assert_eq!(h["plan"]["account"], "overflow");
    assert_eq!(h["plan"]["probe"]["blocked_by"], "window_not_reset");
    assert_eq!(h["plan"]["probe"]["admitted"], false);
    let since_ms = parse_ms(h["plan"]["since"].as_str().expect("since"));
    let (admitted, word) = guard_answer(&p, since_ms * 1_000, true);
    assert_eq!(admitted, false);
    assert_eq!(word.as_deref(), Some("window_not_reset"));

    // The deferral is honest in the other direction: a fresh session's
    // boundary is still SERVED (by the overflow account — a warning
    // never blocks a request, §4.6 rule 3), and the primary is not
    // probed (the counter's only influence is the deferral).
    let (se, _be, _he) = rig.post(Some("S2"), 1);
    assert_eq!(se, 200, "served by the overflow account");
    assert_eq!(
        rig.plan.requests().len(),
        2,
        "no probe: the window deferral held on the request path too"
    );
    assert_eq!(rig.api.requests().len(), 2);

    rig.stop();
}

/// The control: the same spill on a rig with **no** quota declared —
/// no counter, no deferral, and the same state reads `admitted: true`.
/// The arm's value is the counter's, not the spill's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_74_without_a_declared_plan_there_is_no_deferral() {
    let rig = rig("conf74-control", "").await;

    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_forbidden_403());
    rig.api.queue(testkit::plan_ok("spilled"));

    let (s1, _b, _h) = rig.post(Some("S1"), 1);
    assert_eq!(s1, 200);
    let (s2, _b2, _h2) = rig.post(Some("S1"), 2);
    assert_eq!(s2, 200);

    // Same as the main case: the spill's 1s demotion must pass first.
    tokio::time::sleep(std::time::Duration::from_millis(1_300)).await;

    let h = http_get(&rig.listen_addr, "/health");
    assert_eq!(h["plan"]["account"], "overflow");
    assert_eq!(h["plan"]["probe"]["admitted"], true);
    assert_eq!(h["plan"]["probe"]["blocked_by"], serde_json::Value::Null);

    rig.stop();
}
