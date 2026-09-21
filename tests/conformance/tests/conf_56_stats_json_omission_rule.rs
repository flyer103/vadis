//! CONF-56 (§12.8; spec §9.2 + ADR-018 §6): **`router stats --json`
//! keeps its scalar keys with one currency present and omits them when
//! two are present** — the per-currency map is what remains, so a
//! consumer that assumes one total fails loudly instead of adding
//! silently.
//!
//! This is the report-provenance row of the renumbered ADR-018 set (see
//! §12.8's CONF-52…54 paragraph), the half CONF-54's figures-level
//! assertions do not cover: CONF-41/54 call `stats::report` and read
//! `TraceFigures` — the *printed* `--json` shape (the omission rule's
//! two forms) had no witness until this file.
//!
//! The rig is CONF-54's mixed window (the plan rig with `p-api`
//! switched to `currency: CNY`, plus a plain USD provider), driven to
//! the same three records: turn 1 in-plan (CNY zeros), turn 2 spilled to
//! the CNY metered route, plus one plain USD request. Then both halves:
//!
//! - (a) **single-currency window** (a fresh USD-only rig, one request):
//!   `report_json` keeps every scalar cost key, adds `"currency":
//!   "USD"`, and has **no** `by_currency` member; the plan section's
//!   `switch_cost_currency` names the currency;
//! - (b) **mixed window** (the CNY rig's own window): the scalar cost
//!   keys and `"currency"` are **absent**, the figures live under
//!   `cost.by_currency.<tier>` as per-currency maps, the plan section's
//!   scalar switch-cost keys are absent and its `*_by_currency` maps are
//!   present — while the currency-free figures (`requests`,
//!   `switches`, `hit rate`) keep their single, unchanged keys, and the
//!   report itself exits 0 (a mixed window is not an error).

#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::net::TcpStream;

use router_conformance::testkit::{self, MockUpstream, PlanRig};

const POLICY_1H: &str = "  family: m1\n  primary: p-plan/m1\n  overflow: p-api/m1\n  on_primary_exhausted: spill\n  recover: probe\n  cooldown: 1h";

/// CONF-54's mixed rig: the plan rig with `p-api` switched to CNY and a
/// plain USD provider appended (its own mock, its own route).
async fn mixed_rig(tag: &str) -> (PlanRig, MockUpstream) {
    let (plan, api, dir, listen_addr) = testkit::plan_rig_parts(tag, "", POLICY_1H).await;
    let usd = MockUpstream::start().await.unwrap();
    let cfg_path = dir.join("config.yaml");
    let text = std::fs::read_to_string(&cfg_path).unwrap();
    let patched = text
        .replace("  - name: p-api\n", "  - name: p-api\n    currency: CNY\n")
        .replace(
            "aliases: {}",
            &format!(
                r#"  - name: usd-provider
    urls:
      chat: http://127.0.0.1:{port}/v1/chat/completions
    api_key_env: CONF56_USD_KEY
    wire_api: chat
    supports: [chat]
    account: api
    models:
      - id: u1
        context: 128k
        price:
          input_miss: 0.001
          input_hit: 0.0001
          cache_write: 0.0
          output: 0.002
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "mock upstream (no price; test fixture)"

aliases: {{}}"#,
                port = usd.addr.port()
            ),
        );
    std::fs::write(&cfg_path, patched).unwrap();
    std::env::set_var("CONF56_USD_KEY", "sk-usd");
    let cfg = cfg_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    (
        PlanRig {
            plan,
            api,
            listen_addr,
            dir,
            serve_task,
        },
        usd,
    )
}

fn http_post_json(addr: &str, path: &str, body: &str) -> u16 {
    let mut stream = TcpStream::connect(addr).expect("connect");
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    String::from_utf8_lossy(&buf)
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap()
}

