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
//!
//! R58 — how the reads are asserted without racing the seeded boundary
//! (the R56-1 load reds at :104 and :133 — red on byte-identical input,
//! green on the rerun, the same class R56 fixed in conf_72/conf_73).
//! The old text *assumed* three successive operations (the seeded row's
//! read-back, a whole `serve` startup, and a blocking HTTP GET) would
//! always land inside the 700ms window ("the read itself takes
//! ≪700ms"); under load the row legitimately expires through
//! `Query::Cooldown`'s `until_us > now` filter and each assumption went
//! red at a different site. The fixed case never assumes a position in
//! time:
//!
//! - the seeded row's read-back is **bracketed** (`tb0`/`tb1` around the
//!   query): a live row asserts its `until_us` is byte-identical to the
//!   seeded word; an absent row must be *explained* by the bracket
//!   (`tb1 >= boundary` is implied by the filter, so a row dropped
//!   **while still live** — `tb1 < boundary` — is a product bug and
//!   reds);
//! - every `/health` read is bracketed (`t0`/`t1` around the round trip,
//!   one clock, one machine) and the live word is asserted only where
//!   the bracket *proves* the position against the seeded boundary —
//!   provably inside forces `primary_cooling_down`, provably past
//!   forbids it, and a straddling run still asserts the vocabulary and
//!   the `admitted ⟺ no word` self-consistency;
//! - the store-side properties the live arm samples probabilistically
//!   are pinned **deterministically** at fixed, test-controlled instants
//!   (`store_word_pins`: a sub-second µs word survives the projection
//!   round-trip exactly, and an expired row is absent from the filtered
//!   read — no wall clock at all);
//! - the after-boundary read keeps its oversleep-forced position (the
//!   sleep can only overshoot, so "past" is a property of the seeded
//!   boundary, not of scheduling).
//!
//! No window was widened (the seeded offset is still 700ms), no
//! assertion sleeps easier: each fires at least as strictly as the one
//! it replaces, and the falsifiability control lives under
//! `autowork/harness/r58-0/`.

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

