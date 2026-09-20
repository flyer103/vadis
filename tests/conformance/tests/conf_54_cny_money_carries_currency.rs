//! CONF-54 (§12.8; spec §4.8/§6/§9.2 + ADR-018 §2/§5): **a request
//! served by a CNY entry is priced, recorded and reported in CNY —
//! and a mixed window reports each currency with no combined total.**
//!
//! Numbering note: this round's cases start at CONF-52 (46/47 are on
//! the R6 branch, 48–51 reserved for the parallel R8 cards; the
//! operator's allocation, recorded with this file in DESIGN §12.8).
//!
//! The rig: the CONF-41 plan-first shape with the overflow entry
//! (`p-api`) switched to `currency: CNY`. One session, two turns —
//! turn 1 in-plan (a CNY-priced zero in every bucket), turn 2 meeting
//! the 403 and spilling to the CNY metered route (a CNY-priced real
//! five-tier cost). Then, side by side, a plain USD request. Asserted:
//!
//! - (a) every trace record is v2 (`schema_version: 2`) and carries
//!   `cost.currency` equal to **its own** serving entry's currency —
//!   CNY for the family's records, USD for the plain one;
//! - (b) the `cost.computed` store row of the CNY-priced spill carries
//!   `"currency": "CNY"` (the payload states its unit; read on its
//!   own, per ADR-018 §2's store item);
//! - (c) `router stats` over the mixed window: the figure maps hold
//!   exactly {USD, CNY}; the USD line equals the USD record's own
//!   tiers; the CNY line equals the spill record's own tiers; and no
//!   `Nano` sum of any single map key reproduces the records' joint
//!   total except **per currency** — asserted by checking the two
//!   currencies' figures are disjoint (a CNY tier never entered the
//!   USD line and vice versa);
//! - (d) `/health`'s provider list carries each entry's `region` and
//!   `currency` — the CNY entry says so, the default entries say
//!   `intl`/`USD` without the keys being written.

#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::net::TcpStream;

use router_conformance::testkit::{self, MockUpstream, PlanRig};

const POLICY_1H: &str = "  family: m1\n  primary: p-plan/m1\n  overflow: p-api/m1\n  on_primary_exhausted: spill\n  recover: probe\n  cooldown: 1h";

