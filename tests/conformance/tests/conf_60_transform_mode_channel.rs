//! CONF-60 (§12.8, spec §2.1/§6, ADR-019 §2): **the opt-in transform-mode
//! channel** — ① absence of `X-Vadis-Transform` ⇒ `passthrough`, the byte
//! path (upstream-visible bytes = client's modulo mutations (a)/(b)); ②
//! `X-Vadis-Transform: passthrough` ⇒ the same; ③
//! `X-Vadis-Transform: transform` ⇒ transform mode with **no rule engine
//! configured** ("asked, not applied": `transform_mode: "transform"` on the
//! trace, `transforms[]` empty, upstream bytes still byte-equal); ④ any
//! other value ⇒ `400 invalid_request` decided before the body is read,
//! §8's body shape, `X-Vadis-Request-Id` present, and the pre-pipeline
//! record class on the trace (`transform_mode: "passthrough"`, `usage_missing:
//! true`, `errors[].kind == "transform_error"`, priced nowhere).
//!
//! The whole case runs against the real `serve` assembly and a mock
//! upstream that records every request byte — the assertions compare what
//! the upstream **actually received**, never an intermediate structure.

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
    api_key_env: CONF60_MOCK_KEY
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

const CLIENT_BODY: &str = r#"{"model":"mock/glm","messages":[{"role":"user","content":"hi"}],"prompt_cache_key":"conf60-sess"}"#;

/// The bytes the upstream must see: the client's own bytes minus
/// `prompt_cache_key`… no — that key is *not* vadis-owned; the only
/// mutations are (a) vadis-owned top-level keys and (b) the model value.
/// For this body the expected upstream body is therefore the client's
/// bytes with `model` replaced by the native id `glm`.
const EXPECTED_UPSTREAM_BODY: &str = r#"{"model":"glm","messages":[{"role":"user","content":"hi"}],"prompt_cache_key":"conf60-sess"}"#;

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_60_transform_mode_opt_in_channel() {
    let dir = testkit::tempdir("conf60-mode");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    // ① no header, ② explicit passthrough, ③ transform (no engine), ④ is
    // refused before the wire, so three upstream answers suffice.
    for _ in 0..3 {
        upstream.queue(CannedResponse::json(200, "OK", UPSTREAM_OK.as_bytes()));
    }

    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(upstream.addr.port(), listen_port)).unwrap();
    std::env::set_var("CONF60_MOCK_KEY", "sk-conf60");

    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);

    // ① absence ⇒ passthrough: served, and the upstream saw the client's
    //    bytes modulo (a)/(b) only.
    let (status, _body, _h) = testkit::http_post(
        &listen_addr,
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[],
    );
    assert_eq!(status, 200, "no header must serve normally");

    // ② explicit `passthrough` ⇒ the same path.
    let (status, _b, _h) = testkit::http_post(
        &listen_addr,
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[("x-vadis-transform", "passthrough")],
    );
    assert_eq!(status, 200, "explicit passthrough must serve normally");

    // ③ `transform` with no configured engine ⇒ "asked, not applied":
    //    served, mode recorded, empty ledger.
    let (status, _b, _h) = testkit::http_post(
        &listen_addr,
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[("x-vadis-transform", "transform")],
    );
    assert_eq!(
        status, 200,
        "transform with no engine must serve (fail-open on the ledger, not the bytes)"
    );

    // ④ any other value ⇒ 400 invalid_request, §8's body, the request id
    //    echoed in header and body, and the upstream saw NOTHING (the mode
    //    is decided before the body is read or forwarded).
    let (status, body, headers) = testkit::http_post(
        &listen_addr,
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[("x-vadis-transform", "transfrm")], // the typo fixture
    );
    assert_eq!(status, 400, "a typo'd mode value must be refused");
    let v: serde_json::Value = serde_json::from_slice(&body).expect("error body json");
    assert_eq!(v["error"]["type"], "invalid_request");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("X-Vadis-Transform"),
        "the message names the header (got {:?})",
        v["error"]["message"]
    );
    let req_id = v["error"]["request_id"].as_str().expect("request_id");
    assert!(!req_id.is_empty());
    let hdr_id = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("x-vadis-request-id"))
        .map(|(_, v)| v.as_str())
        .expect("X-Vadis-Request-Id on the 400 (§8's always)");
    assert_eq!(hdr_id, req_id, "the header and the body name the same id");

    // The wire saw exactly the three admitted requests — the 400 never
    // reached it.
    let seen = upstream.requests();
    assert_eq!(seen.len(), 3, "the refused request never hit the wire");
    for (i, r) in seen.iter().enumerate() {
        assert_eq!(
            r.body,
            EXPECTED_UPSTREAM_BODY.as_bytes(),
            "request {i}: upstream bytes = client bytes modulo (a)/(b)"
        );
    }

    // The trace: three admitted records + one pre-pipeline refusal.
    let records = read_records(&dir.join("state/traces"));
    assert_eq!(
        records.len(),
        4,
        "one record per request, failures included"
    );
    let by_mode: Vec<&serde_json::Value> = records
        .iter()
        .filter(|r| r["result"]["status"] == 200)
        .collect();
    assert_eq!(by_mode.len(), 3);
    assert_eq!(by_mode[0]["transform_mode"], "passthrough");
    assert_eq!(by_mode[1]["transform_mode"], "passthrough");
    assert_eq!(by_mode[2]["transform_mode"], "transform");
    for r in &by_mode {
        // `transforms` is omitted on the wire when empty (spec §6).
        assert!(
            r.get("transforms").is_none(),
            "no engine ⇒ no ledger entries: {:?}",
            r.get("transforms")
        );
    }
    let refused: Vec<&serde_json::Value> = records
        .iter()
        .filter(|r| r["result"]["status"] == 400)
        .collect();
    assert_eq!(refused.len(), 1, "the typo'd request has its record");
    let r = refused[0];
    assert_eq!(r["transform_mode"], "passthrough", "the chain never ran");
    assert_eq!(r["identity"]["event_id"], 0, "the pre-pipeline sentinel");
    assert_eq!(r["usage_missing"], true, "priced nowhere");
    assert_eq!(r["errors"][0]["kind"], "transform_error");
    assert_eq!(r["errors"][0]["message"], v["error"]["message"]);
    assert_eq!(r["cost"]["total"], 0);

    serve_task.abort();
    let _ = serve_task.await;
}
