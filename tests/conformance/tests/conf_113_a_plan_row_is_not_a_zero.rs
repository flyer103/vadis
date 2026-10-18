//! CONF-113 (spec §9.4.3 column 8 / §9.4.5, GAP-Q17, ADR-057 §4 limb
//! (c)): **a plan row is not a zero.** The rig's first request is
//! served by a provider whose roster entry declares `account:
//! coding_plan`; its row prints the literal `plan` in the money column —
//! the allowance headroom it consumed is not a price this product can
//! state — and the summary counts it in `plan rows`, adding it to **no**
//! currency line. **Red** for a reducer that prints `0.0000` there:
//! ADR-057 §1.3's second finding — 52.4 % of a real window's
//! miss-token volume sat on plan routes whose cost is headroom, not
//! money — is exactly the misreading this limb exists to catch.

#![forbid(unsafe_code)]

mod common;

use common::{run_steps_session, STEPS_SESSION};
use vadis_cli::steps::Money;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_113_a_plan_row_is_not_a_zero() {
    let (config_path, records) = common::run_steps_session("conf113", false).await;
    let st = vadis_cli::steps::load_session(&config_path.to_string_lossy(), STEPS_SESSION)
        .expect("the session loads");
    let rep = vadis_cli::steps::reduce(&st).expect("the reducer runs");

    // The plan row: step 1, on the coding_plan provider — measured
    // usage (the provider answered with one), a `plan` money cell.
    let row = &rep.rows[0];
    assert_eq!(row.route.as_deref(), Some("p-plan/m1"));
    assert_eq!(row.r#in, Some(100), "the plan row is measured");
    assert_eq!(row.cached, Some(20));
    assert_eq!(row.out, Some(5));
    assert_eq!(
        row.money,
        Some(Money::Plan),
        "the money cell is the label, never a numeric zero"
    );

    // The summary counts it as a plan row and adds it to no currency.
    assert_eq!(rep.plan_rows, 1);
    assert_eq!(rep.by_class["first"].plan_rows, 1);
    let expected_usd: u64 = records
        .iter()
        .filter(|r| r["decision"]["provider"] == "mock" && r["usage_missing"] == false)
        .map(|r| r["cost"]["total"].as_u64().unwrap_or(0))
        .sum();
    assert_eq!(
        rep.money_nano.get("USD").copied(),
        Some(expected_usd),
        "the plan row's own cost.total stayed out of the currency line"
    );

    // The label is CONFIG-derived, not route-name-derived: a provider
    // named p-plan would be a plan provider even under another name —
    // and the mock (account: api by absence) never earns the label.
    for row in &rep.rows[1..] {
        assert!(
            !matches!(row.money, Some(Money::Plan)),
            "no metered row wears the plan label (step {})",
            row.step
        );
    }
}
