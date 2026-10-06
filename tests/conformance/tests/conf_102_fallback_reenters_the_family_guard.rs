//! CONF-102 (ADR-051 §2.4, spec §4.2 / §4.6 / §6): **a `fallback` jump
//! re-enters the family guard** — a candidate that lands inside a family is
//! routed by that family's state, not by the jump that reached it.
//!
//! Rig: three providers and one family.
//!
//! * `outside` — `account: api`, `supports: [chat]`, **USD**. The request's
//!   own resolved route (`outside/m`), whose mock answers a retryable `500`.
//! * `zai-plan` — `account: coding_plan`, `supports: [chat]`, **USD**, its
//!   model entry tagged `fam`. The family's plan tier: mock answers `200`.
//! * `zai` — `account: api`, `supports: [chat]`, **CNY**, its model entry
//!   tagged `fam`. The family's metered tier **and** the request's `fallback`
//!   entry: its mock is the one that must stay silent.
//! * `plan_policy: { family: fam, primary: zai-plan/m, overflow: zai/m,
//!   on_primary_exhausted: spill, recover: probe, cooldown: 0s }` — the
//!   family's state is `primary` (no state row: the family has never spilled).
//! * `fallback: [zai/m]` — the jump that reaches the family from outside.
//!
//! Limb (a) — the assertion in the body. A chat request naming `outside/m`
//! fails over on the 500, and the walk's next candidate is `zai/m`, which
//! carries the family tag. The guard runs for **that** candidate (the
//! commitment point, not only the resolved route), the family's state says
//! `primary`, and the request is served by **`zai-plan`**: the plan mock
//! receives exactly one request, the metered mock receives **zero**, the
//! record's `result.plan_switch.reason == "primary_recovered"` (the direction
//! word the existing single owner `displacement_reason` picks for a move
//! *into* the plan tier), and `cost.currency == "USD"` — the settled route's
//! own unit, printed by the same route that produced `protocol.protocol_out`
//! (the currency is deliberately different between the two tiers, so the
//! field discriminates: a base-tree run that attempts the metered route
//! prints `CNY`).
//!
//! Limb (b) — the same rig with the family already on `overflow` — is stated
//! here for `R69-1`: drive the family's own spill first (a direct request for
//! `zai-plan/m` whose mock answers `403` with the quota wording, CONF-32's
//! rig), then repeat limb (a); the request must be served by the metered
//! route with **no** second `plan_switch`, because the family's state — not
//! the jump — decides.
//!
//! Red at the base: the candidate loop attempts `zai/m` with no guard call at
//! all (`forward.rs`'s walk; `family_policy_for_route` + `plan_guard` are
//! reached at `forward.rs:816`/`:834` for the resolved route only).
//!
//! Depends on: the guard being evaluated at every commitment point on both
//! forwarding paths, and the settled route being the single source of
//! `protocol.protocol_out` and `cost.currency`.

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, CannedResponse};

const CHAT_OK: &str = r#"{"id":"ok","choices":[{"index":0,"message":{"role":"assistant","content":"planned"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}"#;
const CLIENT_BODY: &str = r#"{"model":"outside/m","messages":[{"role":"user","content":"x"}],"prompt_cache_key":"s1","stream":false}"#;

struct Rig {
    plan: testkit::MockUpstream,
    metered: testkit::MockUpstream,
    outside: testkit::MockUpstream,
    listen_addr: String,
    dir: std::path::PathBuf,
}

async fn rig(tag: &str) -> Rig {
    let outside = testkit::MockUpstream::start().await.unwrap();
    let plan = testkit::MockUpstream::start().await.unwrap();
    let metered = testkit::MockUpstream::start().await.unwrap();
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let dir = testkit::tempdir(tag);

    std::env::set_var("CONF102_OUTSIDE_KEY", "sk-outside");
    std::env::set_var("CONF102_PLAN_KEY", "sk-plan");
    std::env::set_var("CONF102_METERED_KEY", "sk-metered");

    let config = format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: outside
    region: intl
    currency: USD
    urls:
      chat: http://127.0.0.1:{outside_port}/v1/chat/completions
    api_key_env: CONF102_OUTSIDE_KEY
    wire_api: chat
    supports: [chat]
    account: api
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
  - name: zai-plan
    region: intl
    currency: USD
    urls:
      chat: http://127.0.0.1:{plan_port}/v1/chat/completions
    api_key_env: CONF102_PLAN_KEY
    wire_api: chat
    supports: [chat]
    account: coding_plan
    models:
      - id: m
        family: fam
        context: 128k
        price:
          input_miss: 0.001
          input_hit: 0.0001
          cache_write: 0.0
          output: 0.002
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "mock upstream (no price; test fixture)"
  - name: zai
    region: cn
    currency: CNY
    urls:
      chat: http://127.0.0.1:{metered_port}/v1/chat/completions
    api_key_env: CONF102_METERED_KEY
    wire_api: chat
    supports: [chat]
    account: api
    models:
      - id: m
        family: fam
        context: 128k
        price:
          input_miss: 0.008
          input_hit: 0.002
          cache_write: 0.0
          output: 0.028
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "mock upstream (no price; test fixture)"

aliases: {{}}
plugins: []
fallback:
  - zai/m

plan_policy:
  family: fam
  primary: zai-plan/m
  overflow: zai/m
  on_primary_exhausted: spill
  recover: probe
  cooldown: 0s
"#,
        outside_port = outside.addr.port(),
        plan_port = plan.addr.port(),
        metered_port = metered.addr.port(),
    );
    std::fs::write(dir.join("config.yaml"), config).unwrap();
    Rig {
        plan,
        metered,
        outside,
        listen_addr,
        dir,
    }
}

async fn serve(dir: &std::path::Path, listen_addr: &str) -> tokio::task::JoinHandle<i32> {
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let addr = listen_addr.to_string();
    let task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&addr);
    task
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
#[ignore = "CONF-102: depends on R69-1"]
async fn conf_102_fallback_jump_reenters_the_family_guard() {
    let r = rig("conf102").await;
    r.outside.queue(CannedResponse::json(
        500,
        "Internal Server Error",
        br#"{"error":{"message":"boom"}}"#,
    ));
    r.plan
        .queue(CannedResponse::json(200, "OK", CHAT_OK.as_bytes()));
    let task = serve(&r.dir, &r.listen_addr).await;

    let (status, _b, _h) = testkit::http_post(
        &r.listen_addr,
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[],
    );
    assert_eq!(status, 200, "the request was served");
    assert_eq!(r.outside.requests().len(), 1, "the resolved route failed");
    assert_eq!(
        r.plan.requests().len(),
        1,
        "the family's plan tier served it, on the guard's answer"
    );
    assert_eq!(
        r.metered.requests().len(),
        0,
        "the metered tier was never reached: the jump re-entered the family"
    );

    task.abort();
    let records = trace_records(&r.dir);
    let served = records
        .iter()
        .find(|x| x["result"]["status"] == 200)
        .expect("the served request's record");
    assert_eq!(
        served["result"]["plan_switch"]["reason"], "primary_recovered",
        "the displacement's word is chosen by direction, by the existing owner"
    );
    assert_eq!(
        served["cost"]["currency"], "USD",
        "the settled route (the plan, USD) is the single source of the record's unit"
    );
    assert_eq!(
        served["protocol"]["protocol_out"], "chat",
        "and of the outbound wire"
    );
}
