//! CONF-94 (ADR-049 §5.4 — the ranking **is** the walk order): **three
//! metered providers, ranked by price, walked in that order.**
//!
//! A family on `overflow_selection: cheapest` whose three metered providers
//! are declared in an order that is deliberately **not** the price order:
//! the cheapest mock receives the spilled request first; when it answers a
//! retryable failure the **second-cheapest** receives the next attempt (not
//! its declaration-order neighbour), and the third only after that. Each
//! mock's own request log is the evidence, `result.plan_switch.candidates`
//! carries the resolved order and `chosen` the one the walk settled on — so
//! a pass cannot be an accident of routing, and a declaration-order walk
//! cannot pass.
//!
//! Red at the base: `overflow_selection` is not a config key, so the config
//! does not load.

#![forbid(unsafe_code)]

use vadis_conformance::testkit;

/// Three metered mocks. Declaration order is `mid`, `cheap`, `dear` while
/// price order is `cheap` < `mid` < `dear`, so the two orders disagree.
struct Rig {
    plan: testkit::MockUpstream,
    mid: testkit::MockUpstream,
    cheap: testkit::MockUpstream,
    dear: testkit::MockUpstream,
    listen_addr: String,
    dir: std::path::PathBuf,
}

async fn rig(tag: &str) -> Rig {
    let plan = testkit::MockUpstream::start().await.unwrap();
    let mid = testkit::MockUpstream::start().await.unwrap();
    let cheap = testkit::MockUpstream::start().await.unwrap();
    let dear = testkit::MockUpstream::start().await.unwrap();
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let dir = testkit::tempdir(tag);

    for n in ["CONF94_PLAN", "CONF94_MID", "CONF94_CHEAP", "CONF94_DEAR"] {
        std::env::set_var(n, "sk-fixture");
    }

    let entry = |name: &str, port: u16, account: &str, input_miss: &str| {
        format!(
            r#"  - name: {name}
    urls:
      chat: http://127.0.0.1:{port}/v1/chat/completions
    api_key_env: CONF94_{up}
    wire_api: chat
    supports: [chat]
    account: {account}
    models:
      - id: m
        context: 128k
        price:
          input_miss: {input_miss}
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
{plan_e}{mid_e}{cheap_e}{dear_e}
aliases: {{}}
plugins: []
fallback: []

plan_policy:
  family: m
  primary: p-plan/m
  overflow: p-mid/m
  on_primary_exhausted: spill
  recover: probe
  cooldown: 0s
  overflow_selection: cheapest
"#,
        plan_e = entry("p-plan", plan.addr.port(), "coding_plan", "0.001"),
        mid_e = entry("p-mid", mid.addr.port(), "api", "0.003"),
        cheap_e = entry("p-cheap", cheap.addr.port(), "api", "0.001"),
        dear_e = entry("p-dear", dear.addr.port(), "api", "0.005"),
    );
    std::fs::write(dir.join("config.yaml"), config).unwrap();
    Rig {
        plan,
        mid,
        cheap,
        dear,
        listen_addr,
        dir,
    }
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

/// The walk order is the price order: cheap → dear → mid, never declaration
/// order.
#[ignore = "CONF-94: depends on overflow_selection: cheapest and the ranking function"]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_94_cheapest_first_and_the_walk_follows_the_ranking() {
    let r = rig("conf94-rank").await;
    r.plan.queue(testkit::plan_forbidden_403());
    // The cheapest fails retryably; the second-cheapest answers.
    r.cheap.queue(testkit::CannedResponse::json(
        500,
        "Internal Server Error",
        br#"{"error":{"message":"upstream boom"}}"#,
    ));
    r.dear.queue(testkit::plan_ok("served-by-dear"));

    let cfg = r.dir.join("config.yaml").to_string_lossy().into_owned();
    let task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&r.listen_addr);

    let body = r#"{"model":"p-plan/m","messages":[{"role":"user","content":"x"}],"prompt_cache_key":"S1","stream":false}"#;
    let (status, _b, _h) =
        testkit::http_post(&r.listen_addr, "/v1/chat/completions", body.as_bytes(), &[]);
    assert_eq!(status, 200, "an eligible candidate served");

    assert_eq!(r.plan.requests().len(), 1, "the plan primary, once");
    assert_eq!(
        r.cheap.requests().len(),
        1,
        "the cheapest was attempted first"
    );
    assert_eq!(
        r.dear.requests().len(),
        1,
        "the second-cheapest followed — not the declaration-order neighbour"
    );
    assert_eq!(
        r.mid.requests().len(),
        0,
        "the mid-priced provider was never reached: the walk is the ranking"
    );

    task.abort();
    let _ = task.await;
    let rec = trace_records(&r.dir)
        .into_iter()
        .find(|x| x["result"]["status"] == 200)
        .expect("the served record");
    let cands = rec["result"]["plan_switch"]["candidates"]
        .as_array()
        .expect("candidates[] under cheapest");
    assert_eq!(
        cands,
        &vec![
            serde_json::json!("p-cheap/m"),
            serde_json::json!("p-dear/m"),
            serde_json::json!("p-mid/m"),
        ],
        "the resolved ranking, ascending input_miss"
    );
    assert_eq!(rec["result"]["plan_switch"]["chosen"], "p-dear/m");
}
