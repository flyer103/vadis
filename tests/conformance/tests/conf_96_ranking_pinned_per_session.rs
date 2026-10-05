//! CONF-96 (ADR-049 §6 rule 1 — the ranking is pinned to the session, and the
//! pin has a limit): **one session never pays a second re-prefill for a
//! ranking change.**
//!
//! A session spills onto the cheaper candidate; a **reload** changes the prices
//! so a different candidate is now cheapest; the **same** session's next turn
//! stays on its pinned route (the now-cheaper mock receives nothing, the
//! session's record shows no second `plan_switch`, and the ledger reconciles —
//! one re-prefill, not two), while a **new** session takes the new ranking.
//!
//! The reload is observed the way the operator observes it: rewrite the config
//! file, then wait until `GET /health`'s `config_digest` moves (the accepted
//! reload's own evidence, [`book/operations.md`]) — no sleep guesswork.
//!
//! This is ADR-014's Background constraint 1 pinned as a case: the capability
//! must not reintroduce the per-request flip the design exists to refuse.
//!
//! Red at the base: `overflow_selection` does not load.

#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use vadis_conformance::testkit;

struct Rig {
    plan: testkit::MockUpstream,
    cheap: testkit::MockUpstream,
    dear: testkit::MockUpstream,
    listen_addr: String,
    dir: std::path::PathBuf,
}

async fn rig(tag: &str) -> Rig {
    let plan = testkit::MockUpstream::start().await.unwrap();
    let cheap = testkit::MockUpstream::start().await.unwrap();
    let dear = testkit::MockUpstream::start().await.unwrap();
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let dir = testkit::tempdir(tag);

    for n in ["CONF96_PLAN", "CONF96_CHEAP", "CONF96_DEAR"] {
        std::env::set_var(n, "sk-fixture");
    }

    std::fs::write(
        dir.join("config.yaml"),
        config_text(
            listen_port,
            plan.addr.port(),
            cheap.addr.port(),
            dear.addr.port(),
            // p-cheap is the cheaper one to start with.
            "0.002",
            "0.006",
        ),
    )
    .unwrap();
    Rig {
        plan,
        cheap,
        dear,
        listen_addr,
        dir,
    }
}

/// The config the case ships. `cheap_miss` / `dear_miss` are the two metered
/// prices, so the reload arm flips them and the ranking inverts.
fn config_text(
    listen: u16,
    plan: u16,
    cheap: u16,
    dear: u16,
    cheap_miss: &str,
    dear_miss: &str,
) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}
providers:
  - name: p-plan
    urls: {{ chat: "http://127.0.0.1:{plan}/v1/chat/completions" }}
    api_key_env: CONF96_PLAN
    wire_api: chat
    supports: [chat]
    account: coding_plan
    models: [ {{ id: m, context: 128k, price: {{ input_miss: 0.001, input_hit: 0.0001, cache_write: 0.0, output: 0.002, peak: {{ multiplier: 1.0, windows: [] }} }}, source: "fixture" }} ]
  - name: p-cheap
    urls: {{ chat: "http://127.0.0.1:{cheap}/v1/chat/completions" }}
    api_key_env: CONF96_CHEAP
    wire_api: chat
    supports: [chat]
    account: api
    models: [ {{ id: m, context: 128k, price: {{ input_miss: {cheap_miss}, input_hit: 0.0001, cache_write: 0.0, output: 0.003, peak: {{ multiplier: 1.0, windows: [] }} }}, source: "fixture" }} ]
  - name: p-dear
    urls: {{ chat: "http://127.0.0.1:{dear}/v1/chat/completions" }}
    api_key_env: CONF96_DEAR
    wire_api: chat
    supports: [chat]
    account: api
    models: [ {{ id: m, context: 128k, price: {{ input_miss: {dear_miss}, input_hit: 0.0001, cache_write: 0.0, output: 0.009, peak: {{ multiplier: 1.0, windows: [] }} }}, source: "fixture" }} ]
