//! CONF-97 (ADR-049 §5.1/§5.2 — the plan tier is **drained** before any metered
//! spend, and the state machine across it): **two plan accounts, one family,
//! drained in declaration order — then metered, then back.**
//!
//! Three phases, each asserted from the mocks' own request logs plus the
//! events (so no arm can pass vacuously):
//!
//! 1. **Drain A → B, staying in-plan.** The tier's head (`p-a`, the policy's
//!    `primary`) answers `403 quota_exhausted`. The walk moves to the **second
//!    plan** (`p-b`) — the family stays in-plan: `plan.switched.reason ==
//!    "plan_exhausted"`, `/health`'s `plan.account` is still `primary`,
//!    `plan.route` names `p-b`, and the **metered mock receives nothing**.
//!    (The discriminating assertion: an implementation that spills at the
//!    first 403 fails this.)
//! 2. **Exhaust B → metered.** With `p-a` retired (its demotion and the
//!    family's drained state), the next request is served by the family's
//!    **active route** `p-b` — not yanked back to `p-a` (§5.2's coherence
//!    rule) — and `p-b`'s 403 is what reaches the metered tier:
//!    `reason == "primary_exhausted"`, `plan.account == "overflow"`.
//! 3. **Recover → the tier's head.** After the cooldown and the demotions have
//!    passed, a **new** session's boundary probes the tier's **head**
//!    (`p-a`); its 200 records `primary_recovered` and the metered mock falls
//!    silent.
//!
//! Red at the base: the plan tier is not discovered (the chain is
//! `[primary, overflow, fallback]`), so phase 1's `plan_exhausted` walk does
//! not exist.

#![forbid(unsafe_code)]

use std::time::Duration;

use vadis_conformance::testkit;

struct Rig {
    a: testkit::MockUpstream,
    b: testkit::MockUpstream,
    api: testkit::MockUpstream,
    listen_addr: String,
    dir: std::path::PathBuf,
}

/// Two plan providers (both `coding_plan`, one family tag) and one metered
/// provider. Roster order makes `p-a` the tier's head and `p-b` its second
/// member; the policy's `primary` names `p-a`.
async fn rig(tag: &str) -> Rig {
    let a = testkit::MockUpstream::start().await.unwrap();
    let b = testkit::MockUpstream::start().await.unwrap();
    let api = testkit::MockUpstream::start().await.unwrap();
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let dir = testkit::tempdir(tag);

    for n in ["CONF97_A", "CONF97_B", "CONF97_API"] {
        std::env::set_var(n, "sk-fixture");
    }

    let plan_entry = |name: &str, port: u16, key: &str| {
        format!(
            r#"  - name: {name}
    urls:
      chat: http://127.0.0.1:{port}/v1/chat/completions
    api_key_env: {key}
    wire_api: chat
    supports: [chat]
    account: coding_plan
    models:
      - id: m
        context: 128k
        price:
          input_miss: 0.001
          input_hit: 0.0001
          cache_write: 0.0
          output: 0.002
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "mock upstream (no price; test fixture)"
"#
        )
    };

    let config = format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
{a}{b}  - name: p-api
    urls:
      chat: http://127.0.0.1:{api_port}/v1/chat/completions
    api_key_env: CONF97_API
    wire_api: chat
    supports: [chat]
    account: api
    models:
      - id: m
        context: 128k
        price:
          input_miss: 0.002
          input_hit: 0.0002
          cache_write: 0.0
          output: 0.004
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "mock upstream (no price; test fixture)"

aliases: {{}}
plugins: []
fallback: []

plan_policy:
  family: m
  primary: p-a/m
  overflow: p-api/m
  on_primary_exhausted: spill
  recover: probe
  cooldown: 0s
"#,
        a = plan_entry("p-a", a.addr.port(), "CONF97_A"),
        b = plan_entry("p-b", b.addr.port(), "CONF97_B"),
        api_port = api.addr.port(),
    );
    std::fs::write(dir.join("config.yaml"), config).unwrap();
    Rig {
        a,
        b,
        api,
        listen_addr,
        dir,
    }
}

fn post(addr: &str, session: &str) -> (u16, Vec<u8>, Vec<(String, String)>) {
    let body = format!(
        r#"{{"model":"p-a/m","messages":[{{"role":"user","content":"turn"}}],"prompt_cache_key":"{session}","stream":false}}"#
    );
    testkit::http_post(addr, "/v1/chat/completions", body.as_bytes(), &[])
}

fn http_get(addr: &str, path: &str) -> serde_json::Value {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).unwrap();
    let mut buf = String::new();
    s.read_to_string(&mut buf).unwrap();
    let (_, body) = buf.split_once("\r\n\r\n").expect("response body");
    serde_json::from_str(body).expect("health json")
}

