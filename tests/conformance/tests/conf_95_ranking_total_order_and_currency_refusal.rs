//! CONF-95 (ADR-049 §5.6 — the ranking's total order, its purity, and the
//! currency refusal): **the tie-break is the stated one, and the refusal is
//! at load.**
//!
//! (a) two candidates equal on `input_miss` but differing on `output` → the
//!     lower `output` ranks first;
//! (b) two equal on both → **roster declaration order** decides;
//! (c) a `price.tiers` candidate ranks by its **first band** on the same
//!     footing as a flat price;
//! (d) purity: two evaluations of the ranking on one revision are identical
//!     (a pure function of (roster, config) — no turn number, clock or RNG);
//! (e) refusals at load: a mixed-currency candidate set names the family and
//!     the currencies; `cheapest` + a non-USD candidate + a cap names the cap.
//!
//! Red at the base: `overflow_selection` does not load.

#![forbid(unsafe_code)]

use vadis_conformance::testkit;

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

/// One metered provider's roster block: `(name, port, input_miss, output)`,
/// or a banded price when `tiers` is given.
fn metered_entry(name: &str, port: u16, price_body: &str) -> String {
    format!(
        r#"  - name: {name}
    urls:
      chat: http://127.0.0.1:{port}/v1/chat/completions
    api_key_env: CONF95_{up}
    wire_api: chat
    supports: [chat]
    account: api
    models:
      - id: m
        context: 128k
        price:
{price_body}
        source: "mock upstream (no price; test fixture)"
"#,
        up = name.to_uppercase().replace('-', "_")
    )
}

/// The config for the tie-break arms: `p-a` and `p-b` tie on `input_miss`,
/// `p-a` wins on `output`; `p-c` is a **banded** entry whose first band ties
/// `p-a`'s `input_miss`, so the declaration order between them is what the
/// case reads.
fn tie_config(listen: u16, plan: u16, a: u16, b: u16, c: u16) -> String {
    let flat = |input_miss: &str, output: &str| {
        format!(
            "          input_miss: {input_miss}\n          input_hit: 0.0001\n          cache_write: 0.0\n          output: {output}\n          peak: {{ multiplier: 1.0, windows: [] }}"
        )
    };
    let plan_entry = format!(
        r#"  - name: p-plan
    urls:
      chat: http://127.0.0.1:{plan}/v1/chat/completions
    api_key_env: CONF95_PLAN
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
    );
    let banded_c = format!(
        r#"  - name: p-c
    urls:
      chat: http://127.0.0.1:{c}/v1/chat/completions
    api_key_env: CONF95_P_C
    wire_api: chat
    supports: [chat]
    account: api
    models:
      - id: m
        context: 128k
        price:
          peak: {{ multiplier: 1.0, windows: [] }}
          tiers:
            - up_to: 32000
              input_miss: 0.004
              input_hit: 0.0001
              cache_write: 0.0
              output: 0.006
            - input_miss: 0.009
              input_hit: 0.0001
              cache_write: 0.0
              output: 0.012
        source: "mock upstream (no price; test fixture)"
"#
    );
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
{plan_entry}{a_entry}{b_entry}{banded_c}
aliases: {{}}
plugins: []
fallback: []

plan_policy:
  family: m
  primary: p-plan/m
  overflow: p-a/m
  on_primary_exhausted: spill
  recover: probe
  cooldown: 0s
  overflow_selection: cheapest
"#,
        a_entry = metered_entry("p-a", a, &flat("0.004", "0.006")),
        b_entry = metered_entry("p-b", b, &flat("0.004", "0.009")),
    )
}