aliases: {{}}
plugins: []
fallback: []
plan_policy:
  family: m
  primary: p-plan/m
  overflow: p-cheap/m
  on_primary_exhausted: spill
  recover: probe
  cooldown: 0s
  overflow_selection: cheapest
"#
    )
}

fn post(addr: &str, session: &str) -> (u16, Vec<u8>, Vec<(String, String)>) {
    let body = format!(
        r#"{{"model":"p-plan/m","messages":[{{"role":"user","content":"turn"}}],"prompt_cache_key":"{session}","stream":false}}"#
    );
    testkit::http_post(addr, "/v1/chat/completions", body.as_bytes(), &[])
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

/// Waits until the accepted reload shows on `/health` (the digest moved).
fn wait_for_reload(addr: &str, before: &str) {
    for _ in 0..100 {
        let h = http_get(addr, "/health");
        if h["config"]["config_digest"].as_str() != Some(before) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("the reload never showed on /health");
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

/// The pin: within one session the chosen candidate does not move.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_96_a_live_session_keeps_its_candidate_across_a_reload() {
    let r = rig("conf96-pin").await;
    r.plan.queue(testkit::plan_forbidden_403());
    r.cheap.queue(testkit::plan_ok("turn-1"));
    r.cheap.queue(testkit::plan_ok("turn-2-pinned"));
    r.dear.queue(testkit::plan_ok("a-new-session"));

    let cfg = r.dir.join("config.yaml").to_string_lossy().into_owned();
    let task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&r.listen_addr);

    // Turn 1: session S spills onto the cheapest candidate (p-cheap).
    let (s1, _b1, _h1) = post(&r.listen_addr, "S");
    assert_eq!(s1, 200);
    assert_eq!(r.cheap.requests().len(), 1, "the cheapest served turn 1");

    // The reload: the prices flip, so p-dear is now the cheapest. Wait for the
    // accepted revision (the digest moves) rather than for a guessed delay.
    let before = http_get(&r.listen_addr, "/health")["config"]["config_digest"]
        .as_str()
        .expect("digest")
        .to_string();
    std::fs::write(
        r.dir.join("config.yaml"),
        config_text(
            r.listen_addr.rsplit(':').next().unwrap().parse().unwrap(),
            r.plan.addr.port(),
            r.cheap.addr.port(),
            r.dear.addr.port(),
            "0.008", // p-cheap is now the dearer one
            "0.001", // p-dear is now the cheapest
        ),
    )
    .unwrap();
    wait_for_reload(&r.listen_addr, &before);

    // Turn 2 of the SAME session: the pin holds.
    let (s2, _b2, _h2) = post(&r.listen_addr, "S");
    assert_eq!(s2, 200);
    assert_eq!(
        r.cheap.requests().len(),
        2,
        "session S stayed on its pinned candidate — no second re-prefill"
    );
    assert_eq!(
        r.dear.requests().len(),
        0,
        "a ranking change mid-session moved nothing"
    );

    // A NEW session takes the reloaded ranking.
    let (s3, _b3, _h3) = post(&r.listen_addr, "T");
    assert_eq!(s3, 200);
    assert_eq!(
        r.dear.requests().len(),
        1,
        "a new session takes the reloaded ranking"
    );

    task.abort();
    let _ = task.await;

    // The ledger reconciles: session S carries exactly one plan displacement
    // (the spill), not two — a re-ranking would have written a second.
    let s_records: Vec<_> = trace_records(&r.dir)
        .into_iter()
        .filter(|x| x["identity"]["session"] == "S")
        .collect();
    assert_eq!(s_records.len(), 2, "two turns, two records");
    assert_eq!(
        s_records
            .iter()
            .filter(|x| !x["result"]["plan_switch"].is_null())
            .count(),
        1,
        "one displacement for the session, not one per turn"
    );
}
