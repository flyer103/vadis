//! CONF-76 (ADR-016 §13.3 L1c, fixed this round): **the cooldown read's
//! clock semantics — one read, one instant, exclusive boundary —
//! witnessed live at sub-second granularity.** The two former
//! implementations used different clocks (`forward.rs` read `now_us()`
//! at its own instant; `health.rs` compared against a `now` captured
//! earlier for the section), so a cooldown boundary could pass between
//! the two reads and the report would say `primary_cooling_down` while
//! the request path already admitted — or vice versa. Both now call the
//! single owner (`availability::provider_in_cooldown`) against a clock
//! word the caller supplies, so within one evaluation there is exactly
//! one instant and no private second clock.
//!
//! The witness seeds a cooldown row whose `until_us` lands at a
//! **sub-second offset** (now + 700ms — a whole-seconds boundary could
//! not tell a µs-word from a truncated s-word, the L1b class of drift),
//! then reads the live `/health` **before** and **after** the boundary:
//! the first read reports `primary_cooling_down`, the second reports
//! the probe admitted — the boundary is exclusive and the µs word
//! decides, not the truncated seconds word. The control seeds no
//! cooldown row at all: the same state reports admitted immediately
//! (the arm's value is the projection row's, not the state's).

#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::net::TcpStream;

use router_conformance::testkit::{self, PlanRig};
use router_core::store::{EventKind, NewEvent, ProjectionWrite};
use router_core::store::{Query, QueryRow, Store as _};

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

/// Seed the family to overflow plus (optionally) a provider cooldown on
/// p-plan expiring at `until_us`, before `serve` opens the store (the
/// EXCLUSIVE writer lock forecloses any later writer).
fn seed(dir: &std::path::Path, cooldown_until_us: Option<i64>) {
    let store = router_store::SqliteStore::open(&dir.join("state/router.db")).unwrap();
    let ev = store
        .append(NewEvent {
            kind: EventKind::PlanSwitched,
            request_id: Some("conf-76-seed"),
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
    if let Some(until_us) = cooldown_until_us {
        store
            .project(ProjectionWrite::ProviderCooldown {
                scope: "provider",
                provider: "p-plan",
                model: "",
                until_us,
                reason: "quota_exhausted",
                last_event: ev,
            })
            .unwrap();
    }
    drop(store);
}

/// Read the seeded row's `until_us` back from the projection — never
/// trust the seed's own arithmetic.
fn seeded_until_us(dir: &std::path::Path) -> i64 {
    let store = router_store::SqliteStore::open(&dir.join("state/router.db")).unwrap();
    match store.query(Query::Cooldown {
        provider: "p-plan",
        model: None,
    }) {
        Ok(QueryRow::Cooldown(Some(row))) => row.until_us,
        other => panic!("cooldown row after seed: {other:?}"),
    }
}

/// The live arm: seed first (writer lock released by dropping the
/// handle), then serve, then read `/health` on both sides of the
/// sub-second boundary.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_76_live_cooldown_boundary_is_exclusive_and_microsecond() {
    let (plan, api, dir, listen_addr) =
        testkit::plan_rig_parts("conf76-seeded", "", testkit::PLAN_POLICY_DEFAULT).await;
    let cfg_path = dir.join("config.yaml");
    let cfg = cfg_path.to_string_lossy().into_owned();
    let boundary_us = now_us() + 700_000;
    seed(&dir, Some(boundary_us));
    assert_eq!(seeded_until_us(&dir), boundary_us);
    let serve_task = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    let rig = PlanRig {
        plan,
        api,
        listen_addr,
        dir,
        serve_task,
    };

    // Before the boundary (the read itself takes ≪700ms): refused.
    let h1 = http_get(&rig.listen_addr, "/health");
    assert_eq!(h1["plan"]["account"], "overflow");
    assert_eq!(h1["plan"]["probe"]["blocked_by"], "primary_cooling_down");
    assert_eq!(h1["plan"]["probe"]["admitted"], false);

    // After the boundary: the same section, the same state, admits.
    let wait_us = boundary_us - now_us();
    assert!(
        wait_us > 0,
        "fixture invariant: read 1 was before the boundary"
    );
    tokio::time::sleep(std::time::Duration::from_micros(
        (wait_us + 50_000).max(1_000) as u64,
    ))
    .await;
    let h2 = http_get(&rig.listen_addr, "/health");
    assert_eq!(h2["plan"]["probe"]["blocked_by"], serde_json::Value::Null);
    assert_eq!(h2["plan"]["probe"]["admitted"], true);

    rig.stop();
}

/// The control: no cooldown row at all — the same overflow state admits
/// immediately, so the arm's value in the live arm was the projection
/// row's, not the state's or the clock's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_76_control_no_cooldown_row_means_no_refusal() {
    let (plan, api, dir, listen_addr) =
        testkit::plan_rig_parts("conf76-ctrl", "", testkit::PLAN_POLICY_DEFAULT).await;
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    seed(&dir, None);
    let serve_task = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    let rig = PlanRig {
        plan,
        api,
        listen_addr,
        dir,
        serve_task,
    };

    let h = http_get(&rig.listen_addr, "/health");
    assert_eq!(h["plan"]["account"], "overflow");
    assert_eq!(
        h["plan"]["probe"]["admitted"], true,
        "no cooldown row, no refusal"
    );

    rig.stop();
}
