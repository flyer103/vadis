//! CONF-91 (ADR-049 §3 rules 3/4/6 — the credential pool rotates **within**
//! a provider, and `quota_exhausted` does not rotate at all): **the two axes
//! a naive pool confuses, pinned apart.**
//!
//! Limb (a) — rotation. A provider entry carries `api_keys: [K1, K2]` and the
//! mock answers `401` to the first key: the request is served on the second,
//! **each key is attempted once** (the mock's own request log is the evidence:
//! two requests carrying distinct `Authorization` values, in pool order),
//! the `error.classified` row names the action `rotate_credential` with the
//! **index** (never the value), and `failover_from` stays `null` — the walk
//! never left the provider.
//!
//! Limb (b) — the negative that keeps the two axes apart. The same rig, the
//! primary answering `403` with the quota wording: the **account** moves
//! (§4.6 — one `plan.switched`, reason `primary_exhausted`) and **no**
//! rotation happens; a plan with two keys is still one exhausted plan
//! (ADR-011:181-182; ADR-014's Background). The mock sees exactly one request
//! on the primary's pool from that request.
//!
//! Red at the base: `api_keys` is not a config key, so the config does not
//! even load — the case is parked `#[ignore]`d behind the pool's landing.

#![forbid(unsafe_code)]

use vadis_conformance::testkit;

/// The provider whose pool the case rotates over, plus a metered sibling for
/// limb (b) to spill onto. Keys: `p1` holds the pool; `p2` is the family's
/// `overflow`.
async fn rig(
    tag: &str,
    plan_policy: &str,
) -> (
    testkit::MockUpstream,
    testkit::MockUpstream,
    String,
    std::path::PathBuf,
) {
    let p1 = testkit::MockUpstream::start().await.unwrap();
    let p2 = testkit::MockUpstream::start().await.unwrap();
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let dir = testkit::tempdir(tag);

    std::env::set_var("CONF91_K1", "sk-first");
    std::env::set_var("CONF91_K2", "sk-second");
    std::env::set_var("CONF91_P2_KEY", "sk-metered");

    let config = format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: p1
    urls:
      chat: http://127.0.0.1:{p1_port}/v1/chat/completions
    api_keys: [CONF91_K1, CONF91_K2]
    wire_api: chat
    supports: [chat]
    account: coding_plan
    models:
      - id: m1
        context: 128k
        price:
          input_miss: 0.001
          input_hit: 0.0001
          cache_write: 0.0
          output: 0.002
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "mock upstream (no price; test fixture)"
  - name: p2
    urls:
      chat: http://127.0.0.1:{p2_port}/v1/chat/completions
    api_key_env: CONF91_P2_KEY
    wire_api: chat
    supports: [chat]
    account: api
    models:
      - id: m1
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

{plan_policy}
"#,
        p1_port = p1.addr.port(),
        p2_port = p2.addr.port(),
    );
    std::fs::write(dir.join("config.yaml"), config).unwrap();
    (p1, p2, listen_addr, dir)
}

const POLICY: &str = "plan_policy:\n  family: m1\n  primary: p1/m1\n  overflow: p2/m1\n  on_primary_exhausted: spill\n  recover: probe\n  cooldown: 0s\n";

async fn serve(dir: &std::path::Path, listen_addr: &str) -> tokio::task::JoinHandle<i32> {
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let addr = listen_addr.to_string();
    let task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&addr);
    task
}