/// The plan rig's config with `p-api` switched to CNY, and a plain USD
/// provider appended (its own mock, its own route).
async fn mixed_rig(tag: &str) -> (PlanRig, MockUpstream) {
    let (plan, api, dir, listen_addr) = testkit::plan_rig_parts(tag, "", POLICY_1H).await;
    let usd = MockUpstream::start().await.unwrap();
    // Rewrite the written config: p-api becomes CNY, usd-provider is
    // appended with its own mock's port.
    let cfg_path = dir.join("config.yaml");
    let text = std::fs::read_to_string(&cfg_path).unwrap();
    let patched = text
        .replace("  - name: p-api\n", "  - name: p-api\n    currency: CNY\n")
        .replace(
            "aliases: {}",
            &format!(
                r#"  - name: usd-provider
    base_url: http://127.0.0.1:{port}/v1
    api_key_env: CONF54_USD_KEY
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
    std::env::set_var("CONF54_USD_KEY", "sk-usd");
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

fn http_post_json(addr: &str, path: &str, body: &str) -> (u16, Vec<u8>) {
    let mut stream = TcpStream::connect(addr).expect("connect");
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    let text = String::from_utf8_lossy(&buf).into_owned();
    let status: u16 = text.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = text
        .split("\r\n\r\n")
        .nth(1)
        .unwrap_or("")
        .as_bytes()
        .to_vec();
    (status, body)
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_54_cny_route_records_and_reports_in_cny() {
    let (rig, usd) = mixed_rig("conf54-cny").await;
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_forbidden_403());
    rig.api.queue(testkit::plan_ok("spilled"));
    usd.queue(testkit::plan_ok("plain-usd"));

    // Turn 1 in-plan (CNY zeros); turn 2 spills to the CNY meter route.
    let (s1, _b, _h) = rig.post(Some("S1"), 1);
    assert_eq!(s1, 200);
    let (s2, _b2, _h2) = rig.post(Some("S1"), 2);
    assert_eq!(s2, 200, "turn 2 spills to the CNY metered account");

    // A plain USD request on its own provider.
    let body =
        r#"{"model":"usd-provider/u1","messages":[{"role":"user","content":"hi"}],"stream":false}"#;
    let (s3, _b3) = http_post_json(&rig.listen_addr, "/v1/chat/completions", body);
    assert_eq!(s3, 200, "the plain USD request serves");

    // (d) /health carries region+currency per entry, defaults included.
    let h = http_get(&rig.listen_addr, "/health");
    let providers = h["providers"].as_array().expect("provider list");
    let by_name = |n: &str| {
        providers
            .iter()
            .find(|p| p["name"] == n)
            .unwrap_or_else(|| panic!("provider {n} on /health"))
    };
    assert_eq!(by_name("p-api")["currency"], "CNY", "the CNY entry says so");
    assert_eq!(by_name("p-api")["region"], "intl", "not written ⇒ intl");
    assert_eq!(by_name("p-plan")["currency"], "USD", "not written ⇒ USD");
    assert_eq!(by_name("usd-provider")["currency"], "USD");

    // (a) every record is v2 and states its own serving entry's unit.
    let dir = rig.stop();
    let recs = trace_records(&dir);
    assert!(recs.len() >= 3, "three requests, three records");
    assert!(
        recs.iter().all(|r| r["schema_version"] == 2),
        "all records are v2"
    );
    for r in &recs {
        let provider = r["decision"]["provider"].as_str().unwrap();
        // Each record states ITS OWN serving entry's unit: the spill's
        // p-api entry is CNY; p-plan and usd-provider are USD (no key).
        let expected = if provider == "p-api" { "CNY" } else { "USD" };
        assert_eq!(
            r["cost"]["currency"], expected,
            "record for {provider} carries its own entry's currency"
        );
    }
    // The spill's record: a real (non-zero) CNY total, priced by the
    // metered table — not a USD figure and not a converted one.
    let spill = recs
        .iter()
        .find(|r| r["decision"]["provider"] == "p-api")
        .expect("the spilled request's record");
    assert!(
        spill["cost"]["total"].as_u64().unwrap_or(0) > 0,
        "the metered spill is priced (CNY table, in-plan was the zero)"
    );
    assert_eq!(spill["result"]["plan_switch"]["cost_currency"], "CNY");

    // (b) the spill's cost.computed store row carries the unit.
    {
        use router_core::store::{Query, QueryRow, Store as _};
        let store = router_store::SqliteStore::open_read_only(&dir.join("state/router.db"))
            .expect("store opens read-only");
        let QueryRow::Events(events) = store.query(Query::AllEvents).unwrap() else {
            panic!("events");
        };
        let rows: Vec<_> = events
            .iter()
            .filter(|e| e.kind_raw == "cost.computed")
            .collect();
        assert!(!rows.is_empty(), "cost.computed rows exist");
        let spill_id = spill["identity"]["request_id"].as_str().unwrap();
        let spill_row = rows
            .iter()
            .find(|e| e.request_id.as_deref() == Some(spill_id))
            .expect("the spill's cost.computed row");
        assert_eq!(spill_row.payload["currency"], "CNY");
        assert_eq!(spill_row.schema_version, 2, "EVENT_SCHEMA_VERSION 2");
    }

    // (c) the mixed window reports per currency, no combined total.
    let rep = router_cli::stats::report(&dir.join("config.yaml").to_string_lossy(), "24h")
        .expect("report computes on a mixed window (exit 0, not an error)");
    let figs = &rep.figures;
    let curs: Vec<&str> = figs.currencies.keys().map(|s| s.as_str()).collect();
    assert_eq!(curs, vec!["CNY", "USD"], "both currencies, labelled");
    // The counts stay single (currency-free, §9.2).
    assert_eq!(figs.requests, 3);
    assert_eq!(figs.switches, 1);
    // The CNY line is exactly the spill record's own tiers (the in-plan
    // record contributed zeros); the USD line is the plain record's.
    let usd_rec = recs
        .iter()
        .find(|r| r["decision"]["provider"] == "usd-provider")
        .expect("the plain USD record");
    for (map, key) in [
        (&figs.input_miss_nano, "input_miss"),
        (&figs.input_hit_nano, "input_hit"),
        (&figs.cache_write_nano, "cache_write"),
        (&figs.output_nano, "output"),
    ] {
        assert_eq!(
            map.get("CNY").copied().unwrap_or(0),
            spill["cost"][key].as_u64().unwrap_or(0),
            "CNY {key} equals the spill record's own tier"
        );
        assert_eq!(
            map.get("USD").copied().unwrap_or(0),
            usd_rec["cost"][key].as_u64().unwrap_or(0),
            "USD {key} equals the plain record's own tier"
        );
    }
    // Disjointness, the essence of "never mixed": no record's tier
    // contributed to the other currency's line. The USD record's total
    // must not appear inside the CNY line (and vice versa) — the lines
    // are each a single record's tiers here, already asserted exactly;
    // this is the same fact read as a relation, not a snapshot.
    let usd_total = usd_rec["cost"]["total"].as_u64().unwrap_or(0);
    let cny_sum: u64 = figs.input_miss_nano.get("CNY").copied().unwrap_or(0)
        + figs.input_hit_nano.get("CNY").copied().unwrap_or(0)
        + figs.cache_write_nano.get("CNY").copied().unwrap_or(0)
        + figs.output_nano.get("CNY").copied().unwrap_or(0);
    assert_ne!(
        cny_sum, usd_total,
        "the currencies' figures are never folded into one number"
    );
}