/// The deterministic store pins, at fixed test-controlled instants (no
/// wall clock anywhere): the properties the live arm can only sample.
///
/// - Pin 1 (L1b, the word): a sub-second µs `until_us` survives the
///   projection write + filtered read **byte-identically** — a store
///   that truncated to whole seconds would lose the `.700000` fraction.
/// - Pin 2 (the filter): the SAME row, overwritten to a fixed PAST
///   instant, is absent from `Query::Cooldown` — the liveness filter
///   (`until_us > now`), not the write path (pin 1 just proved it),
///   answers `None`. This is the row-disappearance the old read-back
///   raced against.
fn store_word_pins() {
    let dir = testkit::tempdir("conf76-pins");
    let store = router_store::SqliteStore::open(&dir.join("state/router.db")).unwrap();
    let ev = store
        .append(NewEvent {
            kind: EventKind::PlanSwitched,
            request_id: Some("conf-76-pins"),
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
    // 2030-03-25T05:26:40.700000Z — far future, fraction .700000s.
    const U_FUTURE: i64 = 1_900_000_000_700_000;
    store
        .project(ProjectionWrite::ProviderCooldown {
            scope: "provider",
            provider: "p-plan",
            model: "",
            until_us: U_FUTURE,
            reason: "quota_exhausted",
            last_event: ev,
        })
        .unwrap();
    match store.query(Query::Cooldown {
        provider: "p-plan",
        model: None,
    }) {
        Ok(QueryRow::Cooldown(Some(row))) => assert_eq!(
            row.until_us, U_FUTURE,
            "L1b on the store: the sub-second µs word round-trips byte-identically"
        ),
        other => panic!("a live seeded row is present: {other:?}"),
    }
    // 2001-09-09T01:46:40.700000Z — far past, same fraction.
    const U_PAST: i64 = 1_000_000_000_700_000;
    store
        .project(ProjectionWrite::ProviderCooldown {
            scope: "provider",
            provider: "p-plan",
            model: "",
            until_us: U_PAST,
            reason: "quota_exhausted",
            last_event: ev,
        })
        .unwrap();
    match store.query(Query::Cooldown {
        provider: "p-plan",
        model: None,
    }) {
        Ok(QueryRow::Cooldown(None)) => {}
        other => panic!("an expired row is absent from the filtered read by design: {other:?}"),
    }
}

/// Read the seeded row's `until_us` back from the projection — never
/// trust the seed's own arithmetic. The read is **bracketed**: a
/// present row must carry the seeded word byte-identically; an absent
/// row is legitimate only when the read's own bracket explains the
/// expiry (the filter drops the row at `now >= until`, and the query's
/// `now` precedes the bracket's end — so `None` with `tb1 < until`
/// means a live row was dropped: a product bug, red).
fn seeded_until_us(dir: &std::path::Path, until_us: i64) {
    let tb0 = now_us();
    let store = router_store::SqliteStore::open(&dir.join("state/router.db")).unwrap();
    let row = store.query(Query::Cooldown {
        provider: "p-plan",
        model: None,
    });
    let tb1 = now_us();
    match row {
        Ok(QueryRow::Cooldown(Some(row))) => assert_eq!(
            row.until_us, until_us,
            "the seeded µs word is the word read back"
        ),
        Ok(QueryRow::Cooldown(None)) => assert!(
            tb1 >= until_us,
            "the seeded row expired BEFORE its until: the bracket \
             [{tb0}, {tb1}] never reached {until_us}"
        ),
        other => panic!("cooldown row after seed: {other:?}"),
    }
}

/// The live arm: seed first (writer lock released by dropping the
/// handle), then serve, then read `/health` on both sides of the
/// sub-second boundary — each read bracketed, each word asserted only
/// where the bracket proves the position.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_76_live_cooldown_boundary_is_exclusive_and_microsecond() {
    // The deterministic pins first: no clock, no load sensitivity.
    store_word_pins();

    let (plan, api, dir, listen_addr) =
        testkit::plan_rig_parts("conf76-seeded", "", testkit::PLAN_POLICY_DEFAULT).await;
    let cfg_path = dir.join("config.yaml");
    let cfg = cfg_path.to_string_lossy().into_owned();
    let boundary_us = now_us() + 700_000;
    seed(&dir, Some(boundary_us));
    seeded_until_us(&dir, boundary_us);
    let serve_task = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    let rig = PlanRig {
        plan,
        api,
        listen_addr,
        dir,
        serve_task,
    };

    // Read 1, bracketed [t0, t1]: the section's single evaluation
    // instant lies inside the bracket. The live word is asserted only
    // where the bracket PROVES the position against the seeded
    // boundary — never assumed from the wall clock (the R56-1 reds at
    // :104/:133 were exactly that assumption).
    let t0 = now_us();
    let h1 = http_get(&rig.listen_addr, "/health");
    let t1 = now_us();
    assert_eq!(h1["plan"]["account"], "overflow");
    let blocked_by = &h1["plan"]["probe"]["blocked_by"];
    let admitted = &h1["plan"]["probe"]["admitted"];
    if t1 < boundary_us {
        // Provably inside (evaluation instant <= t1 < boundary): the
        // word is forced — the old :133 assertion, now position-proven.
        eprintln!("conf76 read1 branch: provably-inside");
        assert_eq!(
            *blocked_by, "primary_cooling_down",
            "provably inside the seeded cooldown: the surface must name it"
        );
        assert_eq!(*admitted, false);
    } else if t0 >= boundary_us {
        // Provably past (evaluation instant >= t0 >= boundary): the row
        // has legitimately expired; the surface must admit.
        eprintln!("conf76 read1 branch: provably-past");
        assert_eq!(
            *blocked_by,
            serde_json::Value::Null,
            "provably past the seeded cooldown: no refusal word"
        );
        assert_eq!(*admitted, true);
    } else {
        // The bracket straddles the boundary: no position is provable.
        // What still holds at every instant: the surface speaks only
        // this configuration's vocabulary, and it agrees with itself.
        eprintln!("conf76 read1 branch: straddle");
        assert_eq!(
            admitted.as_bool().expect("admitted is a bool"),
            blocked_by.is_null(),
            "admitted and blocked_by are the same fact on the surface"
        );
        if let Some(w) = blocked_by.as_str() {
            assert_eq!(
                w, "primary_cooling_down",
                "the only refusal word this configuration can produce, got {w:?}"
            );
        }
    }

    // After the boundary: the same section, the same state, admits. The
    // sleep can only overshoot, so read 2's position (past) is a
    // property of the seeded boundary, not of scheduling; if read 1 was
    // already past, no wait is needed at all.
    let wait_us = boundary_us - now_us();
    if wait_us > 0 {
        tokio::time::sleep(std::time::Duration::from_micros(
            (wait_us + 50_000).max(1_000) as u64,
        ))
        .await;
    }
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
