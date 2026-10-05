//! CONF-93 (ADR-049 §4 — `plan_policies` is a list, and two families are
//! **independent**): **two families, two accounts, no cross-talk.**
//!
//! One config declares `plan_policies: [A, B]`. Family A's primary mock
//! answers `403` and A spills to A's own overflow mock, while B's requests
//! keep being served on **B's own primary** — B's plan mock keeps receiving,
//! B's overflow mock receives nothing. `/health`'s `plan` member is an
//! **array of two** objects, each carrying its own family/account/`since`;
//! the `plan.switched` rows carry the right `family`; and each family's
//! `plan_state` row is its own.
//!
//! Plus the two refusal arms, each naming its own key: two list entries
//! naming one tag (`plan_policies[i].family`), and `plan_policy` **and**
//! `plan_policies` both written (both keys — §4.14's ladder, one layer up).
//!
//! Red at the base: `plan_policies` is not a config key, so the config does
//! not load.

#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::net::TcpStream;

use vadis_conformance::testkit;

/// Four mocks: A-plan, A-metered, B-plan, B-metered — so every "who received
/// what" assertion is a relation over the rig's own construction.
struct Rig {
    a_plan: testkit::MockUpstream,
    a_api: testkit::MockUpstream,
    b_plan: testkit::MockUpstream,
    b_api: testkit::MockUpstream,
    listen_addr: String,
    dir: std::path::PathBuf,
}

async fn rig(tag: &str) -> Rig {
    let a_plan = testkit::MockUpstream::start().await.unwrap();
    let a_api = testkit::MockUpstream::start().await.unwrap();
    let b_plan = testkit::MockUpstream::start().await.unwrap();
    let b_api = testkit::MockUpstream::start().await.unwrap();
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let dir = testkit::tempdir(tag);

    for name in ["CONF93_PA", "CONF93_NA", "CONF93_PB", "CONF93_NB"] {
        std::env::set_var(name, "sk-fixture");
    }

    let entry = |name: &str, port: u16, account: &str, mid: &str, family: &str, price: &str| {
        format!(
            r#"  - name: {name}
    urls:
      chat: http://127.0.0.1:{port}/v1/chat/completions
    api_key_env: CONF93_{up}
    wire_api: chat
    supports: [chat]
    account: {account}
    models:
      - id: {mid}
        context: 128k
        family: {family}
        price:
          input_miss: {price}
          input_hit: 0.0001
          cache_write: 0.0
          output: 0.002
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "mock upstream (no price; test fixture)"
"#,
            up = name.to_uppercase()
        )
    };

    let config = format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
{ap}{aa}{bp}{ba}
aliases: {{}}
plugins: []
fallback: []

plan_policies:
  - family: fa
    primary: pa/ma
    overflow: na/ma
    on_primary_exhausted: spill
    recover: probe
    cooldown: 0s
  - family: fb
    primary: pb/mb
    overflow: nb/mb
    on_primary_exhausted: spill
    recover: probe
    cooldown: 0s
"#,
        ap = entry("pa", a_plan.addr.port(), "coding_plan", "ma", "fa", "0.001"),
        aa = entry("na", a_api.addr.port(), "api", "ma", "fa", "0.002"),
        bp = entry("pb", b_plan.addr.port(), "coding_plan", "mb", "fb", "0.001"),
        ba = entry("nb", b_api.addr.port(), "api", "mb", "fb", "0.002"),
    );
    std::fs::write(dir.join("config.yaml"), config).unwrap();
    Rig {
        a_plan,
        a_api,
        b_plan,
        b_api,
        listen_addr,
        dir,
    }
}

fn http_get(addr: &str, path: &str) -> serde_json::Value {
    let mut s = TcpStream::connect(addr).unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).unwrap();
    let mut buf = String::new();
    s.read_to_string(&mut buf).unwrap();
    let (_, body) = buf.split_once("\r\n\r\n").expect("response body");
    serde_json::from_str(body).expect("health json")
}

fn events(dir: &std::path::Path) -> Vec<(String, serde_json::Value)> {
    let store = vadis_store::SqliteStore::open(&dir.join("state/vadis.db")).unwrap();
    use vadis_core::store::{Query, QueryRow, Store as _};
    let QueryRow::Events(rows) = store.query(Query::AllEvents).unwrap() else {
        panic!("events query");
    };
    rows.into_iter().map(|r| (r.kind_raw, r.payload)).collect()
}

