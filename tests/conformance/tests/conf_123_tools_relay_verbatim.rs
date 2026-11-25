//! CONF-123 (spec §4.23, ADR-066): **the tools surface relays** —
//! POST /v1/tools/{search,fetch,tts,asr} on the real serve assembly over
//! loopback HTTP.
//!
//! Limbs:
//! - (a) **relayed both ways, byte for byte** — the upstream's body is the
//!   caller's own bytes (spacing and all), the provider's key rides as
//!   authorization, the caller's content type travels as received, and the
//!   answer reaches the caller as the upstream sent it (status, content
//!   type, body).
//! - (b) **an asr multipart envelope survives** — the body and the
//!   boundary-carrying content type reach the upstream unchanged; a
//!   rewritten body would be one the upstream cannot parse.
//! - (c) **a non-JSON answer is relayed as bytes** — tts answers audio, so
//!   the relay may not assume JSON anywhere.
//! - (d) **an upstream refusal is relayed, never re-shaped** — a 422
//!   arrives at the caller with the upstream's own error body, not §8's
//!   envelope.
//! - (e) **an absent tools: section is a 501** for every verb, with no
//!   dial: the surface is opt-in.
//! - (f) **a verb without its URL is a 501** — declaring search does not
//!   promise asr.
//! - (g) **a keyless provider is not served** — a 501, with no dial.
//! - (h) **the tools bound is its own knob** — a body over
//!   tools.max_body_bytes answers 413 before the dial.
//! - (i) **the guard covers the surface** — no token, no call, 401.
//! - (j) **the record carries the tools group** — verb, provider, upstream
//!   status, both byte counts, priced: false, and the call is
//!   usage_missing with a zero cost.
//!
//! Offline: a canned loopback mock upstream only.

#![forbid(unsafe_code)]

use serde_json::Value;
use vadis_conformance::testkit::{self, CannedResponse};

const SEARCH_CANNED: &str = r#"{"results":[{"rerank_score":0.91,"title":"你好","url":"https://example.com/a","snippet":"spacing  kept"}],"note":"trailing"}"#;

const ASR_CANNED: &str = r#"{"request_id":"asr-1","text":"这句话被听见了","duration_ms":1234}"#;

// A WAV header the blocking test client can round-trip: that harness decodes
// the response as text, so the fixture stays ASCII. Fidelity of arbitrary
// bytes is the provider client's own `resp.bytes()` read, not a property this
// fixture can prove.
const AUDIO_BYTES: &[u8] = b"RIFF\x04\x00\x00\x00WAVE";

fn config_yaml(
    upstream_port: u16,
    listen_port: u16,
    key_env: &str,
    provider_tools: &str,
    tools: &str,
) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s, auth_token_env: CONF123_TOKEN }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: mock
    urls:
      chat: http://127.0.0.1:{upstream_port}/v1/chat/completions
{provider_tools}    api_key_env: {key_env}
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
{tools}"#
    )
}

fn declared(verbs: &[&str], upstream_port: u16) -> String {
    let mut lines = String::from("    tools:\n");
    for verb in verbs {
        lines.push_str(&format!(
            "      {verb}: http://127.0.0.1:{upstream_port}/v1/tools/{verb}\n"
        ));
    }
    lines
}

const TOOLS: &str = "tools: { provider: mock, max_body_bytes: 65536, attempt_timeout: 30s }\n";

fn write_config(
    dir: &std::path::Path,
    upstream_port: u16,
    listen_port: u16,
    key_env: &str,
    provider_tools: &str,
    tools: &str,
) {
    std::fs::write(
        dir.join("config.yaml"),
        config_yaml(upstream_port, listen_port, key_env, provider_tools, tools),
    )
    .unwrap();
}

fn spawn(dir: &std::path::Path, listen_addr: &str) -> tokio::task::JoinHandle<i32> {
    std::env::set_var("CONF123_MOCK_KEY", "sk-conf123");
    std::env::set_var("CONF123_TOKEN", "tok-123");
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(listen_addr);
    task
}

async fn rig(
    tag: &str,
    key_env: &str,
    verbs: &[&str],
    tools: &str,
) -> (
    std::path::PathBuf,
    String,
    testkit::MockUpstream,
    tokio::task::JoinHandle<i32>,
) {
    let dir = testkit::tempdir(tag);
    let upstream = testkit::MockUpstream::start().await.unwrap();
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    write_config(
        &dir,
        upstream.addr.port(),
        listen_port,
        key_env,
        &declared(verbs, upstream.addr.port()),
        tools,
    );
    let task = spawn(&dir, &listen_addr);
    (dir, listen_addr, upstream, task)
}

