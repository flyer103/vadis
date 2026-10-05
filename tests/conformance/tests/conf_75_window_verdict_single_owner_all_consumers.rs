//! CONF-75 (spec §4.6 rule 3 / ADR-016 §13.3 L1d, fixed this round):
//! **the local counter's window verdict, agreed end-to-end — the
//! projection row, the serving path's own probe gate, and the live
//! `/health` section all give the same answer, because there is now one
//! adjudication** (`availability::probe_deferred_by_window`; the second
//! copy in `health.rs` is deleted, not wrapped).
//!
//! The exhaustion is **earned by the run's own accounting** (CONF-74's
//! fixture: one in-plan request charges exactly the 105-token
//! allowance), the family spills for real, and then:
//!
//! - the store's `Query::QuotaUsed` row, read directly by this test,
//!   shows the window exhausted at the window start `window_start_for`
//!   computes for `now` — the raw projection fact;
//! - the live `/health` says `blocked_by: "window_not_reset"` — the
//!   report's verdict;
//! - the **same request the report describes** (a fresh session at a
//!   boundary, with the primary's 1s demotion waited out) is served by
//!   the overflow account and the plan mock is **not** probed — the
//!   request path's verdict, witnessed by the wire, not by an echo;
//! - the guard, fed the deferral input derived independently from the
//!   same projection row, agrees the probe was correctly deferred.
//!
//! If the report and the request path ever re-grow separate
//! adjudications, one of the last two bullets diverges from `/health`
//! and this case names which consumer drifted.

#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::net::TcpStream;

use vadis_conformance::testkit::{self, PlanRig};
use vadis_core::config::{CapUsdVal, DurationVal, OnPrimaryExhausted, PlanPolicyCfg};
use vadis_core::cost::Nano;
use vadis_core::plan::{PlanFirstRule, PlanRequest, PlanStateRow};
use vadis_core::store::{Query, QueryRow, Store as _};

/// A plan whose allowance is exactly one request (105 chargeable
/// tokens), so one served in-plan request exhausts it (CONF-35/74's
/// fixture).
const QUOTA_ONE_REQUEST: &str = "\n    quota:\n      - models: [m1]\n        window: monthly\n        tokens: 105\n        reset_day: 1\n        over_quota: block\n        source: \"fixture: exactly one request's chargeable tokens\"";

fn policy() -> PlanPolicyCfg {
    PlanPolicyCfg {
        family: "m1".into(),
        primary: vadis_core::config::RouteSpec {
            provider: "p-plan".into(),
            model: "m1".into(),
        },
        overflow: vadis_core::config::RouteSpec {
            provider: "p-api".into(),
            model: "m1".into(),
        },
        on_primary_exhausted: OnPrimaryExhausted::Spill,
        recover: vadis_core::config::RecoveryMode::Probe,
        cooldown: DurationVal(0),
        overflow_monthly_cap_usd: Some(CapUsdVal(20.0)),
        overflow_selection: vadis_core::config::OverflowSelection::Declared,
    }
}

fn now_us() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0)
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

