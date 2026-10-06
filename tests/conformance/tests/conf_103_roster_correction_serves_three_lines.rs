//! CONF-103 (ADR-051 §2.5, spec §2 / §4 / §4.9): **the corrected roster
//! serves three lines, and the declaration stays consistent.**
//!
//! The three entries the round's roster correction touches, mirrored as a
//! fixture (one mock upstream per entry, three `urls` on that mock so the
//! **path** each request took is the evidence):
//!
//! * `zai-cn-plan` — `supports: [anthropic, chat, responses]`, its `wire_api`
//!   **flipped to `responses`** (the declarative change of §2.5.2);
//! * `kimi-cn-plan` — `supports: [chat, anthropic, responses]`, its
//!   `responses` cell **added** (`https://api.kimi.com/coding/v1/responses`
//!   in the shipped roster; a mock path here);
//! * `kimi-plan` — `supports: [anthropic, chat, responses]`, its `responses`
//!   cell **added** on the overseas base.
//!
//! The case drives all three inbound protocols against each entry and asserts
//! the mock's recorded `path` equals that cell's own URL — so "the cell is
//! served" is a statement about the request that left the process, not about
//! a config that loaded. Plus the declaration invariants that must survive
//! the edit and are load-time refusals when they do not: `wire_api ∈
//! supports` for every entry (`crates/vadis-core/src/config.rs:2091`),
//! `set(urls) == set(supports)` (§4.9), and no repeated model id within one
//! provider entry.
//!
//! Red at the base: every off-diagonal cell is answered `501 not_implemented`
//! (`forward.rs:902-911`, `stream_forward.rs:440-453`), so nine assertions
//! become three; driven instead over the **shipped pair** it is red one step
//! earlier — a declared cell with no `urls` row refuses to load (spec §4.9),
//! which is what makes the two added rows mandatory rather than cosmetic.
//!
//! Depends on: the declared-cell rule on both forwarding paths, and the
//! roster edit landing with the parser it needs.

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, CannedResponse};

const OK: &str = r#"{"id":"ok","choices":[{"index":0,"message":{"role":"assistant","content":"served"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}"#;

