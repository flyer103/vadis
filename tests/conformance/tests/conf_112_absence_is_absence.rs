//! CONF-112 (spec §9.4.3's absence rule, ADR-057 §4 limb (b)): **an
//! absent measurement is an absence.** The rig's seventh request is
//! answered 200 with no usage block; its row must print `—`/null in
//! `in` / `cached` / `out` / `money`, carry the `no_usage` flag, and be
//! counted in the summary's `usage missing` — while the three outcome
//! buckets stay untouched (`usage_missing` is ADR-056 §2.4's orthogonal
//! second partition, never a fourth bucket) and no money sum receives a
//! contribution. **Red** for a reducer that prints `0` (the
//! "absent-is-not-zero" class, §7).

#![forbid(unsafe_code)]

mod common;

use common::{run_steps_session, STEPS_SESSION};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_112_an_absent_measurement_is_an_absence() {
    let (config_path, records) = common::run_steps_session("conf112", false).await;
    let st = vadis_cli::steps::load_session(&config_path.to_string_lossy(), STEPS_SESSION)
        .expect("the session loads");
    let rep = vadis_cli::steps::reduce(&st).expect("the reducer runs");

    // The usage-less row: step 7 of the canned script.
    let row = &rep.rows[6];
    assert_eq!(row.class, "append");
    assert_eq!(row.r#in, None, "absent, never 0");
    assert_eq!(row.cached, None, "absent, never 0");
    assert_eq!(row.out, None, "absent, never 0");
    assert_eq!(row.money, None, "the cost was never computed (§7)");
    assert!(
        row.flags.iter().any(|f| f == "no_usage"),
        "the row names its own absence: {:?}",
        row.flags
    );

    // The summary: one usage-missing row; the three buckets unchanged
    // by it (it is a clean 2xx — a SUCCESS that carries no figures).
    assert_eq!(rep.usage_missing, 1);
    assert_eq!(rep.rows.len(), 7);
    assert_eq!(rep.succeeded, 7, "the usage-less row rides a success");
    assert_eq!(rep.failed, 0);
    assert_eq!(rep.stream_incomplete, 0);
    assert_eq!(
        rep.succeeded + rep.failed + rep.stream_incomplete,
        rep.rows.len() as u64,
        "the three buckets PARTITION steps (ADR-056 §2.4)"
    );

    // No money sum received a contribution from it: the summary's USD
    // line is exactly the measured metered rows' own `cost.total` sum
    // (steps 2-6 — step 1 is the plan row, counted as `plan`, never
    // added to a currency line; §9.4.5).
    let expected: u64 = records
        .iter()
        .filter(|r| r["decision"]["provider"] == "mock" && r["usage_missing"] == false)
        .map(|r| r["cost"]["total"].as_u64().unwrap_or(0))
        .sum();
    assert_eq!(
        rep.money_nano.get("USD").copied(),
        Some(expected),
        "the usage-less row contributed nothing to the money sum"
    );

    // And the cache ratio excludes it, saying so.
    assert_eq!(rep.cache_excluded, 1);
    assert_eq!(rep.cache, Some((6 * 20, 6 * 100)));
}
