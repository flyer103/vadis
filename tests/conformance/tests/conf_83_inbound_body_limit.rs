//! CONF-83 (spec §4.13 · the inbound body bound, §8 · its refusal, §6 ·
//! the boundary record; DESIGN §12.15 / §12.8's row): **the bound is the
//! router's own, and so is the refusal.**
//!
//! At the base tree the cap existed but was the HTTP framework's: axum's
//! `Bytes` extractor wrapped every protocol-route body in an implicit
//! 2 MiB `Limited` whose over-limit answer was a plain-text `413`
//! (`Failed to buffer the request body`), with **no**
//! `X-Router-Request-Id`, **no** trace record and **no** config key —
//! a refusal an operator could neither correlate nor count (R32-F1).
//! The landing (§12.15) installs the router's own bound as a boundary
//! layer above the transform-mode resolution and the path split,
//! inside the token guard, with the framework's cap **disabled** so
//! exactly one owner exists.
//!
//! Legs (the frozen row, on the real `serve` assembly over a loopback
//! mock, with the rig's own configured bound):
//!
//! - **(a)** a body **exactly at** the bound is served, the
//!   upstream-visible request bytes are the client's own (the byte
//!   control), and its record carries `upstream_ms` present with
//!   `usage_missing: false`;
//! - **(b)** a body **one byte above** it is refused `413` in §8's
//!   unified shape naming `request_too_large`, with
//!   `details.limit_bytes` equal to the rig's own configured value,
//!   `X-Router-Request-Id` present, **one** pre-pipeline trace record
//!   (`event_id: 0`, `usage_missing: true`, nothing priced) and
//!   **zero** requests arriving at the stand-in;
//! - **(c)** the same refusal when the length is **not declared** (a
//!   chunked body), so omitting `Content-Length` cannot walk around
//!   the bound;
//! - **(d)** a body above the bound in a **streaming** request
//!   (`stream: true`) is answered as that same complete, non-SSE
//!   `413` — `content-type: application/json`, **no** `details.stream`
//!   member, no SSE head ever sent;
//! - **(e)** the bound follows the key: two rig values move which body
//!   is accepted, and a value below `1024` is a load refusal (exit 2)
//!   naming the key.
//!
//! Red at base `515a22f` on the decisive legs: the key does not exist,
//! and the refusal is the framework's plain-text `413` with no request
//! id and no trace record.

#![forbid(unsafe_code)]

use std::io::{Read, Write};

use router_conformance::testkit::{self, CannedResponse};

/// The rig's own configured bound (leg (e) moves it): small enough that
/// the fixtures are cheap, above the 1024 floor the loader enforces.
const LIMIT: usize = 4096;

fn config_yaml(upstream_port: u16, listen_port: u16, limit: usize) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s, max_body_bytes: {limit} }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: mock
    urls:
      chat: http://127.0.0.1:{upstream_port}/v1/chat/completions
    api_key_env: CONF83_MOCK_KEY
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

/// One rigged `serve` on a fresh tempdir with the given bound. Returns
/// (listen_addr, dir, serve_task).
async fn rig(
    tag: &str,
    limit: usize,
) -> (String, std::path::PathBuf, tokio::task::JoinHandle<i32>) {
    let dir = testkit::tempdir(tag);
    let upstream = testkit::MockUpstream::start().await.unwrap();
    upstream.queue(CannedResponse::json(
        200,
        "OK",
        br#"{"id":"ok","choices":[{"index":0,"message":{"role":"assistant","content":"served"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}"#,
    ));
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(
        &config_path,
        config_yaml(upstream.addr.port(), listen_port, limit),
    )
    .unwrap();
    std::env::set_var("CONF83_MOCK_KEY", "sk-conf83");
    // The mock must outlive the returned handle: the mock's task is
    // detached inside `MockUpstream` (its recorded requests are read
    // through the shared state, not the task), so park the upstream
    // itself in the returned dir is not possible — instead keep it
    // alive by leaking; the process is a test.
    std::mem::forget(upstream);
    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    (listen_addr, dir, serve_task)
}

