//! CONF-92 (ADR-049 §3 rule 7 + §7(a) — per-key presence is inspectable and
//! the secret boundary holds at N names): **names appear, values never.**
//!
//! Limb (a) — presence. A pool `[A, B]` with B unset: the provider is
//! **available** (`api_key_present` is the pool's disjunction) while
//! `/health`'s `keys[]` lists **both** names in order, `A.present: true` and
//! `B.present: false`. The one-key spelling's own two fields keep their
//! existing values, so no existing reader moves.
//!
//! Limb (b) — the secret boundary. With a canary value in the environment for
//! **every** name the config writes, no canary byte appears on any surface
//! this case can read in-process: `/health`'s body, the trace JSONL, and the
//! event log. (The stdout/stderr arm follows CONF-70's subprocess method and
//! lands with the implementation — an in-process `serve` has no stdout to
//! read from here.)
//!
//! Red at the base: `api_keys` does not load, so limb (a) has nothing to
//! inspect.

#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::net::TcpStream;

use vadis_conformance::testkit;

/// The canary planted as every key's value. A leak is any occurrence of this
/// byte string on any surface.
const CANARY: &str = "CANARY-do-not-leak-9f3a";

async fn rig(tag: &str) -> (testkit::MockUpstream, String, std::path::PathBuf) {
    let p = testkit::MockUpstream::start().await.unwrap();
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let dir = testkit::tempdir(tag);

    std::env::set_var("CONF92_K1", CANARY);
    // CONF92_K2 is deliberately never set: the pool is half-present.
    std::env::remove_var("CONF92_K2");

    let config = format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: p
    urls:
      chat: http://127.0.0.1:{p_port}/v1/chat/completions
    api_keys: [CONF92_K1, CONF92_K2]
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

aliases: {{}}
plugins: []
fallback: []
"#,
        p_port = p.addr.port(),
    );
    std::fs::write(dir.join("config.yaml"), config).unwrap();
    (p, listen_addr, dir)
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

/// (a) The pool's presence, per key, and the provider's disjunction.
#[ignore = "CONF-92: depends on the credential pool (api_keys) and /health's keys[]"]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_92_pool_presence_is_reported_per_key() {
    let (_p, listen_addr, dir) = rig("conf92-presence").await;
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);

    let h = http_get(&listen_addr, "/health");
    let prov = &h["providers"][0];
    assert_eq!(prov["name"], "p");
    assert_eq!(
        prov["api_key_env"], "CONF92_K1",
        "the one-key spelling reports the pool's first name (back-compat)"
    );
    assert_eq!(
        prov["api_key_present"], true,
        "availability is the pool's disjunction"
    );
    let keys = prov["keys"].as_array().expect("keys[] present");
    assert_eq!(keys.len(), 2, "one entry per declared name, in order");
    assert_eq!(keys[0]["env"], "CONF92_K1");
    assert_eq!(keys[0]["present"], true);
    assert_eq!(keys[1]["env"], "CONF92_K2");
    assert_eq!(
        keys[1]["present"], false,
        "the unset name is listed, not hidden"
    );

    // The body carries no value, only names.
    assert!(
        !h.to_string().contains(CANARY),
        "no credential value on /health"
    );
    task.abort();
}

/// (b) The canary: no surface this case can read carries a key value.
#[ignore = "CONF-92: depends on the credential pool (api_keys) and /health's keys[]"]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_92_no_key_value_reaches_any_readable_surface() {
    let (p, listen_addr, dir) = rig("conf92-canary").await;
    p.queue(testkit::plan_ok("served"));
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);

    let body = r#"{"model":"p/m","messages":[{"role":"user","content":"c"}],"stream":false}"#;
    let (_s, _b, _h) =
        testkit::http_post(&listen_addr, "/v1/chat/completions", body.as_bytes(), &[]);

    let health = http_get(&listen_addr, "/health").to_string();
    assert!(!health.contains(CANARY), "/health names no value");

    task.abort();
    let _ = task.await;

    for entry in std::fs::read_dir(dir.join("state/traces")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains(CANARY), "the trace names no value: {path:?}");
    }
    let store = vadis_store::SqliteStore::open(&dir.join("state/vadis.db")).unwrap();
    use vadis_core::store::{Query, QueryRow, Store as _};
    let QueryRow::Events(rows) = store.query(Query::AllEvents).unwrap() else {
        panic!("events query");
    };
    for row in rows {
        assert!(
            !row.payload.to_string().contains(CANARY),
            "the event log names no value: {row:?}"
        );
    }
}