fn header<'h>(headers: &'h [(String, String)], name: &str) -> Option<&'h str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn post(
    addr: &str,
    verb: &str,
    body: &[u8],
    extra: &[(&str, &str)],
) -> (u16, Vec<u8>, Vec<(String, String)>) {
    testkit::http_post(addr, &format!("/v1/tools/{verb}"), body, extra)
}

fn error_body(body: &[u8]) -> Value {
    serde_json::from_slice(body).unwrap_or_else(|e| {
        panic!(
            "a refused call answers §8 JSON: {e}: {:?}",
            String::from_utf8_lossy(body)
        )
    })
}

fn error_type(v: &Value) -> &str {
    v.get("error")
        .and_then(|e| e.get("type"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("§8 body without error.type: {v}"))
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

/// (a) The call is relayed both ways: the caller's bytes, key and content
/// type in; the upstream's status, content type and body out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_call_is_relayed_both_ways() {
    let (_dir, addr, upstream, _task) =
        rig("conf123-relay", "CONF123_MOCK_KEY", &["search"], TOOLS).await;
    upstream.queue(
        CannedResponse::json(200, "OK", SEARCH_CANNED.as_bytes())
            .with_header("content-type", "application/json; charset=utf-8"),
    );

    let body = br#"{"query":"  spaces kept  ","count":3}"#;
    let (status, answer, headers) = post(
        &addr,
        "search",
        body,
        &[("authorization", "Bearer tok-123")],
    );
    assert_eq!(status, 200, "the upstream's status is the caller's");
    assert_eq!(
        answer,
        SEARCH_CANNED.as_bytes(),
        "the answer is relayed verbatim, never re-serialized"
    );
    assert_eq!(
        header(&headers, "content-type"),
        Some("application/json; charset=utf-8"),
        "the upstream's content type is relayed, not invented"
    );
    assert!(
        header(&headers, "x-vadis-request-id").is_some(),
        "the call keeps vadis's request id header"
    );

    let seen = upstream.requests();
    assert_eq!(seen.len(), 1, "exactly one upstream call");
    assert_eq!(seen[0].method, "POST");
    assert_eq!(
        seen[0].body, body,
        "the caller's bytes are the upstream's bytes"
    );
    assert_eq!(
        seen[0].header("authorization"),
        Some("Bearer sk-conf123"),
        "vadis's own key is what the upstream sees, never the caller's token"
    );
    assert_eq!(
        seen[0].header("content-type"),
        Some("application/json"),
        "the caller's content type travels as received"
    );
}

/// (b) The asr multipart envelope and its boundary arrive unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn asr_relays_the_multipart_envelope() {
    let (_dir, addr, upstream, _task) =
        rig("conf123-asr", "CONF123_MOCK_KEY", &["asr"], TOOLS).await;
    upstream.queue(CannedResponse::json(200, "OK", ASR_CANNED.as_bytes()));

    let boundary = "----conf123boundary";
    let content_type = format!("multipart/form-data; boundary={boundary}");
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\n",
    );
    body.extend_from_slice(b"Content-Type: audio/wav\r\n\r\n");
    body.extend_from_slice(&[0u8, 1, 2, 3, 0xff, 0xfe, b'\r', b'\n']);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let (status, answer, _h) = post(
        &addr,
        "asr",
        &body,
        &[
            ("authorization", "Bearer tok-123"),
            ("content-type", content_type.as_str()),
        ],
    );
    assert_eq!(status, 200, "the relay answers the upstream's status");
    assert_eq!(
        answer,
        ASR_CANNED.as_bytes(),
        "the answer is relayed verbatim"
    );

    let seen = upstream.requests();
    assert_eq!(seen.len(), 1, "exactly one upstream call");
    assert_eq!(
        seen[0].body, body,
        "the multipart envelope rides byte for byte: a rewritten body is one the upstream cannot parse"
    );
    assert_eq!(
        seen[0].header("content-type"),
        Some(content_type.as_str()),
        "the boundary's only home is the caller's content type, sent unchanged"
    );
}

