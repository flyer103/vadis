//! CONF-71 (spec §9.1 / ADR-016 §13.3 L1a, fixed this round): **`/health`'s
//! `probe.blocked_by`/`admitted` and the guard's probe gate give the same
//! answer for the same state — because both outputs are produced by one
//! authority.** The section no longer re-derives the evaluation order; it
//! evaluates `PlanFirstRule::probe_admitted` on the surface's reduced
//! request (a fresh session at `turn_index == 1` — the only request shape
//! the gate could still admit, so the two request-shaped arms never fire)
//! and prints the arm's own `blocked_by_surface_word`.
//!
//! This case pins the fix from the outside, on the real `serve` assembly:
//! a state matrix — five projections, one per §9.1 word plus the admitted
//! row — where each arm's live `/health` section is asserted **equal to
//! an independent evaluation of the guard on the very same inputs the
//! test itself put in the store** (`PlanFirstRule` called directly in
//! this file, not through the server). If the surface ever re-grows a
//! second copy of the order, some arm's word diverges and this case names
//! the arm.
//!
//! Determinism: the states are written into the store **before** `serve`
//! opens it (the EXCLUSIVE writer lock forecloses any second writer
//! later), so no request traffic and no racing clock is needed — every
//! arm's expectation is computed from inputs the test chose, with the
//! only live read (`now`) taken after the section rendered and supplied
//! to the independent evaluation. The four blocking words plus the
//! admitted row are exercised without a single sleep: cooldown 0 makes
//! the time arm time-independent, and the other arms sit at positions
//! where the clock cannot change the word.

#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::net::TcpStream;

use vadis_conformance::testkit::{self, PlanRig};
use vadis_core::config::{
    CapUsdVal, DurationVal, OnPrimaryExhausted, PlanPolicyCfg, RecoveryMode, RouteSpec,
};
use vadis_core::cost::Nano;
use vadis_core::plan::{PlanAccount, PlanFirstRule, PlanRequest, PlanStateRow};
use vadis_core::store::{EventKind, NewEvent, ProjectionWrite};
use vadis_core::store::{Query, QueryRow, Store as _};

/// The policy every arm loads: spill + probe with `cooldown: 0s`, so the
/// time arm of the gate is always elapsed and the word depends only on
/// the state the test seeds. Arm D alone overrides `recover: none`.
const POLICY: &str = "  family: m1\n  primary: p-plan/m1\n  overflow: p-api/m1\n  on_primary_exhausted: spill\n  recover: probe\n  cooldown: 0s";
const POLICY_RECOVER_NONE: &str = "  family: m1\n  primary: p-plan/m1\n  overflow: p-api/m1\n  on_primary_exhausted: spill\n  recover: none\n  cooldown: 0s";

/// A quota plan with a 100-token allowance, exhausted by the seed below.
const QUOTA_100: &str = "\n    quota:\n      - models: [m1]\n        window: monthly\n        tokens: 100\n        reset_day: 1\n        over_quota: block\n        source: \"fixture: CONF-71 window arm\"";

/// The same policy the YAML above parses to, built directly for the
/// independent guard evaluation (loader equivalence is CONF-25's concern).
fn policy(recover: RecoveryMode) -> PlanPolicyCfg {
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
        recover,
        cooldown: DurationVal(0),
        overflow_monthly_cap_usd: Some(CapUsdVal(20.0)),
        overflow_selection: vadis_core::config::OverflowSelection::Declared,
    }
}

/// The guard's answer for the surface's reduced request (fresh session,
/// `turn_index == 1`), evaluated independently of the server: the same
/// `PlanFirstRule` type, called in this file on inputs the test chose.
fn guard_answer(
    p: &PlanPolicyCfg,
    state: PlanStateRow,
    now_us: i64,
    primary_allowed: bool,
    deferred_by_window: bool,
) -> (bool, Option<String>) {
    let rule = PlanFirstRule::new(p.clone());
    let req = PlanRequest {
        session: Some("conf-71"),
        turn_index: 1,
        state,
        now_us,
        primary_allowed,
        deferred_by_window,
        overflow_spend: Nano(0),
    };
    match rule.probe_admitted(&req) {
        Ok(()) => (true, None),
        Err(e) => (false, e.blocked_by_surface_word().map(str::to_string)),
    }
}