fn events(dir: &std::path::Path) -> Vec<(String, serde_json::Value)> {
    let store = vadis_store::SqliteStore::open(&dir.join("state/vadis.db")).unwrap();
    use vadis_core::store::{Query, QueryRow, Store as _};
    let QueryRow::Events(rows) = store.query(Query::AllEvents).unwrap() else {
        panic!("events query");
    };
    rows.into_iter().map(|r| (r.kind_raw, r.payload)).collect()
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

async fn stop(task: tokio::task::JoinHandle<i32>, dir: std::path::PathBuf) -> std::path::PathBuf {
    task.abort();
    let _ = task.await;
    dir
}

fn body() -> String {
    r#"{"model":"p1/m1","messages":[{"role":"user","content":"conf91"}],"stream":false}"#
        .to_string()
}

/// (a) A `401` on the pool's first key advances to the second, within the
/// provider, and narrates the rotation by index.
#[ignore = "CONF-91: depends on the credential pool (api_keys) and its rotation arm"]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_91_pool_rotates_on_a_credential_failure() {
    let (p1, p2, listen_addr, dir) = rig("conf91-rotate", POLICY).await;
    p1.queue(testkit::CannedResponse::json(
        401,
        "Unauthorized",
        br#"{"error":{"message":"invalid api key"}}"#,
    ));
    p1.queue(testkit::plan_ok("served-by-k2"));
    let task = serve(&dir, &listen_addr).await;

    let (status, _b, _h) =
        testkit::http_post(&listen_addr, "/v1/chat/completions", body().as_bytes(), &[]);
    assert_eq!(status, 200, "the pool's second key served the request");
    assert_eq!(p2.requests().len(), 0, "the provider was not left");

    // Each key attempted once, in pool order, with distinct credentials.
    let seen = p1.requests();
    assert_eq!(seen.len(), 2, "one attempt per key");
    let a1 = seen[0]
        .header("authorization")
        .unwrap_or_default()
        .to_string();
    let a2 = seen[1]
        .header("authorization")
        .unwrap_or_default()
        .to_string();
    assert_ne!(
        a1, a2,
        "the second attempt presented a different credential"
    );
    assert!(
        a1.contains("sk-first") && a2.contains("sk-second"),
        "pool order"
    );

    let dir = stop(task, dir).await;
    let evs = events(&dir);
    let rotations: Vec<_> = evs
        .iter()
        .filter(|(k, p)| k == "error.classified" && p.to_string().contains("rotate_credential"))
        .collect();
    assert_eq!(rotations.len(), 1, "exactly one rotation row");
    // The index, never the value.
    assert!(
        !rotations[0].1.to_string().contains("sk-first")
            && !rotations[0].1.to_string().contains("sk-second"),
        "a credential value never enters the log"
    );
    let rec = trace_records(&dir)
        .into_iter()
        .find(|r| r["result"]["status"] == 200)
        .expect("the served record");
    assert_eq!(
        rec["result"]["failover_from"],
        serde_json::Value::Null,
        "a rotation is not a fallover: the provider was not left"
    );
    assert_eq!(rec["decision"]["key_index"], 1, "the serving key's index");
}

/// (b) The negative: a `403 quota_exhausted` moves the **account** and never
/// the key.
#[ignore = "CONF-91: depends on the credential pool (api_keys) and its rotation arm"]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_91_quota_exhausted_moves_the_account_not_the_key() {
    let (p1, p2, listen_addr, dir) = rig("conf91-quota", POLICY).await;
    p1.queue(testkit::plan_forbidden_403());
    p2.queue(testkit::plan_ok("spilled"));
    let task = serve(&dir, &listen_addr).await;

    let (status, _b, _h) =
        testkit::http_post(&listen_addr, "/v1/chat/completions", body().as_bytes(), &[]);
    assert_eq!(status, 200, "the metered account answered");
    assert_eq!(
        p1.requests().len(),
        1,
        "no second key was tried: an exhausted account is not an invalid credential"
    );
    assert_eq!(p2.requests().len(), 1, "the spill happened");

    let dir = stop(task, dir).await;
    let evs = events(&dir);
    assert_eq!(
        evs.iter().filter(|(k, _)| k == "plan.switched").count(),
        1,
        "the account moved, once"
    );
    assert_eq!(
        evs.iter()
            .filter(|(k, p)| k == "error.classified" && p.to_string().contains("rotate_credential"))
            .count(),
        0,
        "quota exhaustion is a fact about the account, not the credential"
    );
}
