//! CONF-78 (ADR-016 §13.3 L1d, fixed this round): **the window
//! verdict's adjudication matrix at the report's own clock granularity
//! — the live section equals the test's own derivation of the same
//! adjudication from the very rows it seeded.** The former duplication
//! restated the whole rule (plan lookup, window start, `next_reset`,
//! `Query::QuotaUsed`, `used >= tokens`) in two bodies; this case pins
//! each adjudication input live, on the real `serve` assembly, by
//! seeding the projections before `serve` opens the store (the EXCLUSIVE
//! writer lock forecloses any later writer):
//!
//! - **the read is plan-scoped** — a provider with two declared plans
//!   covering the family: an exhausted second plan defers even while
//!   the first has plenty (the read goes through the covering plan's
//!   own `plan_idx` and window), and a charge that exhausts neither
//!   admits;
//! - **`used == tokens` defers, `used == tokens - 1` does not** — the
//!   comparison is `>=`, pinned at the boundary;
//! - **the read is window-scoped** — the same exhaustion charged in
//!   the *previous* window leaves the current window unexhausted and
//!   the probe admits (the control).
//!
//! Every arm is time-independent (the runs sit mid-window with
//! `reset_day: 1`, `cooldown: 0s`), so no sleep and no racing clock:
//! expectations are computed from rows the test wrote and read back.

#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::net::TcpStream;

use vadis_conformance::testkit;
use vadis_core::store::{EventKind, NewEvent, ProjectionWrite};
use vadis_core::store::{Query, QueryRow, Store as _};

/// Two plans on p-plan, both covering the family: a 1000-token plan
/// (idx 0) and a 100-token plan (idx 1). The read must answer per plan:
/// exhausting idx 1 defers while idx 0 still has plenty, and vice
/// versa a charge inside idx 0's allowance admits.
const QUOTA_TWO_PLANS: &str = "\n    quota:\n      - models: [m1]\n        window: monthly\n        tokens: 1000\n        reset_day: 1\n        over_quota: block\n        source: \"fixture: the roomy plan\"\n      - models: [m1]\n        window: monthly\n        tokens: 100\n        reset_day: 1\n        over_quota: block\n        source: \"fixture: the tight plan\"";

/// One 99-token plan covering the family: arms B/C charge exactly 99
/// (== the allowance) and 98 (one short).
const QUOTA_99: &str = "\n    quota:\n      - models: [m1]\n        window: monthly\n        tokens: 99\n        reset_day: 1\n        over_quota: block\n        source: \"fixture: the comparison boundary\"";

/// One 100-token plan covering the family (the window-scoping control
/// charges it in the previous window).
const QUOTA_100: &str = "\n    quota:\n      - models: [m1]\n        window: monthly\n        tokens: 100\n        reset_day: 1\n        over_quota: block\n        source: \"fixture: the window-scoping control\"";

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

/// The current monthly window's start (reset_day 1) for `now`, and the
/// previous window's start (31 days earlier lands in the prior window).
fn window_starts() -> (u64, u64) {
    let now_s = (now_us().max(0) as u64) / 1_000_000;
    let cur = vadis_core::quota::window_start_for(now_s, 1);
    let prev = vadis_core::quota::window_start_for(now_s.saturating_sub(31 * 86_400), 1);
    (cur, prev)
}

