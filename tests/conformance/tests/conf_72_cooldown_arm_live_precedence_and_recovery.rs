//! CONF-72 (ADR-016 §13.3 L1a/L1b, fixed this round): **the cooldown arm's
//! precedence and its clock unit, witnessed live on both sides of the
//! deadline.** The state matrix (CONF-71) pins the four words at static
//! positions; this case pins the *time-dependent* row — the one the
//! duplicated implementations historically got wrong — by driving a real
//! spill and reading `/health` twice:
//!
//! - **inside the window** the section says `blocked_by: "cooldown"` —
//!   and the same evaluation the guard would make (independent
//!   `PlanFirstRule` call, at a `now` taken after the read) agrees;
//! - **after the deadline** the same section says `admitted: true` — and
//!   the *next real request at a session boundary actually probes the
//!   primary and wins* (CONF-39's recovery sequence), so the "admitted"
//!   row is not the surface's opinion alone: the guard admits too, and
//!   the plan answers.
//!
//! The cooldown is `200ms`: long enough that the first `/health` read is
//! safely inside, short enough for a bounded wait. The deadline itself is
//! asserted from the strings (`deadline − since == 200ms` exactly), which
//! is the L1b witness on the wire: the single `PlanPolicyCfg::cooldown_us`
//! conversion feeds both the printed deadline and the gate the next
//! request passes.
//!
//! A demoted primary would mask the recovery, so the arm uses the
//! fixture's `Retry-After: 1` demotion (1s) and waits past it; the
//! remaining 1.3s total also makes the second read's position (inside vs
//! past) a property of the seeded 200ms deadline, not of scheduling.

#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::net::TcpStream;

use router_conformance::testkit::{self, PlanRig};
use router_core::config::{
    CapUsdVal, DurationVal, OnPrimaryExhausted, PlanPolicyCfg, RecoveryMode, RouteSpec,
};
use router_core::cost::Nano;
use router_core::plan::{PlanAccount, PlanFirstRule, PlanRequest, PlanStateRow};

const POLICY_200MS: &str = "  family: m1\n  primary: p-plan/m1\n  overflow: p-api/m1\n  on_primary_exhausted: spill\n  recover: probe\n  cooldown: 200ms";

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
        cooldown: DurationVal(200),
        overflow_monthly_cap_usd: Some(CapUsdVal(20.0)),
    }
}

/// The guard's word for the surface's reduced request at `now_us`
/// (independent evaluation — the same type the server runs, called here).
fn guard_word(p: &PlanPolicyCfg, since_us: i64, now_us: i64) -> Option<String> {
    let rule = PlanFirstRule::new(p.clone());
    let req = PlanRequest {
        session: Some("conf-72"),
        turn_index: 1,
        state: PlanStateRow {
            account: PlanAccount::Overflow,
            since_us,
        },
        now_us,
        primary_allowed: true,
        deferred_by_window: false,
        overflow_spend: Nano(0),
    };
    rule.probe_admitted(&req)
        .err()
        .and_then(|e| e.blocked_by_surface_word())
        .map(str::to_string)
}

async fn rig(tag: &str, policy: &str) -> PlanRig {
    let (plan, api, dir, listen_addr) = testkit::plan_rig_parts(tag, "", policy).await;
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

fn now_us() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0)
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
async fn conf_72_cooldown_arm_live_before_and_after_the_deadline() {
    let rig = rig("conf72-cool", POLICY_200MS).await;
    let p = policy();

    // A real spill at T: turn 1 answers, turn 2 meets the 403. The probe's
    // 200 is queued UP FRONT with the others: the mock serves a lone
    // queued response by cloning it (CONF-39's authoring rule), so a
    // late-queued 200 would sit behind the 403's unpopped clone and the
    // boundary request would pop the 403 instead — a second spill, not a
    // probe.
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_forbidden_403());
    rig.plan.queue(testkit::plan_ok("probe"));
    rig.api.queue(testkit::plan_ok("spilled"));

    let (s1, _b, _h) = rig.post(Some("S1"), 1);
    assert_eq!(s1, 200);
    let (s2, _b2, _h2) = rig.post(Some("S1"), 2);
    assert_eq!(s2, 200, "the 403 spills the family to overflow");
    assert_eq!(rig.api.requests().len(), 1);

    // Read 1 — inside the 200ms window: the surface says `cooldown`, and
    // the guard (independently evaluated, at a now after the read —
    // strictly harsher) says `cooldown` too.
    let h1 = http_get(&rig.listen_addr, "/health");
    assert_eq!(h1["plan"]["account"], "overflow");
    assert_eq!(h1["plan"]["probe"]["blocked_by"], "cooldown");
    assert_eq!(h1["plan"]["probe"]["admitted"], false);
    let since_ms = parse_ms(h1["plan"]["since"].as_str().expect("since"));
    let deadline_ms = parse_ms(h1["plan"]["probe"]["deadline"].as_str().expect("deadline"));
    assert_eq!(
        deadline_ms - since_ms,
        200,
        "L1b on the wire: deadline - since is exactly the loaded 200ms"
    );
    assert_eq!(
        guard_word(&p, since_ms * 1_000, now_us()),
        Some("cooldown".into()),
        "the guard agrees the probe is blocked (independent call)"
    );

    // Past the 200ms deadline and the 1s fixture demotion (Retry-After: 1
    // on the 403): the section flips to admitted, and the very next
    // session boundary REALLY probes the primary (CONF-39's sequence) —
    // the guard's admission is witnessed by the plan mock being reached.
    tokio::time::sleep(std::time::Duration::from_millis(1_300)).await;
    let h2 = http_get(&rig.listen_addr, "/health");
    assert_eq!(h2["plan"]["probe"]["admitted"], true);
    assert_eq!(h2["plan"]["probe"]["blocked_by"], serde_json::Value::Null);

    let (sp, _bp, hp) = rig.post(Some("S3"), 1);
    assert_eq!(sp, 200);
    // The header set carries no displacement: the probe's route is the
    // primary itself, nothing was abandoned.
    assert_eq!(
        hp.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("x-router-failover-from")),
        None,
        "nothing was abandoned: the probe's route is the primary itself"
    );
    assert_eq!(
        rig.plan.requests().len(),
        3,
        "the boundary request probed the primary (t1, the 403, the probe)"
    );
    assert_eq!(
        rig.api.requests().len(),
        1,
        "no metered spend for the probe"
    );

    // The return trip's trace row (spec §6): the recovery is recorded on
    // the probing request itself — reason `primary_recovered`, probe
    // true, zero cost (an in-plan destination).
    let dir = rig.stop();
    let rec = trace_records(&dir)
        .into_iter()
        .find(|r| r["identity"]["session"] == "S3")
        .expect("the probing request's record");
    assert_eq!(rec["decision"]["provider"], "p-plan");
    assert_eq!(rec["result"]["failover_from"], serde_json::Value::Null);
    let ps = &rec["result"]["plan_switch"];
    assert_eq!(ps["from"], "p-api/m1");
    assert_eq!(ps["to"], "p-plan/m1");
    assert_eq!(ps["reason"], "primary_recovered");
    assert_eq!(ps["probe"], true);
    assert_eq!(ps["switch_cost_nano"], 0);
}

/// Every trace record, in write order (the CONF-42 reader).
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
