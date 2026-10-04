//! CONF-88 (DESIGN §12.8, spec §4.17, ADR-042 §6/§12.2): **the exact-match
//! response cache is off by default** — an absent entry, and an entry whose
//! `config.enabled` is absent or `false`, change nothing on the default
//! path: a byte-identical repeat inside one session is still forwarded
//! upstream (two calls, two records, no `cache` group anywhere), and the
//! upstream-visible bodies are byte-identical to the same build's bodies
//! with the kind absent from the config.
//!
//! Limbs (ADR-042 §12.2):
//! - (a) **no entry ⇒ off** (green at the base by construction — the kind
//!   does not exist there, so nothing can be on);
//! - (b) **`enabled: false` ⇒ off**, and the default path is byte-identical
//!   to (a)'s (limb 4: the cache changed nothing on the default path);
//! - (c) **the key absent ⇒ off** — the limb the loader default pins: the
//!   red control is a sabotage build whose loader defaults
//!   `config.enabled` to `true`, which must turn exactly this arm red.
//!
//! No network egress: a loopback mock upstream only.

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, CannedResponse};
use serde_json::Value;

/// The canned 200 the upstream repeats: a chat completion with usage, so
/// both forwarded requests write ordinary (non-`usage_missing`) records.
const CANNED: &[u8] = br#"{"id":"ok","choices":[{"index":0,"message":{"role":"assistant","content":"off-by-default"},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":5,"total_tokens":105,"prompt_tokens_details":{"cached_tokens":20}}}"#;

/// One session's request, posted twice byte-identically.
const CLIENT_BODY: &[u8] = br#"{"model":"mock/glm","messages":[{"role":"user","content":"repeat me"}],"stream":false,"prompt_cache_key":"sess-88"}"#;

/// The upstream-visible body: the client's bytes with exactly the one
/// permitted mutation this fixture exercises — the top-level `model`
/// value replaced by the provider-native id (spec §2; CONF-27).
const EXPECTED_OUTBOUND: &[u8] = br#"{"model":"glm","messages":[{"role":"user","content":"repeat me"}],"stream":false,"prompt_cache_key":"sess-88"}"#;

fn config_yaml(upstream_port: u16, listen_port: u16, plugins_yaml: &str) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: mock
    urls:
      chat: http://127.0.0.1:{upstream_port}/v1/chat/completions
    api_key_env: CONF88_MOCK_KEY
    wire_api: chat
    supports: [chat]
    models:
      - id: glm
        context: 128k
        price:
          input_miss: 0.001
          input_hit: 0.0001
          cache_write: 0.0
          output: 0.002
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "mock upstream (no price; test fixture)"

aliases: {{}}
{plugins_yaml}
fallback: []
"#,
    )
}

fn read_records(trace_dir: &std::path::Path) -> Vec<Value> {
    let mut records = Vec::new();
    for entry in std::fs::read_dir(trace_dir).expect("trace dir exists") {
        let entry = entry.unwrap();
        if entry.path().extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        for line in std::fs::read_to_string(entry.path()).unwrap().lines() {
            records.push(serde_json::from_str(line).expect("record json"));
        }
    }
    records
}

/// One off-by-default arm: bring up `serve` with the given plugins
/// section, post the same bytes twice inside one session, and assert the
/// default path — two upstream calls, two ordinary records, no `cache`
/// group, and the upstream-visible bodies byte-identical to the expected
/// post-mutation bytes (which is what makes the arms byte-identical to
/// each other: they all assert against the same constant).
async fn run_arm(tag: &str, plugins_yaml: &str) {
    let dir = testkit::tempdir(tag);
    let upstream = testkit::MockUpstream::start().await.unwrap();
    upstream.queue(CannedResponse::json(200, "OK", CANNED));
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(
        &config_path,
        config_yaml(upstream.addr.port(), listen_port, plugins_yaml),
    )
    .unwrap();
    std::env::set_var("CONF88_MOCK_KEY", "sk-conf88");
    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);

    // A byte-identical repeat inside one session.
    let (s1, b1, _) = testkit::http_post(&listen_addr, "/v1/chat/completions", CLIENT_BODY, &[]);
    let (s2, b2, _) = testkit::http_post(&listen_addr, "/v1/chat/completions", CLIENT_BODY, &[]);
    assert_eq!((s1, s2), (200, 200), "both requests are served: {s1}/{s2}");
    assert_eq!(b1, CANNED, "the first response is the upstream's body");
    assert_eq!(b2, b1, "both responses carry the same bytes");

    // The default path: STILL forwarded upstream — two calls, and the
    // upstream-visible bodies are exactly the post-mutation bytes.
    let seen = upstream.requests();
    assert_eq!(
        seen.len(),
        2,
        "off by default: a byte-identical repeat is still forwarded upstream"
    );
    assert_eq!(
        seen[0].body, EXPECTED_OUTBOUND,
        "call 1's upstream-visible body"
    );
    assert_eq!(
        seen[1].body, EXPECTED_OUTBOUND,
        "call 2's upstream-visible body (byte-identical to call 1's — the default path \
         is byte-identical to a cache-less build)"
    );

    // Two records, no `cache` group anywhere, both ordinary (usage measured).
    serve_task.abort();
    let _ = serve_task.await;
    let records = read_records(&dir.join("state/traces"));
    assert_eq!(records.len(), 2, "one record per client request");
    for (i, rec) in records.iter().enumerate() {
        assert!(
            rec.get("cache").is_none(),
            "record {i} must not carry a cache group on the default path: {rec}"
        );
        assert_eq!(rec["usage_missing"], false, "record {i} measured usage");
        assert_eq!(rec["schema_version"], 2, "record {i} is a v2 record");
    }
}

/// Limb 1: the exemplar root mounts no cache entry at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_88a_no_entry_is_off() {
    run_arm("a-no-entry", "plugins: []").await;
}

/// Limb 2, first half: an entry with `config.enabled: false` is mounted
/// inert — the fiber may be loaded and the capability still off.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_88b_explicit_false_is_off() {
    run_arm(
        "b-explicit-false",
        "plugins:\n  - id: response-cache\n    kind: builtin/response_cache\n    config: { enabled: false }",
    )
    .await;
}

/// Limb 2, second half — the limb the loader's default pins: an entry
/// that omits the key is the same as `false`. **This is the sabotage
/// control's target**: a build whose loader defaults `config.enabled` to
/// `true` turns exactly this arm red (the repeat becomes a hit, the
/// upstream sees one call, and the hit's record grows a `cache` group).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_88c_absent_key_is_off() {
    run_arm(
        "c-absent-key",
        "plugins:\n  - id: response-cache\n    kind: builtin/response_cache\n    config: {}",
    )
    .await;
}