/// What state the test seeds for one arm, and the guard inputs that
/// mirror it (`primary_allowed` / `deferred_by_window` are the projection
/// reads /health performs on the seeded rows).
enum Seed {
    /// No `plan_state` row: the family never switched.
    NeverSwitched,
    /// `plan_state: overflow` at a known `since_us`; nothing else.
    OverflowSince(i64),
    /// Overflow plus a live provider cooldown on p-plan (ADR-011's read
    /// refuses the primary).
    OverflowPrimaryCooling(i64),
    /// Overflow plus an exhausted quota counter for the current window.
    OverflowWindowNotReset(i64),
}

/// One arm: write the config, seed the store (before `serve` opens it —
/// the EXCLUSIVE writer lock means later writes are impossible), start
/// the real serve, GET /health, and hand the section back.
async fn arm(tag: &str, policy_yaml: &str, quota_yaml: &str, seed: Seed) -> serde_json::Value {
    let (plan, api, dir, listen_addr) = testkit::plan_rig_parts(tag, quota_yaml, policy_yaml).await;
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();

    // Seed the projections the arm needs, then release the writer lock
    // by dropping the handle before serve is spawned.
    let mut since_us = 0i64;
    let mut primary_allowed = true;
    let mut deferred_by_window = false;
    if !matches!(seed, Seed::NeverSwitched) {
        let store = vadis_store::SqliteStore::open(&dir.join("state/vadis.db")).unwrap();
        // The transition event: its own ts_us becomes since_us (read
        // back from the projection, never assumed).
        let ev = store
            .append(NewEvent {
                kind: EventKind::PlanSwitched,
                request_id: Some("conf-71-seed"),
                session: None,
                body_hash: None,
                trace_ref: None,
                payload: serde_json::json!({
                    "family": "m1",
                    "from_account": "primary",
                    "to_account": "overflow",
                    "reason": "primary_exhausted",
                    "probe": false,
                    "cooldown_ms": 0,
                }),
            })
            .unwrap();
        store
            .project(ProjectionWrite::PlanSwitched {
                family: "m1",
                account: "overflow",
                cooldown_us: 0,
                last_event: ev,
            })
            .unwrap();
        match &seed {
            Seed::OverflowPrimaryCooling(_) => {
                let now = vadis_store_now_us();
                store
                    .project(ProjectionWrite::ProviderCooldown {
                        scope: "provider",
                        provider: "p-plan",
                        model: "",
                        until_us: now + 3_600 * 1_000_000,
                        reason: "quota_exhausted",
                        last_event: ev,
                    })
                    .unwrap();
                primary_allowed = false;
            }
            Seed::OverflowWindowNotReset(_) => {
                let now_s = (vadis_store_now_us().max(0) as u64) / 1_000_000;
                let window_start_us =
                    vadis_core::quota::window_start_for(now_s, 1) as i64 * 1_000_000;
                store
                    .project(ProjectionWrite::QuotaCharged {
                        provider: "p-plan",
                        plan_idx: 0,
                        window_start_us,
                        tokens: 200, // >= the 100-token allowance
                        last_event: ev,
                    })
                    .unwrap();
                deferred_by_window = true;
            }
            _ => {}
        }
        // since_us as the projection recorded it (the event's ts_us).
        match store.query(Query::PlanState { family: "m1" }) {
            Ok(QueryRow::PlanState(Some(row))) => since_us = row.since_us,
            other => panic!("plan_state read after seed: {other:?}"),
        }
        drop(store);
    }

    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    let rig = PlanRig {
        plan,
        api,
        listen_addr,
        dir,
        serve_task,
    };
    let h = http_get(&rig.listen_addr, "/health");
    rig.stop();

    // The independent guard evaluation on the very inputs that were
    // seeded, at a now taken AFTER the section rendered (the word's
    // position in the order makes this timing-safe for every arm).
    let p = policy(if policy_yaml == POLICY_RECOVER_NONE {
        RecoveryMode::None
    } else {
        RecoveryMode::Probe
    });
    if matches!(seed, Seed::NeverSwitched) {
        // A family on its primary has no probe member to compare — the
        // caller asserts the null; the guard agreement below is for the
        // overflow arms only.
        return h;
    }
    let (admitted, blocked_by) = guard_answer(
        &p,
        PlanStateRow {
            account: PlanAccount::Overflow,
            since_us,
        },
        now_us_test(),
        primary_allowed,
        deferred_by_window,
    );
    assert_eq!(h["plan"]["configured"], true, "{tag}: configured");
    assert_eq!(
        h["plan"]["probe"]["admitted"].as_bool(),
        Some(admitted),
        "{tag}: admitted agrees with the guard"
    );
    let surface_word = h["plan"]["probe"]["blocked_by"]
        .as_str()
        .map(str::to_string);
    assert_eq!(
        surface_word, blocked_by,
        "{tag}: blocked_by agrees with the guard's own word"
    );
    h
}

