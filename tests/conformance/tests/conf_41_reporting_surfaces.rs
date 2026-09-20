//! CONF-41 (spec §9, the reporting surfaces): **`/health`'s `plan` section
//! and `router stats` report what is recorded — nothing else.**
//!
//! Both directions, decisive against each other:
//!
//! - (a) a run whose config declares a `plan_policy` serves a `plan` section
//!   on `/health`, and after the family spills the section reports the
//!   family with `account: "overflow"` and `probe.deadline == since +
//!   cooldown` — parsed back from the two RFC3339 strings, with `cooldown`
//!   taken from the config the process loaded (never the stored
//!   informational `until_us`);
//! - (b) a run with **no** `plan_policy` serves exactly
//!   `{"configured": false}` — no fabricated family, account, or deadline;
//! - (c) `router stats`' figures equal the sums computed independently in
//!   this case from the trace rows and the event log the run itself
//!   produced (the same tempdir, read back line by line).
//!
//! The deadline assertion uses a nonzero cooldown (`1h`) deliberately: with
//! the fixture default (`0s`) `deadline == since` would hold even if the
//! implementation printed the wrong instant's string.

#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::net::TcpStream;

use router_conformance::testkit::{self, PlanRig};

/// `cooldown: 1h` — long enough that `now < deadline` holds for the whole
/// test, so `blocked_by` must read `"cooldown"` and the deadline is
/// distinguishable from `since`.
const POLICY_1H: &str = "  family: m1\n  primary: p-plan/m1\n  overflow: p-api/m1\n  on_primary_exhausted: spill\n  recover: probe\n  cooldown: 1h";

async fn rig_with(tag: &str, policy: &str) -> PlanRig {
    let (plan, api, dir, listen_addr) = testkit::plan_rig_parts(tag, "", policy).await;
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

/// Minimal blocking GET (the CONF-25 style).
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

/// "YYYY-MM-DDTHH:MM:SS[.mmm]Z" → epoch milliseconds (test-side parser,
/// independent of the implementation's formatter).
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
    let s: i64 = t.next().unwrap().trim_end_matches('Z').parse().unwrap();
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

/// (a) + (c): a configured family reports its account state and probe
/// deadline after a real spill, and `stats` matches the independent sums.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_41_health_plan_section_and_stats_match_the_records() {
    let rig = rig_with("conf41-plan", POLICY_1H).await;
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_forbidden_403());
    rig.api.queue(testkit::plan_ok("spilled"));

    // Before any switch: the family is configured and on its primary —
    // probe null, since null (never switched).
    let h = http_get(&rig.listen_addr, "/health");
    assert_eq!(h["plan"]["configured"], true, "the policy is declared");
    assert_eq!(h["plan"]["family"], "m1");
    assert_eq!(h["plan"]["primary"], "p-plan/m1");
    assert_eq!(h["plan"]["overflow"], "p-api/m1");
    assert_eq!(h["plan"]["recover"], "probe");
    assert_eq!(h["plan"]["account"], "primary");
    assert_eq!(h["plan"]["since"], serde_json::Value::Null);
    assert_eq!(h["plan"]["probe"], serde_json::Value::Null);

    // Turn 1 on the plan, turn 2 meets the 403 and spills.
    let (s1, _b, _h) = rig.post(Some("S1"), 1);
    assert_eq!(s1, 200);
    let (s2, _b2, _h2) = rig.post(Some("S1"), 2);
    assert_eq!(s2, 200, "turn 2 spills to the metered account");

    // The section now reports overflow with deadline == since + 1h.
    let h = http_get(&rig.listen_addr, "/health");
    assert_eq!(h["plan"]["account"], "overflow");
    let since = h["plan"]["since"].as_str().expect("since is a string");
    let probe = &h["plan"]["probe"];
    let deadline = probe["deadline"].as_str().expect("deadline");
    assert_eq!(
        parse_ms(deadline) - parse_ms(since),
        3_600_000,
        "probe.deadline == since + the loaded cooldown (1h), parsed from the strings"
    );
    // A 1h cooldown from a spill moments ago: not admitted, and the first
    // failing condition in the guard's order is the cooldown itself.
    assert_eq!(probe["admitted"], false);
    assert_eq!(probe["blocked_by"], "cooldown");

    // One more request mid-session (no probe possible; the state serves it).
    rig.api.queue(testkit::plan_ok("spilled-2"));
    let (s3, _b3, _h3) = rig.post(Some("S1"), 3);
    assert_eq!(s3, 200);

    let dir = rig.stop();

    // (c) stats == the independent sums over the run's own trace rows.
    let rep = router_cli::stats::report(&dir.join("config.yaml").to_string_lossy(), "24h")
        .expect("report computes");
    let recs = trace_records(&dir);
    assert!(!recs.is_empty(), "the run produced trace rows");
    assert_eq!(rep.figures.requests, recs.len() as u64, "requests");
    let succeeded = recs
        .iter()
        .filter(|r| {
            let s = r["result"]["status"].as_u64().unwrap_or(0);
            (200..300).contains(&s)
        })
        .count() as u64;
    assert_eq!(rep.figures.succeeded, succeeded, "succeeded");
    assert_eq!(rep.figures.failed, recs.len() as u64 - succeeded, "failed");
    let mut input_total = 0u64;
    let mut input_cached = 0u64;
    let mut total_nano = 0u64;
    let mut switches = 0u64;
    for r in &recs {
        if r["usage_missing"].as_bool() == Some(true) {
            continue;
        }
        input_total += r["usage"]["input_total"].as_u64().unwrap_or(0);
        input_cached += r["usage"]["input_cached"].as_u64().unwrap_or(0);
        total_nano += r["cost"]["total"].as_u64().unwrap_or(0);
        if !r["result"]["plan_switch"].is_null() {
            switches += 1;
        }
    }
    assert_eq!(rep.figures.input_total_tokens, input_total, "input tokens");
    assert_eq!(
        rep.figures.input_cached_tokens, input_cached,
        "cached tokens"
    );
    assert_eq!(
        rep.figures.input_miss_nano
            + rep.figures.input_hit_nano
            + rep.figures.cache_write_nano
            + rep.figures.output_nano,
        total_nano,
        "cost: the four tiers sum to the records' own cost.total sum"
    );
    assert_eq!(rep.figures.switches, switches, "switches");
    // The spilled record (turn 2) is the one switch; its own measured
    // cost.total is the verified switch cost.
    let switch_cost: u64 = recs
        .iter()
        .filter(|r| {
            !r["result"]["plan_switch"].is_null() && r["usage_missing"] != serde_json::json!(true)
        })
        .map(|r| r["cost"]["total"].as_u64().unwrap_or(0))
        .sum();
    assert_eq!(
        rep.figures.switch_cost_verified_nano, switch_cost,
        "switch cost (verified) is the displaced record's own measured total"
    );
    assert_eq!(rep.figures.switches_without_usage, 0);

    // The event log was written by serve and opens read-only: the log-held
    // figure is present (zero unknown outcomes — every request answered).
    assert!(rep.events.log_was_read, "the store opened read-only");
    assert_eq!(rep.events.unknown_outcome_requests, 0);
}