/// (c) tts answers audio: the relay may not assume JSON.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_non_json_answer_is_relayed_as_bytes() {
    let (_dir, addr, upstream, _task) =
        rig("conf123-tts", "CONF123_MOCK_KEY", &["tts"], TOOLS).await;
    upstream.queue(
        CannedResponse::json(200, "OK", AUDIO_BYTES).with_header("content-type", "audio/wav"),
    );

    let body = r#"{"text":"你好","voice_id":"stella","format":"wav"}"#.as_bytes();
    let (status, answer, headers) =
        post(&addr, "tts", body, &[("authorization", "Bearer tok-123")]);
    assert_eq!(
        status, 200,
        "audio is a 200 with a body, not a JSON envelope"
    );
    assert_eq!(
        answer, AUDIO_BYTES,
        "the audio bytes reach the caller intact"
    );
    assert_eq!(
        header(&headers, "content-type"),
        Some("audio/wav"),
        "audio/wav is relayed as the upstream declared it"
    );
}

/// (d) An upstream refusal is relayed verbatim, never re-shaped into §8.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_upstream_refusal_is_relayed_verbatim() {
    let (_dir, addr, upstream, _task) =
        rig("conf123-refusal", "CONF123_MOCK_KEY", &["search"], TOOLS).await;
    let refusal = br#"{"error":{"message":"query too long","type":"invalid_request_error","param":"query","code":"invalid_parameter"}}"#;
    upstream.queue(
        CannedResponse::json(422, "Unprocessable Entity", refusal)
            .with_header("content-type", "application/json"),
    );

    let (status, answer, _h) = post(
        &addr,
        "search",
        br#"{"query":"x"}"#,
        &[("authorization", "Bearer tok-123")],
    );
    assert_eq!(
        status, 422,
        "the provider's status is the caller's: vadis did not decide this"
    );
    assert_eq!(
        answer, refusal,
        "the provider's error body is relayed as sent, not rebuilt as §8's envelope"
    );
    let parsed: Value = serde_json::from_slice(&answer).unwrap();
    assert_eq!(
        parsed["error"]["type"], "invalid_request_error",
        "the provider's own error vocabulary survives"
    );
}

/// (e) No tools: section, no surface: every verb is a 501 and nothing is
/// dialled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_absent_tools_section_answers_501_on_every_verb() {
    let (_dir, addr, upstream, _task) = rig("conf123-off", "CONF123_MOCK_KEY", &[], "").await;
    for verb in ["search", "fetch", "tts", "asr"] {
        let (status, body, headers) = post(
            &addr,
            verb,
            br#"{"query":"x"}"#,
            &[("authorization", "Bearer tok-123")],
        );
        assert_eq!(
            status,
            501,
            "{verb} is opt-in: {:?}",
            String::from_utf8_lossy(&body)
        );
        let parsed = error_body(&body);
        assert_eq!(error_type(&parsed), "not_implemented", "{verb}: {parsed}");
        assert!(
            parsed["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("tools:"),
            "{verb} names the missing section: {parsed}"
        );
        assert!(
            header(&headers, "x-vadis-request-id").is_some(),
            "{verb}: §8's formatter stamps the request id"
        );
    }
    assert!(
        upstream.requests().is_empty(),
        "an off surface never dials the upstream"
    );
}

/// (f) Declaring one verb does not promise another.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_verb_without_its_url_is_a_501() {
    let (_dir, addr, upstream, _task) =
        rig("conf123-partial", "CONF123_MOCK_KEY", &["search"], TOOLS).await;
    let (status, body, _h) = post(
        &addr,
        "asr",
        b"junk",
        &[
            ("authorization", "Bearer tok-123"),
            ("content-type", "multipart/form-data; boundary=x"),
        ],
    );
    assert_eq!(
        status,
        501,
        "asr was not declared: {:?}",
        String::from_utf8_lossy(&body)
    );
    let parsed = error_body(&body);
    assert_eq!(error_type(&parsed), "not_implemented", "{parsed}");
    assert!(
        parsed["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("tools.asr"),
        "the refusal names the missing verb URL: {parsed}"
    );
    assert!(upstream.requests().is_empty(), "no URL, no dial");
}