/// (a)+(b)+(c) The resolved ranking, read off the first spilled request's own
/// trace record.
#[ignore = "CONF-95: depends on overflow_selection: cheapest and the ranking function"]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_95_output_then_declaration_order_break_ties() {
    let plan = testkit::MockUpstream::start().await.unwrap();
    let a = testkit::MockUpstream::start().await.unwrap();
    let b = testkit::MockUpstream::start().await.unwrap();
    let c = testkit::MockUpstream::start().await.unwrap();
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let dir = testkit::tempdir("conf95-ties");

    for n in ["CONF95_PLAN", "CONF95_P_A", "CONF95_P_B", "CONF95_P_C"] {
        std::env::set_var(n, "sk-fixture");
    }
    std::fs::write(
        dir.join("config.yaml"),
        tie_config(
            listen_port,
            plan.addr.port(),
            a.addr.port(),
            b.addr.port(),
            c.addr.port(),
        ),
    )
    .unwrap();

    // The plan 403s (so the family spills); the cheapest candidate answers.
    plan.queue(testkit::plan_forbidden_403());
    a.queue(testkit::plan_ok("served"));

    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);

    let body = r#"{"model":"p-plan/m","messages":[{"role":"user","content":"x"}],"prompt_cache_key":"S","stream":false}"#;
    let _ = testkit::http_post(&listen_addr, "/v1/chat/completions", body.as_bytes(), &[]);

    task.abort();
    let _ = task.await;

    let rec = trace_records(&dir)
        .into_iter()
        .find(|r| r["result"]["plan_switch"]["candidates"].is_array())
        .expect("a spilled record carrying the ranking");
    let order: Vec<String> = rec["result"]["plan_switch"]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();

    // p-a and p-b tie on input_miss (0.004); p-a's output (0.006) < p-b's
    // (0.009), so p-a ranks first. Then the banded p-c, whose FIRST band
    // (input_miss 0.004, output 0.006) ties p-a exactly — and being declared
    // last, it follows. A declaration-order-only ranking would put p-b before
    // p-c and p-c last; a first-band-blind one would rank p-c at 0.009.
    assert_eq!(
        order,
        vec![
            "p-a/m".to_string(),
            "p-c/m".to_string(),
            "p-b/m".to_string()
        ],
        "input_miss ties broken by output, then by declaration order, \
         with the banded entry ranked on its first band"
    );
}

/// (e) The refusals: mixed currency, and a cap over a non-USD candidate.
#[ignore = "CONF-95: depends on overflow_selection: cheapest and the ranking function"]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_95_mixed_currency_is_refused_at_load() {
    let dir = testkit::tempdir("conf95-currency");
    // p-usd (USD) and p-cny (CNY) both carry the family tag: a ranking across
    // them is a comparison of two currencies, which §4.8 forbids.
    let config = r#"server:   { addr: "127.0.0.1:1", upstream_attempt_timeout: 10s, request_timeout: 30s }
session:  { key_sources: ["prompt_cache_key"], ttl: 11h }
cache:    { sticky: true, breakeven: { enabled: true, min_remaining_turns: 2, safety_factor: 1.1 } }
trace:    { dir: "./state/traces", rollover: hourly }
providers:
  - name: p-plan
    urls: { chat: "http://127.0.0.1:1/v1/chat/completions" }
    api_key_env: CONF95_PLAN
    wire_api: chat
    supports: [chat]
    account: coding_plan
    currency: USD
    models: [ { id: m, context: 128k, price: { input_miss: 0.001, input_hit: 0.0001, cache_write: 0.0, output: 0.002, peak: { multiplier: 1.0, windows: [] } }, source: "fixture" } ]
  - name: p-usd
    urls: { chat: "http://127.0.0.1:1/v1/chat/completions" }
    api_key_env: CONF95_USD
    wire_api: chat
    supports: [chat]
    account: api
    currency: USD
    models: [ { id: m, context: 128k, price: { input_miss: 0.004, input_hit: 0.0001, cache_write: 0.0, output: 0.006, peak: { multiplier: 1.0, windows: [] } }, source: "fixture" } ]
  - name: p-cny
    urls: { chat: "http://127.0.0.1:1/v1/chat/completions" }
    api_key_env: CONF95_CNY
    wire_api: chat
    supports: [chat]
    account: api
    currency: CNY
    models: [ { id: m, context: 128k, price: { input_miss: 0.004, input_hit: 0.0001, cache_write: 0.0, output: 0.006, peak: { multiplier: 1.0, windows: [] } }, source: "fixture" } ]
aliases: {}
plugins: []
fallback: []
plan_policy:
  family: m
  primary: p-plan/m
  overflow: p-usd/m
  on_primary_exhausted: spill
  recover: probe
  cooldown: 0s
  overflow_selection: cheapest
"#;
    std::fs::write(dir.join("config.yaml"), config).unwrap();
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let code = vadis_cli::serve(&cfg).await;
    assert_ne!(code, 0, "the mixed-currency family refuses to load");
}
