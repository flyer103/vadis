//! CONF-114 (spec §9.4.2's block-order caveat / §9.4.6, ADR-057 §4 limb
//! (d)): **the cache column is the provider's own.** Every measured row
//! of the canned session prints `cached` equal to the mock's own
//! `usage.prompt_tokens_details.cached_tokens` (20) **verbatim** —
//! including the `repeat` row, whose block relation implies total reuse,
//! and the `rewrite` row, whose block relation implies almost none.
//! **Red** for a reducer that derives the cache figure from
//! `prefix.blocks`: the block order is vadis's own enumeration, not the
//! provider's template order, and on real codex traffic the two readings
//! were measured to disagree by a factor of four (ADR-057 §2.2 — the
//! metric-vs-verified-counter pitfall this repository already paid for
//! once).

#![forbid(unsafe_code)]

mod common;

use common::{run_steps_session, STEPS_SESSION};

/// The mock's own cached figure — the only cache number in the rig.
const MOCK_CACHED: u64 = 20;
const MOCK_IN: u64 = 100;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_114_the_cache_column_is_the_providers_own() {
    let (config_path, records) = common::run_steps_session("conf114", false).await;
    // Ground truth first: the records the writer wrote carry the mock's
    // counters verbatim, and so must the rows.
    for r in &records {
        if r["usage_missing"] == false {
            assert_eq!(r["usage"]["input_cached"].as_u64(), Some(MOCK_CACHED));
            assert_eq!(r["usage"]["input_total"].as_u64(), Some(MOCK_IN));
        }
    }
    let st = vadis_cli::steps::load_session(&config_path.to_string_lossy(), STEPS_SESSION)
        .expect("the session loads");
    let rep = vadis_cli::steps::reduce(&st).expect("the reducer runs");

    // Every measured row prints the provider's own counter — no matter
    // what the block relation implies.
    for row in &rep.rows[..6] {
        assert_eq!(row.cached, Some(MOCK_CACHED), "step {}", row.step);
        assert_eq!(row.r#in, Some(MOCK_IN), "step {}", row.step);
    }

    // The decisive rows, named:
    let repeat = &rep.rows[3];
    assert_eq!((repeat.kept, repeat.blocks), (3, 0));
    assert_eq!(
        repeat.cached,
        Some(MOCK_CACHED),
        "the repeat's block relation implies FULL reuse; the provider \
         said 20/100 — the two columns are independent facts"
    );
    let rewrite = &rep.rows[5];
    assert_eq!((rewrite.kept, rewrite.blocks), (1, 2));
    assert_eq!(
        rewrite.cached,
        Some(MOCK_CACHED),
        "the rewrite's block relation implies a near-full miss; the \
         provider said 20/100"
    );

    // The summary ratio is the same measured figures restated — a
    // ratio of counters, never a block-continuity figure, with the
    // excluded count beside it (§9.4.5).
    assert_eq!(rep.cache, Some((6 * MOCK_CACHED, 6 * MOCK_IN)));
    assert_eq!(
        rep.cache_excluded, 1,
        "the usage-less row is excluded, and said"
    );
}