/// (g) A provider without a readable key is not served.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_keyless_provider_is_not_served() {
    let (_dir, addr, upstream, _task) =
        rig("conf123-keyless", "CONF123_MISSING_KEY", &["search"], TOOLS).await;
    let (status, body, _h) = post(
        &addr,
        "search",
        br#"{"query":"x"}"#,
        &[("authorization", "Bearer tok-123")],
    );
    assert_eq!(
        status,
        501,
        "no credential, no surface: {:?}",
        String::from_utf8_lossy(&body)
    );
    let parsed = error_body(&body);
    assert_eq!(error_type(&parsed), "not_implemented", "{parsed}");
    assert!(
        parsed["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("credential"),
        "the refusal names the missing credential: {parsed}"
    );
    assert!(upstream.requests().is_empty(), "no key, no dial");
}

/// (h) The tools bound is its own knob: over it a 413 before the dial,
/// under it the same call is served.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tools_body_bound_is_its_own_knob() {
    let tools = "tools: { provider: mock, max_body_bytes: 4096, attempt_timeout: 30s }\n";
    let (_dir, addr, upstream, _task) =
        rig("conf123-big", "CONF123_MOCK_KEY", &["search"], tools).await;
    upstream.queue(CannedResponse::json(200, "OK", SEARCH_CANNED.as_bytes()));

    let big = format!(r#"{{"query":"{}"}}"#, "A".repeat(8192));
    let (status, body, _h) = post(
        &addr,
        "search",
        big.as_bytes(),
        &[("authorization", "Bearer tok-123")],
    );
    assert_eq!(
        status,
        413,
        "over tools.max_body_bytes: {:?}",
        String::from_utf8_lossy(&body)
    );
    let parsed = error_body(&body);
    assert_eq!(error_type(&parsed), "request_too_large", "{parsed}");
    assert_eq!(
        parsed["error"]["details"]["bound"], "tools.max_body_bytes",
        "the refusal names the knob it enforced: {parsed}"
    );
    assert!(
        upstream.requests().is_empty(),
        "an oversize body is refused before any upstream call"
    );

    let (status, _body, _h) = post(
        &addr,
        "search",
        br#"{"query":"x"}"#,
        &[("authorization", "Bearer tok-123")],
    );
    assert_eq!(status, 200, "under the bound the call is served");
    assert_eq!(upstream.requests().len(), 1);
}

/// (i) The guard covers the surface: no token, no call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tools_surface_is_guarded() {
    let (_dir, addr, upstream, _task) =
        rig("conf123-guard", "CONF123_MOCK_KEY", &["search"], TOOLS).await;
    let (status, body, _h) = post(&addr, "search", br#"{"query":"x"}"#, &[]);
    assert_eq!(
        status,
        401,
        "the guard runs before the handler: {:?}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(error_type(&error_body(&body)), "unauthorized");
    assert!(upstream.requests().is_empty(), "a refused call never dials");
}

/// (j) The record carries the tools group, and the call is unpriced — never
/// read as free.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_record_carries_the_tools_group() {
    let (dir, addr, upstream, task) =
        rig("conf123-record", "CONF123_MOCK_KEY", &["search"], TOOLS).await;
    upstream.queue(CannedResponse::json(200, "OK", SEARCH_CANNED.as_bytes()));
    let body = br#"{"query":"x","count":1}"#;
    let (status, _answer, _h) = post(
        &addr,
        "search",
        body,
        &[("authorization", "Bearer tok-123")],
    );
    assert_eq!(status, 200);

    task.abort();
    let _ = task.await;
    let records = read_records(&dir.join("state/traces"));
    assert_eq!(records.len(), 1, "exactly one record for the call");
    let rec = &records[0];

    assert_eq!(
        rec["protocol"]["protocol_in"], "tools",
        "the surface is its own protocol word: {rec}"
    );
    let tools = rec
        .get("tools")
        .unwrap_or_else(|| panic!("the tools group: {rec}"));
    assert_eq!(tools["verb"], "search");
    assert_eq!(tools["provider"], "mock");
    assert_eq!(tools["upstream_status"], 200);
    assert_eq!(tools["request_bytes"], body.len());
    assert_eq!(tools["response_bytes"], SEARCH_CANNED.len());
    assert_eq!(tools["priced"], false, "v0.1 has no tool price table");
    assert_eq!(rec["result"]["status"], 200);
    assert_eq!(rec["usage_missing"], true, "a tool answer reports no usage");
    assert_eq!(
        rec["cost"]["total"], 0,
        "zero cost, and the priced flag says why"
    );
    assert!(
        rec.get("cache").is_none(),
        "a tool call is never a cache hit"
    );
}