/// `router stats --json`'s document, built through the same public
/// surface the CLI's own print path calls (`stats` → `report` +
/// `report_json`).
fn stats_json(config: &std::path::Path) -> serde_json::Value {
    let cfg = config.to_string_lossy().into_owned();
    let rep = router_cli::stats::report(&cfg, "24h").expect("report computes");
    let rc = router_cli::config_load::load(config).expect("config reloads");
    router_cli::stats::report_json(&rc, "24h", &rep, &None)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_56_json_scalar_keys_single_currency_absent_when_mixed() {
    // (a) single-currency window: a plain USD rig, one request.
    let (rig, usd) = mixed_rig("conf56-single").await;
    usd.queue(testkit::plan_ok("plain-usd"));
    let body =
        r#"{"model":"usd-provider/u1","messages":[{"role":"user","content":"hi"}],"stream":false}"#;
    let s = http_post_json(&rig.listen_addr, "/v1/chat/completions", body);
    assert_eq!(s, 200, "the plain USD request serves");
    let dir = rig.stop();
    let v = stats_json(&dir.join("config.yaml"));

    let cost = &v["cost"];
    assert_eq!(cost["currency"], "USD", "the one currency is named");
    for key in [
        "input_miss_nano",
        "input_hit_nano",
        "cache_write_nano",
        "output_nano",
    ] {
        assert!(
            cost[key].is_u64(),
            "single-currency window keeps the scalar key {key}"
        );
    }
    assert!(
        cost.get("by_currency").is_none(),
        "no per-currency map with one currency present"
    );
    // The plan section obeys the same rule: this rig declares a policy,
    // and its scalar switch key stays with the currency named beside it.
    assert_eq!(v["plan_family"]["switch_cost_currency"], "USD");
    assert!(
        v["plan_family"]
            .get("switch_cost_verified_nano_by_currency")
            .is_none(),
        "no by-currency map in the plan section either"
    );
    // The single request is in-plan for the family: zero measured spend,
    // but the scalar key is present and typed — the shape is the point.
    assert!(v["plan_family"]["switch_cost_verified_nano"].is_u64());

    // (b) mixed window: the CNY rig's own window (in-plan CNY zeros +
    // the CNY spill + the plain USD record).
    let (rig2, usd2) = mixed_rig("conf56-mixed").await;
    rig2.plan.queue(testkit::plan_ok("t1"));
    rig2.plan.queue(testkit::plan_forbidden_403());
    rig2.api.queue(testkit::plan_ok("spilled"));
    usd2.queue(testkit::plan_ok("plain-usd"));
    let (s1, _b, _h) = rig2.post(Some("S1"), 1);
    assert_eq!(s1, 200);
    let (s2, _b2, _h2) = rig2.post(Some("S1"), 2);
    assert_eq!(s2, 200, "turn 2 spills to the CNY metered account");
    let body =
        r#"{"model":"usd-provider/u1","messages":[{"role":"user","content":"hi"}],"stream":false}"#;
    let s3 = http_post_json(&rig2.listen_addr, "/v1/chat/completions", body);
    assert_eq!(s3, 200);
    let dir2 = rig2.stop();
    let v2 = stats_json(&dir2.join("config.yaml"));

    // The report itself exits 0 — `report(...)` returning Ok is that
    // fact for the library path; a mixed window is not an error.
    let cost2 = &v2["cost"];
    assert!(
        cost2.get("currency").is_none(),
        "no single-currency string with two currencies present"
    );
    for key in [
        "input_miss_nano",
        "input_hit_nano",
        "cache_write_nano",
        "output_nano",
    ] {
        assert!(
            cost2.get(key).is_none(),
            "the scalar key {key} is absent in a mixed window (spec 9.2's omission rule)"
        );
    }
    let by = &cost2["by_currency"];
    assert!(by.is_object(), "the per-currency map is what remains");
    for key in [
        "input_miss_nano",
        "input_hit_nano",
        "cache_write_nano",
        "output_nano",
    ] {
        let m = &by[key];
        assert!(m.is_object(), "by_currency.{key} is a per-currency map");
        let curs: Vec<&str> = m.as_object().unwrap().keys().map(|s| s.as_str()).collect();
        assert_eq!(curs, vec!["CNY", "USD"], "both currencies, sorted");
    }
    // The plan section: scalars absent, per-currency maps present.
    let pf = &v2["plan_family"];
    assert!(pf.get("switch_cost_verified_nano").is_none());
    assert!(pf.get("switch_cost_currency").is_none());
    assert!(pf.get("reprefill_cost_nano_inferred").is_none());
    assert!(pf["switch_cost_verified_nano_by_currency"].is_object());
    assert!(pf["reprefill_cost_nano_inferred_by_currency"].is_object());
    // The currency-free figures keep their single, unchanged keys.
    assert_eq!(v2["requests"]["total"], 3, "counts stay single");
    assert_eq!(v2["plan_family"]["switches"], 1, "switches stay single");
    assert_eq!(
        v2["cache"]["hit_rate"]["input_total"], 300,
        "ratios stay single"
    );
    assert!(v2["cache"]["hit_rate"]["value"].is_number());
    // And no member anywhere in `cost` sums the currencies: the omission
    // is the whole rule — read as the absence asserted above, plus the
    // one relation that must hold: each tier's map has exactly the two
    // keys, never a folded entry.
    for key in [
        "input_miss_nano",
        "input_hit_nano",
        "cache_write_nano",
        "output_nano",
    ] {
        assert_eq!(
            cost2["by_currency"][key].as_object().unwrap().len(),
            2,
            "exactly the two currencies, no combined entry"
        );
    }
}
