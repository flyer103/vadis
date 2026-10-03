//! CONF-31 (DESIGN §10 conformance · §6 prefix metric): **prefix blocks
//! are enumerated in the provider's effective prompt (template) order** —
//! `system` position → `tools` → `messages`/`input` items — not body byte
//! order.
//!
//! Why this exists (the 2026-09-20 Plan A user decision, AGENTS constraint
//! 9 / ADR-012): on the real codex + deepseek pair the client body
//! serializes `input` *before* `tools`, while the provider's effective
//! prompt places tools before the conversation. Under the old byte-order
//! enumeration the client's tail append (a pure tail append upstream —
//! verified hit rate 0.991 = 17536/17694) registered as a mid-sequence
//! insertion and `prefix.continuity` reported 0.250, a ~4× under-report.
//! The historical numbers (0.250 byte-order / 0.690 raw byte prefix /
//! 0.991 verified / 1.000 template-order recomputation) are preserved in
//! the harness guard that pinned the decision; the decision and its
//! evidence are recorded with the 2026-09-20 date in the loop's own
//! history, which does not ship with the product.
//!
//! Assertions, over the real `router_cli::serve` assembly on loopback
//! HTTP with a mock upstream speaking the responses wire shape:
//!
//! 1. the trace's `prefix.blocks[]` kinds run `tool…, input_item…` for a
//!    body that serializes `input` before `tools`;
//! 2. the codex shape (turn 2 appends items at the `input` tail, tools
//!    unchanged) measures `prefix.continuity == 1.0` — exactly the
//!    regression this order change prevents;
//! 3. the companion: mutating turn 1's **first input item** drops the
//!    ratio below 1.0 — the metric is bidirectionally movable, not a
//!    constant that would pass for any traffic.

#![forbid(unsafe_code)]

use router_conformance::testkit::{self, CannedResponse};

fn config_yaml(upstream_port: u16, listen_port: u16) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: mock
    urls:
      responses: http://127.0.0.1:{upstream_port}/v1/responses
    api_key_env: CONF31_MOCK_KEY
    wire_api: responses
    supports: [responses]
    models:
      - id: rsp
        context: 128k
        price:
          input_miss: 0.001
          input_hit: 0.0001
          cache_write: 0.0
          output: 0.002
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "mock upstream (no price; test fixture)"

aliases: {{}}
plugins: []
fallback: []
"#
    )
}

/// The codex body shape (from the captured traffic): the members serialize
/// as `model`, `instructions`, `input`, `tools` — `input` **before**
/// `tools` — plus the session's `prompt_cache_key`. Turn 2 appends items
/// at the `input` tail and changes nothing else.
fn codex_body(input_items: &str, session: &str) -> String {
    format!(
        r#"{{"model":"mock/rsp","instructions":"be terse","input":[{input_items}],"tools":[{{"type":"function","name":"shell"}},{{"type":"function","name":"read_file"}}],"store":false,"prompt_cache_key":"{session}","stream":false}}"#
    )
}

const TURN1_ITEMS: &str = r#"{"type":"message","role":"user","content":"question one"}"#;
/// Turn 2 = turn 1's item + the assistant answer + the new user message
/// (the stateless-client resend shape), appended at the `input` tail.
const TURN2_ITEMS: &str = concat!(
    r#"{"type":"message","role":"user","content":"question one"},"#,
    r#"{"type":"message","role":"assistant","content":"answer one"},"#,
    r#"{"type":"message","role":"user","content":"question two"}"#
);
/// The mutated variant of turn 2: the FIRST input item is rewritten (the
/// conversation's front moves), the tail is still a superset.
const MUTATED_FIRST_ITEM: &str = concat!(
    r#"{"type":"message","role":"user","content":"REWRITTEN FRONT"},"#,
    r#"{"type":"message","role":"assistant","content":"answer one"},"#,
    r#"{"type":"message","role":"user","content":"question two"}"#
);

fn canned(turn: usize) -> CannedResponse {
    let body = format!(
        r#"{{"id":"resp-{turn}","object":"response","usage":{{"input_tokens":1000,"input_tokens_details":{{"cached_tokens":900}},"output_tokens":50,"total_tokens":1050}}}}"#
    );
    CannedResponse::json(200, "OK", body.as_bytes())
}

