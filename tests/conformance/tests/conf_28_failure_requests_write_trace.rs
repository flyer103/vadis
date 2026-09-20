//! CONF-28 (DESIGN §12.8, spec §6 + §8): **a terminal failure still writes
//! its trace line** — the request that ends 502 is exactly the request the
//! analysis truth is for. Measured motivation (the smoke): nine requests
//! ending 502 left **zero** lines under `trace.dir` while successes landed
//! normally; `decision` / `errors[]` / `failover_from` / `usage_missing`
//! were all lost.
//!
//! The normative case: an upstream **400** (deterministic `format_error`,
//! never retried) must appear as exactly one trace line whose
//! `errors[].details.error_class == "format_error"` and
//! `result.status == 502`, with `usage_missing: true` (no usage arrived,
//! nothing charged, no cost invented). Two companion cases pin the same
//! invariant on the connect-failure and pre-route paths.

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
    base_url: http://127.0.0.1:{upstream_port}/v1
    api_key_env: CONF28_MOCK_KEY
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
"#,
    )
}

const CLIENT_BODY: &str = r#"{"model":"mock/glm","messages":[{"role":"user","content":"hi"}],"prompt_cache_key":"conf28-sess"}"#;

/// Every JSONL line under the trace dir, parsed.
fn read_records(trace_dir: &std::path::Path) -> Vec<serde_json::Value> {
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

/// The normative case: upstream 400 → one trace line, class carried,
/// client status 502, usage missing and nothing charged.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_28_upstream_400_writes_one_failure_line() {
    let dir = testkit::tempdir("conf28-400");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    upstream.queue(CannedResponse::json(
        400,
        "Bad Request",
        br#"{"error":{"message":"Unknown parameter: 'x'","type":"invalid_request_error"}}"#,
    ));

    let listen_port = testkit::free_port();
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(upstream.addr.port(), listen_port)).unwrap();
    std::env::set_var("CONF28_MOCK_KEY", "sk-conf28");
    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
    testkit::wait_listening(&format!("127.0.0.1:{listen_port}"));

    let (status, _body, _headers) = testkit::http_post(
        &format!("127.0.0.1:{listen_port}"),
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[],
    );
    serve_task.abort();
    let _ = serve_task.await;

    // The client saw the mapped 502 (a deterministic rejection, CONF-14's
    // territory) — this case is about what the trace did meanwhile.
    assert_eq!(status, 502, "a deterministic upstream 400 maps to 502");

    let records = read_records(&dir.join("state/traces"));
    assert_eq!(records.len(), 1, "exactly one trace line for the request");
    let rec = &records[0];

    // The failure is the analysis subject: errors[0] carries the class.
    let errors = rec["errors"].as_array().expect("errors is an array");
    assert_eq!(errors.len(), 1, "one error entry");
    assert_eq!(errors[0]["kind"], "upstream_error");
    assert_eq!(
        errors[0]["details"]["error_class"], "format_error",
        "the classification travels in the trace"
    );
    assert_eq!(
        errors[0]["details"]["upstream_status"], 400,
        "the upstream's own status is recorded"
    );

    // Result fields per spec §6: the client-facing status, the upstream's
    // own status mirrored into result.upstream_status (the field
    // completion — the record answers "which provider said what"), and
    // the failover origin (absent here — format_error never fails over).
    assert_eq!(rec["result"]["status"], 502);
    assert_eq!(rec["result"]["upstream_status"], 400);
    assert_eq!(rec["result"]["failover_from"], serde_json::Value::Null);

    // The attempted route is named, not blank: the failure record
    // answers "which provider died" first.
    assert_eq!(rec["decision"]["provider"], "mock");
    assert_eq!(rec["decision"]["model"], "glm");

    // No usage arrived: usage_missing, zero usage, nothing charged.
    assert_eq!(rec["usage_missing"], true);
    assert_eq!(rec["cost"]["total"], 0);

    // The join keys survive on the failure path too (CONF-24's contract
    // is per request, not per success).
    assert!(rec["identity"]["request_id"].is_string());
    assert!(rec["identity"]["event_id"].is_i64());
    // The facts the router knew before the failure are not lost.
    assert_eq!(rec["identity"]["session"], "conf28-sess");
    assert_eq!(rec["decision"]["requested_model"], "mock/glm");
}

/// Companion: the upstream is unreachable (connect refused) and the chain
/// is empty — the terminal 502 still leaves exactly one trace line, with
/// the terminal error entry and no invented usage.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_28_connect_failure_writes_one_line() {
    let dir = testkit::tempdir("conf28-conn");
    // A port with no listener: bind then drop to pick a free one.
    let dead_port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let p = l.local_addr().unwrap().port();
        drop(l);
        p
    };
    let listen_port = testkit::free_port();
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(dead_port, listen_port)).unwrap();
    std::env::set_var("CONF28_MOCK_KEY", "sk-conf28b");
    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
    testkit::wait_listening(&format!("127.0.0.1:{listen_port}"));

    let (status, _body, _headers) = testkit::http_post(
        &format!("127.0.0.1:{listen_port}"),
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[],
    );
    serve_task.abort();
    let _ = serve_task.await;

    assert_eq!(status, 502, "connect refused with an empty chain is a 502");
    let records = read_records(&dir.join("state/traces"));
    assert_eq!(records.len(), 1, "the connect failure left its line");
    let rec = &records[0];
    assert_eq!(rec["result"]["status"], 502);
    assert_eq!(rec["usage_missing"], true);
    let errors = rec["errors"].as_array().expect("errors array");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["kind"], "upstream_error");
}

/// Companion: a pre-route rejection (unknown model → 404) also leaves its
/// line — the trace's job starts before the first attempt.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_28_pre_route_rejection_writes_one_line() {
    let dir = testkit::tempdir("conf28-pre");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    let listen_port = testkit::free_port();
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(upstream.addr.port(), listen_port)).unwrap();
    std::env::set_var("CONF28_MOCK_KEY", "sk-conf28c");
    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
    testkit::wait_listening(&format!("127.0.0.1:{listen_port}"));

    let (status, _body, _headers) = testkit::http_post(
        &format!("127.0.0.1:{listen_port}"),
        "/v1/chat/completions",
        br#"{"model":"mock/nope","messages":[{"role":"user","content":"hi"}]}"#,
        &[],
    );
    serve_task.abort();
    let _ = serve_task.await;

    assert_eq!(status, 404, "an unresolved route is answered 404");
    assert_eq!(upstream.requests().len(), 0, "nothing reached the upstream");
    let records = read_records(&dir.join("state/traces"));
    assert_eq!(records.len(), 1, "the pre-route rejection left its line");
    let rec = &records[0];
    assert_eq!(rec["result"]["status"], 404);
    assert_eq!(rec["decision"]["requested_model"], "mock/nope");
    let errors = rec["errors"].as_array().expect("errors array");
    assert_eq!(errors.len(), 1);
}
