//! CONF-73 (ADR-016 §13.3 L1b, fixed this round): **the cooldown's
//! ms→µs conversion is anchored at a sub-second granularity, live.** The
//! single conversion (`PlanPolicyCfg::cooldown_us`) feeds three readers —
//! the guard's gate, the `/health` deadline, and the `plan.switched`
//! projection's informational `until_us` — and the failure class it
//! exists to prevent (R5-F3: two readers disagreeing about the unit)
//! only becomes *visible* when the configured cooldown is not a whole
//! number of seconds: `700ms` is 700_000µs, and a µs-as-ms or ×1_000_000
//! mistake moves the printed deadline by a factor no rounding can hide.
//!
//! Two rigs, three witnesses, all parsed/read back by test-side code:
//!
//! - rig A, the surface and the gate: `/health`'s
//!   `probe.deadline − plan.since == 700ms` exactly; inside the window a
//!   fresh session's boundary does NOT reach the primary (served by the
//!   overflow), after `700ms` (+ the fixture's 1s demotion) it DOES —
//!   the gate that refused is the gate that then admitted, one unit.
//! - rig B, the projection: a spill-only run (no probe to flip the row
//!   back), the store read after the server stopped, the
//!   `plan_state.until_us − since_us == 700_000µs` — the third reader,
//!   the same unit.
//!
//! The counterweight is structural: `cooldown: 0s` cannot fail this case
//! (deadline == since either way), which is why the knob here is 700ms.

#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::net::TcpStream;

use router_conformance::testkit::{self, PlanRig};
use router_core::store::{Query, QueryRow, Store as _};

const POLICY_700MS: &str = "  family: m1\n  primary: p-plan/m1\n  overflow: p-api/m1\n  on_primary_exhausted: spill\n  recover: probe\n  cooldown: 700ms";

async fn rig(tag: &str) -> PlanRig {
    let (plan, api, dir, listen_addr) = testkit::plan_rig_parts(tag, "", POLICY_700MS).await;
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

/// Spill the family on a fresh rig: t1 answers, turn 2 meets the 403,
/// the overflow serves the spill.
async fn spill(rig: &PlanRig) {
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_forbidden_403());
    rig.api.queue(testkit::plan_ok("spilled"));
    let (s1, _b, _h) = rig.post(Some("S1"), 1);
    assert_eq!(s1, 200);
    let (s2, _b2, _h2) = rig.post(Some("S1"), 2);
    assert_eq!(s2, 200, "the 403 spills the family to overflow");
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

/// Rig A: the surface and the request path name the same 700ms boundary.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_73_surface_and_gate_share_the_sub_second_boundary() {
    let rig = rig("conf73-gate").await;
    // All plan responses queued up front (the mock answers an empty queue
    // with a loud 500 — CONF-39's authoring rule): t1, the 403, the
    // probe 200 for the post-window boundary.
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_forbidden_403());
    rig.plan.queue(testkit::plan_ok("probe"));
    spill(&rig).await;

    // Witness 1 — the surface: deadline - since is exactly the loaded
    // 700ms, parsed back from the two RFC3339 strings.
    let h = http_get(&rig.listen_addr, "/health");
    assert_eq!(h["plan"]["account"], "overflow");
    let since_ms = parse_ms(h["plan"]["since"].as_str().expect("since"));
    let deadline_ms = parse_ms(h["plan"]["probe"]["deadline"].as_str().expect("deadline"));
    assert_eq!(
        deadline_ms - since_ms,
        700,
        "L1b witness 1 (surface): the deadline is since + exactly 700ms"
    );

    // Witness 2 — the request path agrees on the same boundary: inside
    // the 700ms window a fresh session's boundary does NOT reach the
    // primary (the overflow serves it).
    rig.api.queue(testkit::plan_ok("inside-window"));
    let (se, _be, _he) = rig.post(Some("S2"), 1);
    assert_eq!(se, 200, "served — by the overflow account");
    assert_eq!(
        rig.plan.requests().len(),
        2,
        "the gate refused the probe with the same 700ms unit"
    );
    assert_eq!(rig.api.requests().len(), 2);

    // Past the 700ms deadline and the 1s fixture demotion: the boundary
    // IS the probe and the primary answers it.
    tokio::time::sleep(std::time::Duration::from_millis(1_800)).await;
    let (sp, _bp, _hp) = rig.post(Some("S3"), 1);
    assert_eq!(sp, 200);
    assert_eq!(
        rig.plan.requests().len(),
        3,
        "the same gate admitted once the same 700ms had passed"
    );
    assert_eq!(rig.api.requests().len(), 2, "no metered spend");

    rig.stop();
}

/// Rig B: the projection — the `plan.switched` row's informational
/// `until_us` is `since_us + 700_000µs`, the third reader of the one
/// conversion. Spill-only run (no probe: a successful probe would flip
/// the row back to 'primary' and NULL its `until_us` before it can be
/// read), store opened read-only after the server stopped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_73_projection_until_us_is_since_plus_700ms() {
    let rig = rig("conf73-proj").await;
    spill(&rig).await;
    let dir = rig.stop();

    let store = router_store::SqliteStore::open_read_only(&dir.join("state/router.db")).unwrap();
    let QueryRow::PlanState(Some(row)) = store.query(Query::PlanState { family: "m1" }).unwrap()
    else {
        panic!("plan_state row after the spill");
    };
    assert_eq!(row.account, "overflow", "no probe ran to flip it back");
    let until = row
        .until_us
        .expect("an overflow family with a 700ms cooldown has a deadline");
    assert_eq!(
        until - row.since_us,
        700_000,
        "L1b witness 3 (projection): until_us - since_us is exactly 700_000µs"
    );
}