/// The deferral input, derived **in this file** from the projection row
/// the run itself produced: exhausted (used >= 105) inside the window
/// that opens at `window_start_for(now, 1)` whose 10/1 boundary has not
/// passed. This is the adjudication's shape, spelled by the test — if
/// the single owner's steps drift, one of the agreement assertions
/// below fires.
fn deferral_from_projection(dir: &std::path::Path) -> bool {
    let store = vadis_store::SqliteStore::open(&dir.join("state/vadis.db")).unwrap();
    let now_s = (now_us().max(0) as u64) / 1_000_000;
    let window_start_s = vadis_core::quota::window_start_for(now_s, 1);
    let next_boundary_s = vadis_core::quota::next_reset(window_start_s, 1);
    assert!(
        now_s < next_boundary_s,
        "fixture invariant: the test runs inside the window"
    );
    match store.query(Query::QuotaUsed {
        provider: "p-plan",
        plan_idx: 0,
        window_start_us: window_start_s as i64 * 1_000_000,
    }) {
        Ok(QueryRow::Count(used)) => (used as u64) >= 105,
        other => panic!("quota row after the spill: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_75_window_verdict_agreed_by_projection_report_and_request_path() {
    let (plan, api, dir, listen_addr) =
        testkit::plan_rig_parts("conf75", QUOTA_ONE_REQUEST, testkit::PLAN_POLICY_DEFAULT).await;
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    let rig = PlanRig {
        plan,
        api,
        listen_addr,
        dir,
        serve_task,
    };
    let p = policy();

    // Earn the exhaustion: turn 1 in-plan charges 105 of 105, turn 2
    // meets the 403 and spills. Queue the probe's would-be answer: if
    // the deferral is wrong the count betrays it.
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_forbidden_403());
    rig.plan.queue(testkit::plan_ok("never-probed"));
    rig.api.queue(testkit::plan_ok("spilled"));
    rig.api.queue(testkit::plan_ok("deferred-but-served"));

    let (s1, _b, _h) = rig.post(Some("S1"), 1);
    assert_eq!(s1, 200);
    let (s2, _b2, _h2) = rig.post(Some("S1"), 2);
    assert_eq!(s2, 200, "the 403 spills the family to overflow");

    // The spill's 403 demoted p-plan for the fixture's 1s; the
    // demotion would mask the window arm, so wait it out (CONF-74's
    // timing).
    tokio::time::sleep(std::time::Duration::from_millis(1_300)).await;

    // (2) The live report's verdict.
    let h = http_get(&rig.listen_addr, "/health");
    assert_eq!(h["plan"]["account"], "overflow");
    assert_eq!(h["plan"]["probe"]["blocked_by"], "window_not_reset");
    assert_eq!(h["plan"]["probe"]["admitted"], false);

    // (3) The request path's verdict, on the wire: the fresh session's
    // boundary — the exact request shape the report describes — is
    // still SERVED by the overflow account and never probes the plan.
    let plan_before = rig.plan.requests().len();
    let api_before = rig.api.requests().len();
    let (se, _be, _he) = rig.post(Some("S2"), 1);
    assert_eq!(se, 200, "a warning never blocks: served on overflow");
    assert_eq!(
        rig.plan.requests().len(),
        plan_before,
        "no probe: the request path deferred on the same verdict"
    );
    assert_eq!(rig.api.requests().len(), api_before + 1);

    // The store is the writer's (EXCLUSIVE lock — L3's own lesson, and
    // the reason the reads below happen only after serve stops).
    let dir = rig.stop();

    // (1) The raw projection fact, read by this test from the row the
    // run's own accounting wrote.
    let deferred = deferral_from_projection(&dir);
    assert!(deferred, "the run's own charge exhausted the window");

    // (4) The guard agrees the deferral was the right call: fed the
    // deferral input derived independently in (1), the probe gate
    // answers `DeferredByWindow` for the surface's reduced request.
    let rule = PlanFirstRule::new(p.clone(), vec![p.primary.clone()]);
    let req = PlanRequest {
        session: Some("conf-75"),
        turn_index: 1,
        state: PlanStateRow {
            route: p.overflow.clone(),
            since_us: 0, // cooldown 0s: the arm under test is the window's
        },
        now_us: now_us(),
        primary_allowed: true,
        deferred_by_window: deferred,
        overflow_spend: Nano(0),
        // R66-1e's field, absent here: no prior binding (this fixture
        // evaluates the probe gate, which the pin never feeds).
        pinned: None,
    };
    assert_eq!(
        rule.probe_admitted(&req),
        Err(vadis_core::plan::ProbeBlockedBy::DeferredByWindow),
        "the guard, fed the test-derived deferral, refuses the probe"
    );
}