/// A's spill does not move B; the list's two families are independent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_93_two_families_spill_independently() {
    let r = rig("conf93-two").await;
    r.a_plan.queue(testkit::plan_forbidden_403());
    r.a_api.queue(testkit::plan_ok("from-a-api"));
    r.b_plan.queue(testkit::plan_ok("from-b-plan"));
    r.b_plan.queue(testkit::plan_ok("from-b-plan-2"));

    let cfg = r.dir.join("config.yaml").to_string_lossy().into_owned();
    let task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&r.listen_addr);

    let post = |model: &str, session: &str| {
        let body = format!(
            r#"{{"model":"{model}","messages":[{{"role":"user","content":"x"}}],"prompt_cache_key":"{session}","stream":false}}"#
        );
        testkit::http_post(&r.listen_addr, "/v1/chat/completions", body.as_bytes(), &[])
    };

    let (sa, _b, _h) = post("pa/ma", "SA");
    assert_eq!(sa, 200, "family A spilled and was served");
    let (sb, _b2, _h2) = post("pb/mb", "SB");
    assert_eq!(sb, 200, "family B was served on its own primary");

    assert_eq!(r.a_plan.requests().len(), 1, "A's primary, once");
    assert_eq!(r.a_api.requests().len(), 1, "A's spill");
    assert_eq!(r.b_plan.requests().len(), 1, "B stayed on its primary");
    assert_eq!(r.b_api.requests().len(), 0, "A's spill moved nothing in B");

    let h = http_get(&r.listen_addr, "/health");
    let fams = h["plan"]
        .as_array()
        .expect("plan is an array under plan_policies");
    assert_eq!(fams.len(), 2, "one entry per declared family");
    assert_eq!(fams[0]["family"], "fa");
    assert_eq!(fams[0]["account"], "overflow");
    assert_eq!(fams[1]["family"], "fb");
    assert_eq!(fams[1]["account"], "primary", "B never switched");

    task.abort();
    let _ = task.await;
    let switches: Vec<_> = events(&r.dir)
        .into_iter()
        .filter(|(k, _)| k == "plan.switched")
        .collect();
    assert_eq!(switches.len(), 1, "one transition, in family A");
    assert_eq!(switches[0].1["family"], "fa");
}

/// The refusal arms, each naming its own key: two list entries naming
/// one tag (`plan_policies[i].family`), and `plan_policy` AND
/// `plan_policies` both written (both keys). Both are asserted on the
/// loader's own reason (the same string `serve` prints before exiting
/// 2), so the message and the key path are both pinned verbatim.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_93_refusals_name_their_keys() {
    // (a) duplicate family: the same rig's config with both entries
    // naming `fa`.
    let r = rig("conf93-dup").await;
    let config = std::fs::read_to_string(r.dir.join("config.yaml")).unwrap();
    let dup = config.replace("  - family: fb", "  - family: fa");
    assert_ne!(dup, config, "the patch landed");
    std::fs::write(r.dir.join("config.yaml"), &dup).unwrap();
    let err = vadis_cli::config_load::load(&r.dir.join("config.yaml"))
        .expect_err("the duplicate-family list refuses at load");
    assert!(
        err.contains("plan_policies[1].family"),
        "the refusal names the second entry's family key, got: {err}"
    );
    assert!(
        err.contains("already carried by an earlier entry"),
        "the refusal states the two-writers reason, got: {err}"
    );

    // (b) both spellings written: the one-key policy is prepended to
    // the same config (the rig's own routes are valid, so the arm is
    // about the ladder alone — the ladder is checked before any
    // per-family rule).
    let r = rig("conf93-both").await;
    let config = std::fs::read_to_string(r.dir.join("config.yaml")).unwrap();
    let both =
        format!("plan_policy:\n  family: fa\n  primary: pa/ma\n  overflow: na/ma\n\n{config}");
    std::fs::write(r.dir.join("config.yaml"), &both).unwrap();
    let err = vadis_cli::config_load::load(&r.dir.join("config.yaml"))
        .expect_err("both spellings written refuse at load");
    assert!(
        err.contains("plan_policy") && err.contains("plan_policies"),
        "the refusal names both keys, got: {err}"
    );
    assert!(
        err.contains("both keys are written"),
        "the ladder's own wording, got: {err}"
    );
}