fn events(dir: &std::path::Path) -> Vec<serde_json::Value> {
    let store = vadis_store::SqliteStore::open(&dir.join("state/vadis.db")).unwrap();
    use vadis_core::store::{Query, QueryRow, Store as _};
    let QueryRow::Events(rows) = store.query(Query::AllEvents).unwrap() else {
        panic!("events query");
    };
    rows.into_iter()
        .filter(|r| r.kind_raw == "plan.switched")
        .map(|r| r.payload)
        .collect()
}

/// The three phases, in one run: drain A→B, exhaust B→metered, probe back to A.
#[ignore = "CONF-97: depends on the plan tier (drain) and the generalized plan_state"]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_97_plan_tier_is_drained_before_any_metered_spend() {
    let r = rig("conf97-tier").await;
    // p-a: first request 403 (drained), then 200 (the recovery probe).
    r.a.queue(testkit::plan_forbidden_403());
    r.a.queue(testkit::plan_ok("a-recovered"));
    // p-b: serves while it lasts, then 403 (its own exhaustion).
    r.b.queue(testkit::plan_ok("from-b"));
    r.b.queue(testkit::plan_forbidden_403());
    // The metered tier: the last response repeats.
    r.api.queue(testkit::plan_ok("from-api"));

    let cfg = r.dir.join("config.yaml").to_string_lossy().into_owned();
    let task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&r.listen_addr);

    // ---- Phase 1: the tier's head 403s; the walk drains into the second plan.
    let (s1, _b1, _h1) = post(&r.listen_addr, "S1");
    assert_eq!(s1, 200, "the second plan member served the request");
    assert_eq!(r.a.requests().len(), 1, "the tier's head was attempted");
    assert_eq!(
        r.b.requests().len(),
        1,
        "the second plan member was attempted"
    );
    assert_eq!(
        r.api.requests().len(),
        0,
        "NOTHING was spent: the plan tier still had room"
    );
    let h = http_get(&r.listen_addr, "/health");
    assert_eq!(
        h["plan"]["account"], "primary",
        "the family stayed in-plan (a naive implementation spills here)"
    );
    assert_eq!(
        h["plan"]["route"], "p-b/m",
        "the active route is the drained-to plan"
    );

    // The demotion that follows the 403 must lapse before phase 2, so the
    // request is not refused by route availability.
    std::thread::sleep(Duration::from_millis(1_500));

    // ---- Phase 2: the active plan 403s; only now is anything spent.
    let (s2, _b2, _h2) = post(&r.listen_addr, "S2");
    assert_eq!(s2, 200, "the metered tier served the request");
    assert_eq!(
        r.b.requests().len(),
        2,
        "the family went back to its ACTIVE route, not to the retired head"
    );
    assert_eq!(r.api.requests().len(), 1, "the metered tier, once");
    let h = http_get(&r.listen_addr, "/health");
    assert_eq!(
        h["plan"]["account"], "overflow",
        "the plan tier is exhausted"
    );
    assert_eq!(h["plan"]["route"], "p-api/m");

    // ---- Phase 3: after the cooldown, a new session probes the tier's HEAD.
    std::thread::sleep(Duration::from_millis(1_500));
    let (s3, _b3, _h3) = post(&r.listen_addr, "S3");
    assert_eq!(s3, 200, "the probe was admitted and the head answered");
    assert_eq!(r.a.requests().len(), 2, "the probe reached the tier's head");
    assert_eq!(
        r.api.requests().len(),
        1,
        "the recovery stopped the spend: no further metered request"
    );
    let h = http_get(&r.listen_addr, "/health");
    assert_eq!(h["plan"]["account"], "primary", "the family recovered");
    assert_eq!(h["plan"]["route"], "p-a/m", "back on the tier's head");

    task.abort();
    let _ = task.await;

    // The three transitions, in order, with their own reasons.
    let sw = events(&r.dir);
    assert_eq!(sw.len(), 3, "one plan.switched per transition");
    assert_eq!(sw[0]["reason"], "plan_exhausted", "in-plan move");
    assert_eq!(sw[0]["from"], "p-a/m");
    assert_eq!(sw[0]["to"], "p-b/m");
    assert_eq!(sw[0]["to_account"], "primary", "still in-plan");
    assert_eq!(
        sw[1]["reason"], "primary_exhausted",
        "the plan tier was left"
    );
    assert_eq!(sw[1]["to_account"], "overflow");
    assert_eq!(sw[2]["reason"], "primary_recovered", "the way back");
    assert_eq!(sw[2]["from_account"], "overflow");
    assert_eq!(sw[2]["to"], "p-a/m");
}
