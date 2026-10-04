//! CONF-15 (DESIGN §10 conformance · cache): same-session two-turn
//! passthrough, `prefix_continuity == 1.0`.
//!
//! The measured mechanism behind the captured 99.2% `cached_tokens` hit:
//! a stateless client resends the whole conversation every turn, so turn
//! 2's prefix blocks are turn 1's blocks plus new ones — a strict
//! superset. Continuity (spec §6: the longest common block ratio
//! **relative to the previous request of the session**) must stay 1.0
//! through the vadis's own pipeline (receive → remove vadis-owned
//! keys → forward → ledger → trace).
//!
//! Drives the real `vadis_cli::serve` assembly over loopback HTTP
//! against the testkit mock upstream; the measurement is read back from
//! the hourly JSONL trace and cross-checked against the `cache_ledger`
//! projection and the `request.received` event rows (the join CONF-24
//! anchors on).
//!
//! Note on enumeration order: this fixture carries `messages` and no
//! `tools`, so it is insensitive to the 2026-09-20 Plan A block-order
//! change (template order: system → tools → conversation; spec §6) — its
//! expectation value predates and survives the change unchanged. The
//! codex-shaped body (`input` serialized before `tools`) is CONF-31's
//! object.

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, CannedResponse};

fn config_yaml(upstream_port: u16, listen_port: u16) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: mock
    urls:
      chat: http://127.0.0.1:{upstream_port}/v1/chat/completions
    api_key_env: CONF15_MOCK_KEY
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
plugins: []
fallback: []
"#
    )
}

/// The stateless-client conversation shape (from the captured codex/hermes
/// traffic): same `prompt_cache_key` every turn, full history resent, one
/// assistant answer and one new user message appended per turn. The
/// `vadis_meta` echo rides along — removing it must not move any block
/// hash (CONF-10's claim, exercised here on the live path).
fn turn_body(turn: usize) -> String {
    // Array contents, comma-joined; the brackets are added once below.
    let e1 = r#"{"role":"user","content":"question one"}"#;
    let e2 = r#"{"role":"assistant","content":"answer one"}"#;
    let e3 = r#"{"role":"user","content":"question two"}"#;
    let messages = if turn >= 2 {
        format!("{e1},{e2},{e3}")
    } else {
        e1.to_string()
    };
    format!(
        r#"{{"model":"mock/glm","messages":[{messages}],"prompt_cache_key":"sess-conf15","vadis_meta":{{"echo":true}}}}"#
    )
}

/// One canned 200 with a usage object whose cached_tokens mirrors the
/// captured shape (turn 2 nearly fully cached).
fn canned(turn: usize) -> CannedResponse {
    let (prompt, cached) = if turn == 1 {
        (14409, 960)
    } else {
        (14520, 14400)
    };
    let body = format!(
        r#"{{"id":"resp-{turn}","choices":[{{"index":0,"message":{{"role":"assistant","content":"ok"}},"finish_reason":"stop"}}],"usage":{{"prompt_tokens":{prompt},"completion_tokens":111,"total_tokens":{},"prompt_tokens_details":{{"cached_tokens":{cached}}}}}}}"#,
        prompt + 111
    );
    CannedResponse::json(200, "OK", body.as_bytes())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_15_prefix_continuity() {
    let dir = testkit::tempdir("conf15");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    // Two distinct canned answers: queue order is turn 1, turn 2 (the
    // last one repeats, but only two requests are sent).
    upstream.queue(canned(1));
    upstream.queue(canned(2));

    let listen_port = testkit::free_port();
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(upstream.addr.port(), listen_port)).unwrap();

    std::env::set_var("CONF15_MOCK_KEY", "sk-conf15");
    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&format!("127.0.0.1:{listen_port}"));

    // Two turns of the same session, sequential (adjacent requests).
    for turn in 1..=2 {
        let (status, body, _headers) = testkit::http_post(
            &format!("127.0.0.1:{listen_port}"),
            "/v1/chat/completions",
            turn_body(turn).as_bytes(),
            &[],
        );
        assert_eq!(status, 200, "turn {turn} status; body: {:?}", body);
    }

    serve_task.abort();
    let _ = serve_task.await;

    // The trace: exactly two DecisionRecords, both under the session.
    let trace_dir = dir.join("state/traces");
    let mut records: Vec<serde_json::Value> = Vec::new();
    for entry in std::fs::read_dir(&trace_dir).expect("trace dir exists") {
        let entry = entry.unwrap();
        if entry.path().extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        for line in std::fs::read_to_string(entry.path()).unwrap().lines() {
            records.push(serde_json::from_str(line).expect("each line is one record"));
        }
    }
    assert_eq!(records.len(), 2, "one record per request");

    let turn1 = &records[0];
    let turn2 = &records[1];
    for (i, r) in records.iter().enumerate() {
        assert_eq!(
            r["identity"]["session"], "sess-conf15",
            "record {i} carries the session key"
        );
        assert_eq!(r["identity"]["turn_index"], (i + 1) as u64);
        assert_eq!(r["errors"].as_array().map(Vec::len), Some(0));
    }

    // Turn 1 has no previous request: continuity is absent, not invented.
    assert!(
        turn1["prefix"]["continuity"].is_null(),
        "first request of a session has no previous request to measure against"
    );

    // The object under test: turn 2's blocks are turn 1's plus new ones,
    // and the vadis preserved every block hash — continuity == 1.0.
    let blocks1 = turn1["prefix"]["blocks"].as_array().expect("blocks");
    let blocks2 = turn2["prefix"]["blocks"].as_array().expect("blocks");
    assert!(
        blocks2.len() > blocks1.len(),
        "the stateless-client shape: turn 2 resends the full history ({} vs {} blocks)",
        blocks2.len(),
        blocks1.len()
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
        "same-session passthrough continuity must be exactly 1.0"
    );

    // The measurement chain behind the number: the cache_ledger holds
    // turn 2's block set (what a hypothetical turn 3 would be measured
    // against), and both turns left a request.received event row.
    let store = vadis_store::SqliteStore::open(&dir.join("state/router.db")).unwrap();
    use vadis_core::store::{Query, QueryRow, Store as _};
    match store
        .query(Query::CacheLedgerBlocks {
            session_key: "sess-conf15",
        })
        .unwrap()
    {
        QueryRow::CacheLedger(ledger) => {
            assert_eq!(
                ledger.len(),
                blocks2.len(),
                "the ledger holds the LAST request's block set"
            );
            for (l, b) in ledger.iter().zip(blocks2.iter()) {
                assert_eq!(l.hash, b["hash"].as_str().unwrap());
            }
        }
        other => panic!("expected a ledger, got {other:?}"),
    }
    let QueryRow::Events(events) = store.query(Query::AllEvents).unwrap() else {
        panic!("events");
    };
    let received: Vec<_> = events
        .iter()
        .filter(|e| e.kind_raw == "request.received" && e.session.as_deref() == Some("sess-conf15"))
        .collect();
    assert_eq!(received.len(), 2, "one request.received row per turn");

    // And the upstream really saw the full resent history both times
    // (the byte-level superset shape the ratio is computed from).
    let reqs = upstream.requests();
    assert_eq!(reqs.len(), 2, "two upstream attempts, no failover");
}
