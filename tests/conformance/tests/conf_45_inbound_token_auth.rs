//! CONF-45 (§12.8, spec §4.7 + §8): **the inbound token-auth boundary
//! guard, six ways** — ① no token → `401` (`unauthorized`, §8's body
//! verbatim); ② a wrong token → `401`; ③ the right token, once as
//! `Authorization: Bearer ***` and once as `x-api-key: ***` → forwarded
//! normally, with the upstream-visible bytes unchanged (the guard adds
//! nothing to the body); ④ `GET /health` with no token → `200`; ⑤ no
//! `server.auth_token_env` ⇒ behaviour identical to before the key
//! existed (no auth anywhere); ⑥ the key written but its environment
//! variable missing/empty ⇒ the process does not start (non-zero exit,
//! the variable named on stderr).
//!
//! The 401's trace line — one record, `errors[].kind == "unauthorized"`,
//! `usage_missing: true`, priced nowhere — is asserted with them (spec
//! §6's pre-pipeline record class, field by field).

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, CannedResponse};

fn config_yaml(upstream_port: u16, listen_port: u16, auth_line: &str) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s{auth_line} }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: mock
    urls:
      chat: http://127.0.0.1:{upstream_port}/v1/chat/completions
    api_key_env: CONF45_MOCK_KEY
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

const CLIENT_BODY: &str = r#"{"model":"mock/glm","messages":[{"role":"user","content":"hi"}],"prompt_cache_key":"conf45-sess"}"#;

const UPSTREAM_OK: &str = r#"{"id":"resp-1","choices":[{"index":0,"message":{"role":"assistant","content":"ok"}}],"usage":{"prompt_tokens":100,"completion_tokens":10,"total_tokens":110}}"#;

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

/// ①②③④ The guarded run: auth on, the six boundary behaviours probed
/// over real loopback HTTP against a bare-TCP mock upstream.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_45_inbound_token_auth_guard() {
    let dir = testkit::tempdir("conf45-auth");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    upstream.queue(CannedResponse::json(200, "OK", UPSTREAM_OK.as_bytes()));
    upstream.queue(CannedResponse::json(200, "OK", UPSTREAM_OK.as_bytes()));

    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(
        &config_path,
        config_yaml(
            upstream.addr.port(),
            listen_port,
            ", auth_token_env: CONF45_GUARD_TOKEN",
        ),
    )
    .unwrap();
    std::env::set_var("CONF45_MOCK_KEY", "sk-conf45");
    std::env::set_var("CONF45_GUARD_TOKEN", "tok-conf45-secret");

    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);

    // ① no token → 401, §8's body verbatim, X-Router-Request-Id present.
    let (status, body, headers) = testkit::http_post(
        &listen_addr,
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[],
    );
    assert_eq!(status, 401, "no token must be refused, got {status}");
    let v: serde_json::Value = serde_json::from_slice(&body).expect("error body json");
    assert_eq!(v["error"]["type"], "unauthorized");
    assert_eq!(
        v["error"]["message"],
        "inbound auth: no token presented (send it as 'Authorization: Bearer <token>' or 'x-api-key: <token>')"
    );
    assert_eq!(v["error"]["details"]["header"], serde_json::Value::Null);
    let req_id = v["error"]["request_id"].as_str().expect("request_id");
    assert!(!req_id.is_empty());
    let hdr_id = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("x-router-request-id"))
        .map(|(_, v)| v.as_str())
        .expect("X-Router-Request-Id on a 401 (§8's always)");
    assert_eq!(hdr_id, req_id, "the header and the body name the same id");

    // ② a wrong token (differs only in the last byte) → 401; the body
    //    names the header that was read.
    let (status, body, _h) = testkit::http_post(
        &listen_addr,
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[("authorization", "Bearer tok-conf45-secreX")],
    );
    assert_eq!(status, 401, "wrong token must be refused");
    let v: serde_json::Value = serde_json::from_slice(&body).expect("error body json");
    assert_eq!(v["error"]["type"], "unauthorized");
    assert_eq!(
        v["error"]["message"],
        "inbound auth: the presented token does not match the value of the environment variable named by server.auth_token_env"
    );
    assert_eq!(v["error"]["details"]["header"], "authorization");

    // The refused requests never reached the upstream (no wire bytes
    // exist for them, spec §4.7).
    assert_eq!(
        upstream.requests().len(),
        0,
        "no upstream contact for a 401"
    );

    // ③ the right token — first as Authorization: Bearer, then as
    //    x-api-key — is forwarded normally both times.
    for headers_sent in [
        vec![("authorization", "Bearer tok-conf45-secret")],
        vec![("x-api-key", "tok-conf45-secret")],
    ] {
        let (status, body, _h) = testkit::http_post(
            &listen_addr,
            "/v1/chat/completions",
            CLIENT_BODY.as_bytes(),
            &headers_sent,
        );
        assert_eq!(status, 200, "right token via either header forwards");
        assert_eq!(
            body,
            UPSTREAM_OK.as_bytes(),
            "the response is the upstream's bytes verbatim"
        );
    }
    // The guard adds nothing to the body: the upstream saw the client's
    // own bytes minus the vadis-owned member (CONF-27's class; here it
    // is what makes "auth on" byte-faithful, DESIGN §12.11).
    let requests = upstream.requests();
    assert_eq!(
        requests.len(),
        2,
        "exactly one upstream attempt per admission"
    );
    let b1: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    let b2: serde_json::Value = serde_json::from_slice(&requests[1].body).unwrap();
    let client: serde_json::Value = serde_json::from_str(CLIENT_BODY).unwrap();
    assert_eq!(b1["messages"], client["messages"]);
    assert_eq!(b2["messages"], client["messages"]);
    // The one permitted body mutation: the resolved route's native id
    // (CONF-27) — the guard adds nothing on top of it.
    assert_eq!(b1["model"], "glm");

    // ④ GET /health with no token → 200 (the structural exemption).
    let (status, body) = http_get(&listen_addr, "/health");
    assert_eq!(status, 200, "/health never needs a token");
    let v: serde_json::Value = serde_json::from_str(body.trim()).expect("health json");
    assert_eq!(v["auth"]["required"], true);
    assert_eq!(v["auth"]["env"], "CONF45_GUARD_TOKEN");

    serve_task.abort();
    let _ = serve_task.await;

    // The 401s left their trace lines: one record each, pre-pipeline
    // class, priced nowhere (spec §6's table).
    let records = read_records(&dir.join("state/traces"));
    assert_eq!(records.len(), 4, "two 401s + two forwarded requests");
    let refused: Vec<&serde_json::Value> = records
        .iter()
        .filter(|r| {
            r["errors"]
                .as_array()
                .is_some_and(|e| !e.is_empty() && e[0]["kind"] == "unauthorized")
        })
        .collect();
    assert_eq!(refused.len(), 2, "one trace line per refused request");
    for rec in &refused {
        assert_eq!(rec["identity"]["event_id"], 0, "no request.received row");
        assert_eq!(rec["decision"]["provider"], "");
        assert_eq!(rec["decision"]["model"], "");
        assert_eq!(rec["decision"]["requested_model"], serde_json::Value::Null);
        assert_eq!(rec["usage_missing"], true);
        assert_eq!(rec["cost"]["total"], 0);
        assert_eq!(rec["result"]["status"], 401);
        assert_eq!(rec["result"]["upstream_status"], serde_json::Value::Null);
        let errors = rec["errors"].as_array().unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0]["kind"], "unauthorized");
    }
    // The first refusal carried no header at all; the second named
    // `authorization` (the diagnostic, spec §4.7).
    assert_eq!(
        refused[0]["errors"][0]["details"]["header"],
        serde_json::Value::Null
    );
    assert_eq!(
        refused[1]["errors"][0]["details"]["header"],
        "authorization"
    );
}

