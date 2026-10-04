//! CONF-40 (spec §4.6 rule 4 / ADR-014 item 6): **the account a request
//! was served under decides its books.** An in-plan request — one whose
//! serving provider declares `account: coding_plan` — is accounted at the
//! plan's **marginal cost 0** (every cost bucket and `total`), while its
//! `quota_after` is still recorded when the provider declares a plan
//! (rule 4: "…and record `quota_after`"). An overflow request — provider
//! `account: api` — is priced at the model's real price through the
//! ordinary five-tier path. Asserting both in one session makes the
//! evidence bidirectional: a bug that zeroed everything (or priced
//! everything) cannot pass.
//!
//! The zero holds for a plan whose allowance is **not published** too
//! (no `quota` on the provider — spec §4.6: "a plan whose token allowance
//! is not published is still a plan"), which is why the second case runs
//! the spill on a quota-less rig: the spilled turn prices real, the
//! in-plan turn before it still zeros.

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, PlanRig};

async fn rig(tag: &str, quota_yaml: &str) -> PlanRig {
    let (plan, api, dir, listen_addr) =
        testkit::plan_rig_parts(tag, quota_yaml, testkit::PLAN_POLICY_DEFAULT).await;
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    PlanRig {
        plan,
        api,
        listen_addr,
        dir,
        serve_task,
    }
}

/// A quota plan for p-plan covering m1, sized so the case's usage stays
/// inside the allowance (the verdict is `ok`, not the blocking path —
/// CONF-35 owns that).
const QUOTA: &str = "\n    quota:\n      - models: [m1]\n        window: monthly\n        tokens: 100000\n        reset_day: 1\n        over_quota: block\n        source: \"fixture: published allowance (CONF-40)\"";

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

fn record_for<'a>(
    recs: &'a [serde_json::Value],
    session: &str,
    turn: u32,
) -> &'a serde_json::Value {
    recs.iter()
        .find(|r| r["identity"]["session"] == session && r["identity"]["turn_index"] == turn)
        .unwrap_or_else(|| panic!("record for {session} turn {turn}"))
}

/// The bidirectional pair, on a rig whose p-plan **declares** its quota:
/// turn 1 is served in-plan and books all-zero cost **with**
/// `quota_after` still recorded; turn 2's 403 spills to the metered
/// account and books the same usage at the real five-tier price
/// (non-zero). Same usage on both turns — the only variable that
/// changed is the serving account.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_40_in_plan_books_zero_with_quota_overflow_books_real() {
    let rig = rig("conf40-pair", QUOTA).await;
    rig.plan.queue(testkit::plan_ok("from-plan"));
    rig.plan.queue(testkit::plan_forbidden_403());
    rig.api.queue(testkit::plan_ok("from-api"));

    let (s1, _b1, _h1) = rig.post(Some("S1"), 1);
    assert_eq!(s1, 200, "the plan account answered turn 1");
    let (s2, _b2, _h2) = rig.post(Some("S1"), 2);
    assert_eq!(s2, 200, "the overflow account answered the spill");

    let dir = rig.stop();
    let recs = trace_records(&dir);

    // In-plan: every bucket zero (the plan's marginal cost is 0, not
    // "small"), and the allowance is still booked.
    let in_plan = record_for(&recs, "S1", 1);
    assert_eq!(in_plan["decision"]["provider"], "p-plan");
    assert_eq!(
        in_plan["cost"]["total"], 0,
        "in-plan total is the plan's marginal cost 0"
    );
    assert_eq!(in_plan["cost"]["input_miss"], 0);
    assert_eq!(in_plan["cost"]["input_hit"], 0);
    assert_eq!(in_plan["cost"]["cache_write"], 0);
    assert_eq!(in_plan["cost"]["output"], 0);
    let qa = &in_plan["cost"]["quota_after"];
    assert!(
        qa.is_object(),
        "quota_after is still recorded for an in-plan request (rule 4), got {qa}"
    );
    assert_eq!(qa["provider"], "p-plan");
    assert_eq!(qa["verdict"], "inside");
    // The usage itself is untouched — only the money was zeroed.
    assert_eq!(in_plan["usage"]["input_total"], 100);
    assert_eq!(in_plan["usage"]["output"], 5);

    // Overflow: the same usage at the metered table is non-zero (80
    // miss x 2000 + 20 hit x 200 + 5 out x 4000 = 184000 nano) — the
    // paired assertion that "zero everything" cannot satisfy.
    let spilled = record_for(&recs, "S1", 2);
    assert_eq!(spilled["decision"]["provider"], "p-api");
    assert_eq!(
        spilled["cost"]["total"], 184_000,
        "overflow books the real five-tier price"
    );
}

/// The zero is a property of the account, not of a published allowance:
/// with **no** `quota` on the coding-plan provider the in-plan request
/// still books 0 (and records no `quota_after` — there is no plan to
/// book; spec §4.6's GAP-Q1 preamble makes the request legal).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_40_unpublished_allowance_still_zeros() {
    let rig = rig("conf40-noquota", "").await;
    rig.plan.queue(testkit::plan_ok("from-plan"));

    let (s1, _b1, _h1) = rig.post(Some("S2"), 1);
    assert_eq!(s1, 200);

    let dir = rig.stop();
    let recs = trace_records(&dir);
    let in_plan = record_for(&recs, "S2", 1);
    assert_eq!(in_plan["decision"]["provider"], "p-plan");
    assert_eq!(
        in_plan["cost"]["total"], 0,
        "no published allowance, still marginal cost 0"
    );
    assert!(
        in_plan["cost"]["quota_after"].is_null(),
        "no declared plan means no quota_after to record"
    );
}