/// Drives two sequential requests of one session through the real serve
/// assembly and returns the two trace records in order.
async fn run_two_turns(dir: &std::path::Path, first_body: String, second_body: String) {
    let upstream = testkit::MockUpstream::start().await.unwrap();
    upstream.queue(canned(1));
    upstream.queue(canned(2));

    let listen_port = testkit::free_port();
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(upstream.addr.port(), listen_port)).unwrap();

    std::env::set_var("CONF31_MOCK_KEY", "sk-conf31");
    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
    testkit::wait_listening(&format!("127.0.0.1:{listen_port}"));

    for body in [first_body, second_body] {
        let (status, resp, _headers) = testkit::http_post(
            &format!("127.0.0.1:{listen_port}"),
            "/v1/responses",
            body.as_bytes(),
            &[],
        );
        assert_eq!(status, 200, "router status; body: {:?}", resp);
    }

    serve_task.abort();
    let _ = serve_task.await;
}

fn read_trace(dir: &std::path::Path) -> Vec<serde_json::Value> {
    let trace_dir = dir.join("state/traces");
    let mut records = Vec::new();
    for entry in std::fs::read_dir(&trace_dir).expect("trace dir exists") {
        let entry = entry.unwrap();
        if entry.path().extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        for line in std::fs::read_to_string(entry.path()).unwrap().lines() {
            records.push(serde_json::from_str(line).expect("each line is one record"));
        }
    }
    records
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_31_template_order_enumeration() {
    let dir = testkit::tempdir("conf31a");
    run_two_turns(
        &dir,
        codex_body(TURN1_ITEMS, "conf31-sess"),
        codex_body(TURN2_ITEMS, "conf31-sess"),
    )
    .await;

    let records = read_trace(&dir);
    assert_eq!(records.len(), 2, "one record per request");

    // 1. The enumeration is the provider template order: for a body that
    //    serializes `input` before `tools`, the block kinds run tools
    //    first, then the input items.
    let turn1 = &records[0];
    let blocks1 = turn1["prefix"]["blocks"].as_array().expect("blocks");
    let kinds: Vec<&str> = blocks1
        .iter()
        .map(|b| b["kind"].as_str().expect("kind"))
        .collect();
    assert_eq!(
        kinds,
        vec!["tool", "tool", "input_item"],
        "blocks must be enumerated in template order (tools before input items)"
    );

    // 2. The codex shape: turn 2 appended at the input tail with tools
    //    unchanged — in template order a pure tail append, so continuity
    //    is exactly 1.0. This is the regression the Plan A order change
    //    prevents: byte-order enumeration reported 0.250 on the same
    //    shape while the upstream's verified hit rate was 0.991.
    let turn2 = &records[1];
    let blocks2 = turn2["prefix"]["blocks"].as_array().expect("blocks");
    let kinds2: Vec<&str> = blocks2
        .iter()
        .map(|b| b["kind"].as_str().expect("kind"))
        .collect();
    assert_eq!(
        kinds2,
        vec!["tool", "tool", "input_item", "input_item", "input_item"]
    );
    for (i, b) in blocks1.iter().enumerate() {
        assert_eq!(
            b["hash"], blocks2[i]["hash"],
            "block {i} hash must not move between turns"
        );
    }
    let continuity = turn2["prefix"]["continuity"].as_f64().expect("measured");
    assert_eq!(
        continuity, 1.0,
        "the codex tail-append shape must measure exactly 1.0 in template order"
    );

    // And the first request of the session has no previous request to
    // measure against: absent, not invented.
    assert!(turn1["prefix"]["continuity"].is_null());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_31_mutated_first_item_drops_below_one() {
    let dir = testkit::tempdir("conf31b");
    run_two_turns(
        &dir,
        codex_body(TURN1_ITEMS, "conf31-sess-b"),
        codex_body(MUTATED_FIRST_ITEM, "conf31-sess-b"),
    )
    .await;

    let records = read_trace(&dir);
    assert_eq!(records.len(), 2, "one record per request");
    let continuity = records[1]["prefix"]["continuity"]
        .as_f64()
        .expect("measured");
    assert!(
        continuity < 1.0,
        "rewriting the first input item must drop continuity below 1.0 (got {continuity}); \
         a constant 1.0 would mean the metric is pinned, not measuring"
    );
}
