//! CONF-32 (spec §4.6 / ADR-014 items 4–5): **plan-first priority and the
//! priced spill.** While the family's account state is `primary` a family
//! request is served by the primary route's own `urls` entry for that wire; when the primary
//! answers `403 quota_exhausted` the request spills to the overflow route,
//! the account move is recorded as one `plan.switched` event (durability
//! FULL), the trace carries `result.plan_switch` with the re-prefill cost
//! priced at the destination account's miss price, and `failover_from` names
//! the route the failure-class fact moved the request off.
//!
//! The 403 fixture's wording contains `insufficient_quota` — one of the
//! classifier's QUOTA_EXHAUSTED_PATTERNS — and carries `Retry-After: 1` so
//! the ADR-011 provider demotion that follows the 403 lasts one second
//! instead of the 60s default (this case needs no second request on the
//! primary, so the short demotion only keeps the rig fast).

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, PlanRig};

async fn rig(tag: &str) -> PlanRig {
    let (plan, api, dir, listen_addr) =
        testkit::plan_rig_parts(tag, "", testkit::PLAN_POLICY_DEFAULT).await;
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

/// Every stored event as (kind_raw, payload), read after the server stopped.
fn events(dir: &std::path::Path) -> Vec<(String, serde_json::Value)> {
    let store = vadis_store::SqliteStore::open(&dir.join("state/router.db")).unwrap();
    use vadis_core::store::{Query, QueryRow, Store as _};
    let QueryRow::Events(rows) = store.query(Query::AllEvents).unwrap() else {
        panic!("events query");
    };
    rows.into_iter().map(|r| (r.kind_raw, r.payload)).collect()
}

/// Every trace record, in write order.
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

/// Rule 1 (priority): the family's request goes to the primary route's own
/// `urls` endpoint — the plan mock receives it, the metered mock receives nothing
/// — and the primary's bytes are relayed verbatim.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_32_primary_is_preferred_while_healthy() {
    let rig = rig("conf32-priority").await;
    rig.plan.queue(testkit::plan_ok("from-plan"));

    let (status, body, _h) = rig.post(None, 1);

    assert_eq!(status, 200, "the plan account answered");
    assert_eq!(
        String::from_utf8_lossy(&body),
        String::from_utf8_lossy(testkit_ok_body("from-plan").as_bytes()),
        "relayed verbatim"
    );
    assert_eq!(rig.plan.requests().len(), 1, "the primary was attempted");
    assert_eq!(
        rig.api.requests().len(),
        0,
        "the metered account is untouched"
    );
    let seen = &rig.plan.requests()[0];
    assert_eq!(seen.method, "POST");
    assert!(
        seen.path.starts_with("/v1/"),
        "the request went to the primary's own URL, got path {}",
        seen.path
    );

    rig.stop();
}

fn testkit_ok_body(content: &str) -> String {
    format!(
        r#"{{"id":"ok","choices":[{{"index":0,"message":{{"role":"assistant","content":"{content}"}},"finish_reason":"stop"}}],"usage":{{"prompt_tokens":100,"completion_tokens":5,"total_tokens":105,"prompt_tokens_details":{{"cached_tokens":20}}}}}}"#
    )
}

/// Rule 2 (overflow): a primary `403 quota_exhausted` spills the request to
/// the overflow route; `plan.switched` is recorded once (durability FULL)
/// with the priced re-prefill; the trace shows the displacement
/// (`plan_switch` + `failover_from`) and the metered cost.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_32_quota_exhausted_spills_records_switch_and_prices_reprefill() {
    let rig = rig("conf32-spill").await;
    // Turn 1 succeeds on the plan (the ledger learns 100 prefix tokens);
    // turn 2's 403 is what prices the switch.
    rig.plan.queue(testkit::plan_ok("from-plan"));
    rig.plan.queue(testkit::plan_forbidden_403());
    rig.api.queue(testkit::plan_ok("from-api"));

    let (s1, _b1, _h1) = rig.post(Some("S1"), 1);
    assert_eq!(s1, 200);
    let (s2, body2, headers2) = rig.post(Some("S1"), 2);
    assert_eq!(s2, 200, "the overflow account answered the spilled request");
    assert_eq!(
        String::from_utf8_lossy(&body2),
        String::from_utf8_lossy(testkit_ok_body("from-api").as_bytes()),
        "the metered bytes relayed verbatim"
    );
    assert_eq!(
        rig.plan.requests().len(),
        2,
        "one attempt per turn on the primary"
    );
    assert_eq!(rig.api.requests().len(), 1, "one overflow attempt");
    let ff = headers2
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("x-router-failover-from"))
        .map(|(_, v)| v.as_str());
    assert_eq!(ff, Some("p-plan/m1"), "the failure-class fact is recorded");

    let dir = rig.stop();

    // One transition, one row: family, direction, reason, priced re-prefill.
    let evs = events(&dir);
    let switches: Vec<&(String, serde_json::Value)> =
        evs.iter().filter(|(k, _)| k == "plan.switched").collect();
    assert_eq!(
        switches.len(),
        1,
        "exactly one plan.switched per transition"
    );
    let (_k, p) = switches[0];
    assert_eq!(p["family"], "m1");
    assert_eq!(p["from_account"], "primary");
    assert_eq!(p["to_account"], "overflow");
    assert_eq!(p["reason"], "primary_exhausted");
    assert_eq!(p["probe"], false);
    assert_eq!(p["reprefill_tokens"], 100, "the session's prefix tokens");
    assert_eq!(
        p["switch_cost_nano"], 200_000,
        "100 tokens x p-api input_miss 0.002/1K = 0.0002 USD"
    );
    // Durability FULL (ADR-014 item 8): the wire word maps to the FULL tier.
    let kind = vadis_core::store::EventKind::from_str_lossy("plan.switched").unwrap();
    assert!(kind.is_full(), "plan.switched commits at durability FULL");

    // The trace: the spilled request's own record carries both markers and
    // the metered cost (the in-plan marginal price is 0, the spill is real).
    let rec = trace_records(&dir)
        .into_iter()
        .find(|r| r["identity"]["session"] == "S1" && r["identity"]["turn_index"] == 2)
        .expect("turn 2 record");
    assert_eq!(rec["decision"]["provider"], "p-api");
    assert_eq!(rec["decision"]["requested_model"], "p-plan/m1");
    assert_eq!(rec["result"]["failover_from"], "p-plan/m1");
    let ps = &rec["result"]["plan_switch"];
    assert_eq!(ps["from"], "p-plan/m1");
    assert_eq!(ps["to"], "p-api/m1");
    assert_eq!(ps["reason"], "primary_exhausted");
    assert_eq!(ps["probe"], false);
    assert_eq!(ps["reprefill_tokens"], 100);
    assert_eq!(ps["switch_cost_nano"], 200_000);
    assert_eq!(
        rec["cost"]["total"], 184_000,
        "metered: 80 miss x 2000 + 20 hit x 200 + 5 out x 4000 nano"
    );
}