/// ⑤ No `auth_token_env` ⇒ no auth anywhere: a tokenless request is
/// forwarded (today's behaviour, untouched) and /health reports
/// `auth: {required: false}` with no other key.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_45_no_key_means_no_auth() {
    let dir = testkit::tempdir("conf45-nokey");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    upstream.queue(CannedResponse::json(200, "OK", UPSTREAM_OK.as_bytes()));

    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(
        &config_path,
        config_yaml(upstream.addr.port(), listen_port, ""),
    )
    .unwrap();
    std::env::set_var("CONF45_MOCK_KEY", "sk-conf45");
    std::env::remove_var("CONF45_ROUTER_TOKEN");

    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);

    let (status, body, _h) = testkit::http_post(
        &listen_addr,
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[],
    );
    assert_eq!(
        status, 200,
        "no key ⇒ tokenless request forwarded as before"
    );
    assert_eq!(body, UPSTREAM_OK.as_bytes());

    let (status, body) = http_get(&listen_addr, "/health");
    assert_eq!(status, 200);
    let v: serde_json::Value = serde_json::from_str(body.trim()).expect("health json");
    assert_eq!(v["auth"], serde_json::json!({"required": false}));

    serve_task.abort();
    let _ = serve_task.await;
}

/// ⑥ The key written but the env var missing/empty ⇒ the process refuses
/// to start: non-zero exit (4), the variable named on stderr (CONF-23's
/// pattern for startup refusals, driven against the real assembly).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_45_missing_env_refuses_startup() {
    for (tag, value) in [("unset", None), ("empty", Some(""))] {
        let dir = testkit::tempdir(&format!("conf45-env-{tag}"));
        let listen_port = testkit::free_port();
        let config_path = dir.join("config.yaml");
        std::fs::write(
            &config_path,
            config_yaml(0, listen_port, ", auth_token_env: CONF45_ROUTER_TOKEN"),
        )
        .unwrap();
        match value {
            Some(v) => std::env::set_var("CONF45_ROUTER_TOKEN", v),
            None => std::env::remove_var("CONF45_ROUTER_TOKEN"),
        }
        let cfg = config_path.to_string_lossy().into_owned();
        let code = vadis_cli::serve(&cfg).await;
        assert_ne!(code, 0, "({tag}) a token-less start must be refused");
        // The stderr line itself is asserted by the run-evidence comment
        // (the message is frozen in DESIGN §12.11; the exit code is the
        // machine-checkable half). 4 = unsatisfiable environment
        // prerequisite (CONF-23's class).
        assert_eq!(code, 4, "({tag}) exit 4, got {code}");
    }
}

/// Minimal blocking GET, returning (status, body).
fn http_get(addr: &str, path: &str) -> (u16, String) {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(addr).expect("connect");
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    let text = String::from_utf8_lossy(&buf).into_owned();
    let status: u16 = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("status line");
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}