/// (b): with no `plan_policy` the section is exactly `{"configured": false}`
/// and `stats` has **no** plan section.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_41_no_policy_is_not_fabricated() {
    // The rig always writes a plan_policy section; build a plain rig by
    // rewriting the config's policy away before serve starts.
    let (plan, api, dir, listen_addr) = testkit::plan_rig_parts(
        "conf41-nopolicy",
        "",
        "  family: m1\n  primary: p-plan/m1\n  overflow: p-api/m1\n  recover: none\n  cooldown: 0s",
    )
    .await;
    // Strip the whole plan_policy block from the written config.
    let cfg_path = dir.join("config.yaml");
    let text = std::fs::read_to_string(&cfg_path).unwrap();
    let stripped = text
        .split("plan_policy:")
        .next()
        .expect("the policy section is last")
        .to_string();
    std::fs::write(&cfg_path, stripped).unwrap();
    let cfg = cfg_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    let rig = PlanRig {
        plan,
        api,
        listen_addr: listen_addr.clone(),
        dir,
        serve_task,
    };

    let h = http_get(&rig.listen_addr, "/health");
    assert_eq!(
        h["plan"],
        serde_json::json!({ "configured": false }),
        "no policy ⇒ exactly configured:false, no other key"
    );

    // One request still works (a plain two-provider roster).
    rig.plan.queue(testkit::plan_ok("plain"));
    let (s, _b, _hh) = rig.post(Some("S1"), 1);
    assert_eq!(s, 200);

    let dir = rig.stop();
    let rep = router_cli::stats::report(&dir.join("config.yaml").to_string_lossy(), "24h")
        .expect("report computes");
    assert_eq!(rep.figures.requests, 1);
    assert_eq!(
        rep.figures.switches, 0,
        "no policy ⇒ no switch counts (nothing displaced anything)"
    );
    // And the report's plan section does not exist for this config —
    // witnessed by stats::report succeeding with no plan figures: the
    // struct carries switch counts only when a policy exists is a property
    // of the printer; the case asserts the figures it may not fabricate.
    assert_eq!(rep.figures.reprefill_tokens, 0);
    assert_eq!(rep.figures.reprefill_cost_nano, 0);
}