/// Seed the family to overflow and charge `tokens` against plan
/// `plan_idx`'s window at `window_start_s`. Returns the used count read
/// back from the projection for the CURRENT window of that plan (the
/// raw fact the adjudication reads — never the seed's own arithmetic).
fn seed_and_read_used(
    dir: &std::path::Path,
    plan_idx: u32,
    tokens: i64,
    window_start_s: u64,
) -> u64 {
    let store = vadis_store::SqliteStore::open(&dir.join("state/router.db")).unwrap();
    let ev = store
        .append(NewEvent {
            kind: EventKind::PlanSwitched,
            request_id: Some("conf-78-seed"),
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
    store
        .project(ProjectionWrite::QuotaCharged {
            provider: "p-plan",
            plan_idx,
            window_start_us: window_start_s as i64 * 1_000_000,
            tokens,
            last_event: ev,
        })
        .unwrap();
    // The adjudication's own read: the CURRENT window's counter. The
    // absent-row convention is the single owner's own (`_ => 0`; the
    // store answers "no rows" for a window without a charge — the
    // control's whole point).
    let (cur, _) = window_starts();
    let used = match store.query(Query::QuotaUsed {
        provider: "p-plan",
        plan_idx,
        window_start_us: cur as i64 * 1_000_000,
    }) {
        Ok(QueryRow::Count(n)) => n.max(0) as u64,
        _ => 0,
    };
    drop(store);
    used
}

/// One arm: build the rig, seed, serve, read the live section. Returns
/// (the section, the current-window used count the test read back).
async fn arm(
    tag: &str,
    quota_yaml: &str,
    plan_idx: u32,
    tokens: i64,
    window_start_s: u64,
) -> (serde_json::Value, u64) {
    let (_plan, _api, dir, listen_addr) =
        testkit::plan_rig_parts(tag, quota_yaml, testkit::PLAN_POLICY_DEFAULT).await;
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let used = seed_and_read_used(&dir, plan_idx, tokens, window_start_s);
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    let h = http_get(&listen_addr, "/health");
    serve_task.abort();
    drop(serve_task);
    (h, used)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_78_window_adjudication_matrix_at_the_report_clock() {
    let (cur, _prev) = window_starts();

    // A1: the tight plan (idx 1, 100 tokens) charged to exactly its
    // allowance while the roomy plan (idx 0, 1000) is untouched — the
    // read is plan-scoped: the exhausted covering plan defers.
    let (h, used) = arm("conf78-a1", QUOTA_TWO_PLANS, 1, 100, cur).await;
    assert_eq!(used, 100, "the seed's charge, read back");
    assert_eq!(h["plan"]["account"], "overflow");
    assert_eq!(h["plan"]["probe"]["blocked_by"], "window_not_reset");
    assert_eq!(h["plan"]["probe"]["admitted"], false);

    // A2: the mirror — the roomy plan (idx 0) charged 100 of 1000 and
    // the tight plan untouched: nothing is exhausted, the probe admits.
    // (The read answered per plan; a lifetime total across plans would
    // see 200 of nothing and this arm could not pass both ways.)
    let (h, used) = arm("conf78-a2", QUOTA_TWO_PLANS, 0, 100, cur).await;
    assert_eq!(used, 100);
    assert_eq!(h["plan"]["probe"]["blocked_by"], serde_json::Value::Null);
    assert_eq!(h["plan"]["probe"]["admitted"], true);

    // B: exactly at the allowance — `used == tokens` defers.
    let (h, used) = arm("conf78-b", QUOTA_99, 0, 99, cur).await;
    assert_eq!(used, 99);
    assert_eq!(h["plan"]["probe"]["blocked_by"], "window_not_reset");

    // C: one token short — `used == tokens - 1` admits.
    let (h, used) = arm("conf78-c", QUOTA_99, 0, 98, cur).await;
    assert_eq!(used, 98);
    assert_eq!(h["plan"]["probe"]["blocked_by"], serde_json::Value::Null);
    assert_eq!(h["plan"]["probe"]["admitted"], true);
}

/// The control: the read is window-scoped — the same exhaustion charged
/// in the PREVIOUS window leaves the current window empty and the probe
/// admits, whatever the old window's counter says.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_78_control_a_previous_window_charge_never_defers() {
    let (_cur, prev) = window_starts();
    let (h, used) = arm("conf78-ctrl", QUOTA_100, 0, 100, prev).await;
    assert_eq!(
        used, 0,
        "the current window has no row: the charge landed in the previous one"
    );
    assert_eq!(h["plan"]["account"], "overflow");
    assert_eq!(h["plan"]["probe"]["blocked_by"], serde_json::Value::Null);
    assert_eq!(h["plan"]["probe"]["admitted"], true);
}