/// One entry under test: its name, its route id, its three cells (path on the
/// shared mock) and whether its `wire_api` is the diagonal it names.
struct Entry {
    name: &'static str,
    model: &'static str,
    key_env: &'static str,
    wire_api: &'static str,
    /// (inbound protocol, request path, the cell's own path on the mock)
    cells: [(&'static str, &'static str, &'static str); 3],
}

const ENTRIES: [Entry; 3] = [
    Entry {
        name: "zai-cn-plan",
        model: "glm-5.3",
        key_env: "CONF103_ZAI_CN_PLAN_KEY",
        wire_api: "responses",
        cells: [
            (
                "chat",
                "/v1/chat/completions",
                "/api/coding/paas/v4/chat/completions",
            ),
            ("responses", "/v1/responses", "/api/v1/responses"),
            ("anthropic", "/v1/messages", "/api/anthropic/v1/messages"),
        ],
    },
    Entry {
        name: "kimi-cn-plan",
        model: "k3",
        key_env: "CONF103_KIMI_CN_PLAN_KEY",
        wire_api: "chat",
        cells: [
            (
                "chat",
                "/v1/chat/completions",
                "/coding/v1/chat/completions",
            ),
            ("responses", "/v1/responses", "/coding/v1/responses"),
            ("anthropic", "/v1/messages", "/coding/v1/messages"),
        ],
    },
    Entry {
        name: "kimi-plan",
        model: "k3",
        key_env: "CONF103_KIMI_PLAN_KEY",
        wire_api: "anthropic",
        cells: [
            (
                "chat",
                "/v1/chat/completions",
                "/coding/v1/chat/completions",
            ),
            ("responses", "/v1/responses", "/coding/v1/responses"),
            ("anthropic", "/v1/messages", "/coding/v1/messages"),
        ],
    },
];

async fn rig(tag: &str) -> (Vec<testkit::MockUpstream>, String, std::path::PathBuf) {
    let mut mocks = Vec::new();
    for _ in 0..ENTRIES.len() {
        mocks.push(testkit::MockUpstream::start().await.unwrap());
    }
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let dir = testkit::tempdir(tag);

    let mut providers = String::new();
    for (i, e) in ENTRIES.iter().enumerate() {
        std::env::set_var(e.key_env, format!("sk-{}", e.name));
        let port = mocks[i].addr.port();
        providers.push_str(&format!(
            r#"  - name: {name}
    region: cn
    currency: CNY
    urls:
      chat:      http://127.0.0.1:{port}{chat}
      responses: http://127.0.0.1:{port}{responses}
      anthropic: http://127.0.0.1:{port}{anthropic}
    api_key_env: {key_env}
    wire_api: {wire_api}
    supports: [chat, responses, anthropic]
    account: coding_plan
    models:
      - id: {model}
        context: 1m
        price:
          input_miss: 0.02
          input_hit: 0.002
          cache_write: 0.02
          output: 0.1
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "mock upstream (no price; test fixture)"
"#,
            name = e.name,
            chat = e.cells[0].2,
            responses = e.cells[1].2,
            anthropic = e.cells[2].2,
            key_env = e.key_env,
            wire_api = e.wire_api,
            model = e.model,
        ));
    }

    let config = format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
{providers}
aliases: {{}}
plugins: []
fallback: []
"#
    );
    std::fs::write(dir.join("config.yaml"), config).unwrap();
    (mocks, listen_addr, dir)
}

async fn serve(dir: &std::path::Path, listen_addr: &str) -> tokio::task::JoinHandle<i32> {
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let addr = listen_addr.to_string();
    let task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&addr);
    task
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_103_corrected_roster_serves_three_lines() {
    let (mocks, listen_addr, dir) = rig("conf103").await;
    for m in &mocks {
        for _ in 0..3 {
            m.queue(CannedResponse::json(200, "OK", OK.as_bytes()));
        }
    }
    let task = serve(&dir, &listen_addr).await;

    for (i, e) in ENTRIES.iter().enumerate() {
        let mock = &mocks[i];
        for (protocol, inbound_path, cell_path) in e.cells {
            let body = format!(
                r#"{{"model":"{}/{m}","messages":[{{"role":"user","content":"x"}}],"stream":false}}"#,
                e.name,
                m = e.model
            );
            let (status, _b, _h) =
                testkit::http_post(&listen_addr, inbound_path, body.as_bytes(), &[]);
            assert_eq!(
                status, 200,
                "{} declared the {protocol} cell, so it must serve it natively",
                e.name
            );
            let seen = mock.requests();
            let last = seen.last().expect("the mock recorded a request");
            assert_eq!(
                last.path, cell_path,
                "{} served the {protocol} cell at its own URL",
                e.name
            );
            // Mutation (b) alone (spec §2 / AGENTS constraint 1): the
            // top-level `model` value the client wrote — the route id, built
            // above as `{entry}/{model}` — is rewritten to the resolved
            // route's provider-native model id before the bytes are
            // forwarded; every other byte is the client's own. Build the
            // expectation by rewriting exactly that one value, the same way
            // conf_100 states its two permitted mutations.
            let expected = {
                let mut v: serde_json::Value = serde_json::from_str(&body).unwrap();
                v["model"] = serde_json::Value::String(e.model.to_string());
                serde_json::to_string(&v).unwrap()
            };
            assert_eq!(
                String::from_utf8_lossy(&last.body),
                expected,
                "the client's own bytes, modulo mutation (b) alone (the top-level `model` value rewritten to the native id)"
            );
        }
    }

    // The declaration invariants the edit must not break, and which are
    // load-time refusals when broken: the config above loaded, so
    // `wire_api ∈ supports` (`config.rs:2091`) and `set(urls) == set(supports)`
    // (§4.9) hold for every entry; and no (provider, model) pair repeats.
    let mut ids = ENTRIES
        .iter()
        .map(|e| (e.name, e.model))
        .collect::<Vec<_>>();
    ids.sort_unstable();
    let before = ids.len();
    ids.dedup();
    assert_eq!(before, ids.len(), "no (provider, model) repeats");

    task.abort();
}