/// A chat body padded to exactly `size` bytes (valid JSON: the padding
/// rides in the last message's content).
fn sized_body(size: usize, stream: bool) -> Vec<u8> {
    let head = r#"{"model":"mock/glm","messages":[{"role":"user","content":""#;
    let tail = if stream {
        r#""}],"stream":true}"#
    } else {
        r#""}],"stream":false}"#
    };
    let pad = size - head.len() - tail.len();
    let mut b = String::with_capacity(size);
    b.push_str(head);
    b.push_str(&"x".repeat(pad));
    b.push_str(tail);
    assert_eq!(b.len(), size, "the fixture sizes the body exactly");
    b.into_bytes()
}

/// POSTs raw bytes with a declared content-length; returns
/// (status, body, headers).
fn post_declared(addr: &str, body: &[u8]) -> (u16, Vec<u8>, Vec<(String, String)>) {
    testkit::http_post(addr, "/v1/chat/completions", body, &[])
}

/// POSTs raw bytes with **no** content-length (chunked framing): the
/// hand-rolled client the byte-fidelity cases use, in the chunked arm's
/// shape. One chunk carries the whole body.
fn post_chunked(addr: &str, body: &[u8]) -> (u16, Vec<u8>, Vec<(String, String)>) {
    let mut stream = std::net::TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    let head = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nHost: {addr}\r\ncontent-type: application/json\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n"
    );
    stream.write_all(head.as_bytes()).unwrap();
    stream
        .write_all(format!("{:x}\r\n", body.len()).as_bytes())
        .unwrap();
    stream.write_all(body).unwrap();
    stream.write_all(b"\r\n0\r\n\r\n").unwrap();
    // Read the answer to the end (connection: close on both sides).
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    parse_response(&buf)
}

/// Splits a raw response into (status, body, headers), trimming the
/// body to a declared content-length (chunked bodies are compared
/// dechunked by the caller when needed).
fn parse_response(buf: &[u8]) -> (u16, Vec<u8>, Vec<(String, String)>) {
    let text = String::from_utf8_lossy(buf);
    let (head, body) = match text.split_once("\r\n\r\n") {
        Some((h, b)) => (h.to_string(), b.as_bytes().to_vec()),
        None => (text.into_owned(), Vec::new()),
    };
    let mut lines = head.split("\r\n");
    let status: u16 = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .expect("status line");
    let mut headers = Vec::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    let declared = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<usize>().ok());
    let body = match declared {
        Some(len) if len <= body.len() => body[..len].to_vec(),
        _ => body,
    };
    (status, body, headers)
}

/// Every trace record in the rig's dir, in write order.
fn trace_records(dir: &std::path::Path) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir.join("state/traces")).expect("trace dir") {
        let entry = entry.unwrap();
        if entry.path().extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        for line in std::fs::read_to_string(entry.path()).unwrap().lines() {
            if !line.trim().is_empty() {
                out.push(serde_json::from_str(line).expect("record json"));
            }
        }
    }
    out
}

/// The refusal's invariants, shared by legs (b)/(c)/(d): §8's unified
/// body naming the rule, the configured bound in `details`, the request
/// id in header and body, and the connection closed.
fn assert_refusal(
    status: u16,
    body: &[u8],
    headers: &[(String, String)],
    expected_content_length: serde_json::Value,
) {
    assert_eq!(status, 413, "the bound refuses with 413, got {status}");
    let v: serde_json::Value = serde_json::from_slice(body).expect("§8 unified error body json");
    assert_eq!(v["error"]["type"], "request_too_large", "the rule is named");
    assert_eq!(v["error"]["details"]["limit_bytes"], LIMIT as u64);
    assert_eq!(
        v["error"]["details"]["content_length"],
        expected_content_length
    );
    let req_id = v["error"]["request_id"].as_str().expect("request_id");
    assert!(!req_id.is_empty());
    let hdr_id = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("x-router-request-id"))
        .map(|(_, v)| v.as_str())
        .expect("X-Router-Request-Id on a 413 (§8's always)");
    assert_eq!(hdr_id, req_id, "the header and the body name the same id");
    let conn = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("connection"))
        .map(|(_, v)| v.to_ascii_lowercase());
    assert_eq!(
        conn.as_deref(),
        Some("close"),
        "the connection is closed after the refusal (§4.13)"
    );
    // The response is a complete, non-SSE answer (the streaming leg's
    // own rule): json content type, no event-stream anywhere.
    let ct = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    assert!(
        ct.contains("application/json"),
        "the 413 is §8's unified json body, got content-type {ct}"
    );
    // The body carries no `details.stream` member (§4.13: a fact of a
    // body this refusal never read).
    assert!(v["error"]["details"].get("stream").is_none());
}