fn vadis_store_now_us() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0)
}

fn now_us_test() -> i64 {
    vadis_store_now_us()
}

/// Minimal blocking GET (the CONF-25/41 style).
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

/// "YYYY-MM-DDTHH:MM:SS.mmmZ" → epoch milliseconds (test-side parser,
/// independent of the implementation's formatter — CONF-41's).
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
    vadis_core::peak::utc_midnight_epoch(y, mo, day) as i64 * 1_000
        + h * 3_600_000
        + mi * 60_000
        + s * 1_000
        + ms
}

/// The state matrix: five rows, one per §9.1 word plus the admitted row,
/// each live and each equal to the guard's independent answer. The four
/// overflow rows additionally witness the L1b side on the wire: with
/// `cooldown: 0s` the printed `deadline` differs from `since` by exactly
/// zero milliseconds — a unit drift here (µs printed as ms, or ×1_000_000
/// instead of ×1_000) would move that difference.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_71_blocked_by_matrix_report_and_guard_agree() {
    // A: never switched — primary, no probe member at all.
    let h = arm("conf71-a", POLICY, "", Seed::NeverSwitched).await;
    assert_eq!(h["plan"]["account"], "primary");
    assert_eq!(h["plan"]["probe"], serde_json::Value::Null);

    // B: overflow, nothing blocking — admitted, blocked_by null, and the
    // deadline is since + 0ms exactly.
    let h = arm("conf71-b", POLICY, "", Seed::OverflowSince(0)).await;
    assert_eq!(h["plan"]["account"], "overflow");
    let since = h["plan"]["since"].as_str().expect("since");
    let deadline = h["plan"]["probe"]["deadline"].as_str().expect("deadline");
    assert_eq!(
        parse_ms(deadline) - parse_ms(since),
        0,
        "cooldown 0s ⇒ deadline == since (parsed from the strings)"
    );

    // C: ADR-011 refuses the primary — the guard's third arm.
    let h = arm("conf71-c", POLICY, "", Seed::OverflowPrimaryCooling(0)).await;
    assert_eq!(h["plan"]["account"], "overflow");
    assert_eq!(h["plan"]["probe"]["blocked_by"], "primary_cooling_down");

    // D: recover: none — the guard's first arm, over everything else.
    let h = arm("conf71-d", POLICY_RECOVER_NONE, "", Seed::OverflowSince(0)).await;
    assert_eq!(h["plan"]["account"], "overflow");
    assert_eq!(h["plan"]["recover"], "none");
    assert_eq!(h["plan"]["probe"]["blocked_by"], "recovery_disabled");

    // E: the local counter's window has not reset — the guard's last arm,
    // through the real quota projection the seeded charge produced.
    let h = arm(
        "conf71-e",
        POLICY,
        QUOTA_100,
        Seed::OverflowWindowNotReset(0),
    )
    .await;
    assert_eq!(h["plan"]["account"], "overflow");
    assert_eq!(h["plan"]["probe"]["blocked_by"], "window_not_reset");
}