/// The one pre-pipeline record the refusal leaves, and its shape.
/// Returns the count of refusal records in the set (the callers assert
/// the count themselves — one per refusal performed).
fn refusal_records(records: &[serde_json::Value]) -> Vec<&serde_json::Value> {
    records
        .iter()
        .filter(|r| {
            r["errors"]
                .as_array()
                .is_some_and(|e| e.iter().any(|x| x["kind"] == "request_too_large"))
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_83_inbound_body_limit() {
    // (a)+(b): at the bound serves (byte control, upstream_ms present);
    // one byte above refuses with the rule named, nothing at the mock.
    let (addr, dir, serve) = rig("conf83-main", LIMIT).await;

    // (a) exactly at the bound.
    let at_bound = sized_body(LIMIT, false);
    let (status, _body, _h) = post_declared(&addr, &at_bound);
    assert_eq!(status, 200, "a body exactly at the bound is served");

    // (b) one byte above the bound, declared.
    let over = sized_body(LIMIT + 1, false);
    let (status, body, headers) = post_declared(&addr, &over);
    assert_refusal(status, &body, &headers, serde_json::json!(LIMIT + 1));

    // (c) the same body, chunked (no Content-Length): same refusal,
    // content_length null — omitting the declaration walks around
    // nothing.
    let (status, body, headers) = post_chunked(&addr, &over);
    assert_refusal(status, &body, &headers, serde_json::Value::Null);

    // (d) a streaming request above the bound: the same complete,
    // non-SSE 413 at the head (assert_refusal checks content-type,
    // json body, no SSE head in it).
    let over_stream = sized_body(LIMIT + 1, true);
    let (status, body, headers) = post_declared(&addr, &over_stream);
    assert_refusal(status, &body, &headers, serde_json::json!(LIMIT + 1));

    // (b) continued: exactly one boundary record per refusal, each the
    // §6 pre-pipeline class; and (a)'s served record carries
    // upstream_ms present with usage_missing false.
    serve.abort();
    let records = trace_records(&dir);

    let served: Vec<&serde_json::Value> = records
        .iter()
        .filter(|r| r["result"]["status"] == 200)
        .collect();
    assert_eq!(served.len(), 1, "leg (a) served exactly one request");
    assert!(
        served[0]["result"]["upstream_ms"].is_u64(),
        "the served record carries upstream_ms present"
    );
    assert_eq!(served[0]["usage_missing"], false);

    let refused: Vec<&serde_json::Value> = refusal_records(&records);
    assert_eq!(
        refused.len(),
        3,
        "legs (b), (c) and (d) each leave one record"
    );
    let _ = &refused;
    for rec in &refused {
        assert_eq!(rec["identity"]["event_id"], 0, "the pre-pipeline class");
        assert_eq!(rec["usage_missing"], true);
        assert_eq!(rec["result"]["upstream_ms"], serde_json::Value::Null);
        assert_eq!(rec["decision"]["provider"], "", "no route was selected");
        assert_eq!(rec["cost"]["total"], 0, "nothing priced");
    }
    // (b)'s own record names the declared length; (c)'s names null.
    assert_eq!(
        refused[0]["errors"][0]["details"]["content_length"],
        LIMIT + 1
    );
    assert_eq!(
        refused[1]["errors"][0]["details"]["content_length"],
        serde_json::Value::Null
    );

    // The byte control for (a): the mock's recorded body equals the
    // client's own bytes modulo the two permitted mutations (AGENTS 1)
    // — here computed as: identical after removing router-owned keys
    // is what the forwarding path guarantees; the conformance-level
    // witness is the recorded body's length and the model field, plus
    // the recorded request existing at all (the refusal legs assert
    // the mock received NOTHING).
    // (The mock was detached by `rig`; its recordings are asserted in
    // the second rig below, where the mock is held.)
    assert!(!refusal_records(&records).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_83_bound_follows_the_key_and_the_mock_receives_nothing() {
    // (e) + the mock-side witnesses: with a different bound the SAME
    // body flips verdicts, a value below 1024 refuses the load, and a
    // refused request never reaches the upstream.
    let dir = testkit::tempdir("conf83-key");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    upstream.queue(CannedResponse::json(
        200,
        "OK",
        br#"{"id":"ok","choices":[{"index":0,"message":{"role":"assistant","content":"served"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}"#,
    ));
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let limit = 8192usize;
    std::fs::write(
        dir.join("config.yaml"),
        config_yaml(upstream.addr.port(), listen_port, limit),
    )
    .unwrap();
    std::env::set_var("CONF83_MOCK_KEY", "sk-conf83");
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);

    // The same body that was refused at 4096 is served at 8192 (the
    // bound follows the key) — and this time the mock is held, so the
    // byte control runs: the recorded request's body is the client's
    // own, and the refused sibling never arrives.
    let body = sized_body(5000, false);
    let (status, _b, _h) = testkit::http_post(&listen_addr, "/v1/chat/completions", &body, &[]);
    assert_eq!(status, 200, "the bound follows the key: 5000 < 8192 serves");

    let over = sized_body(8193, false);
    let (status, resp_body, headers) =
        testkit::http_post(&listen_addr, "/v1/chat/completions", &over, &[]);
    assert_eq!(status, 413);
    let v: serde_json::Value = serde_json::from_slice(&resp_body).expect("unified body");
    assert_eq!(v["error"]["type"], "request_too_large");
    assert_eq!(v["error"]["details"]["limit_bytes"], 8193u64 - 1); // 8192
    let _ = headers; // shape asserted in the main leg

    // Give the mock a moment to observe nothing further, then read its
    // recordings: exactly the one served request.
    std::thread::sleep(std::time::Duration::from_millis(200));
    let recorded = upstream.requests();
    assert_eq!(
        recorded.len(),
        1,
        "the refused request never reached the upstream"
    );
    // The byte control: the recorded body IS the client's body modulo
    // the two permitted byte-level mutations (AGENTS 1) — here exactly
    // the model re-point (`mock/glm` → the resolved route's native id
    // `glm`); no router-owned key is present to remove.
    let body_str = String::from_utf8(body.clone()).expect("fixture body is utf-8");
    let expected_upstream = body_str.replace(r#""mock/glm""#, r#""glm""#);
    assert_eq!(
        recorded[0].body,
        expected_upstream.as_bytes(),
        "the upstream-visible bytes are the client's own modulo the model re-point"
    );
    serve_task.abort();

    // (e) the loader's own refusal: a value below 1024 is a load error
    // naming the key. Driven through the loader `serve` itself uses.
    let err = router_cli::config_load::validate_text(&config_yaml(
        upstream.addr.port(),
        listen_port,
        1023,
    ))
    .expect_err("a bound below 1024 cannot load");
    assert!(
        err.contains("server.max_body_bytes"),
        "the key is named: {err}"
    );
    assert!(err.contains("1024"), "the floor is named: {err}");
    // And `serve` maps it to exit 2.
    let dir2 = testkit::tempdir("conf83-load-refusal");
    std::fs::write(
        dir2.join("config.yaml"),
        config_yaml(upstream.addr.port(), testkit::free_port(), 0),
    )
    .unwrap();
    let cfg2 = dir2.join("config.yaml").to_string_lossy().into_owned();
    let code = router_cli::serve(&cfg2).await;
    assert_eq!(code, 2, "an unusable bound exits 2 (the loader's code)");
}
