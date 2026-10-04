//! CONF-13 (DESIGN §10 conformance · SSE): native streaming passes
//! through **event-by-event byte-identically** — no re-framing, no
//! reordering, no dropped terminal events — proven over real loopback
//! HTTP against a mock SSE upstream, plus the disconnect and mid-stream
//! failure semantics of DESIGN §12.10.3 (requirements R5/R6).

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, ReadOpts, SseChunk};

/// The exact byte sequence the mock upstream emits, with awkward content:
/// braces, quotes, multi-byte UTF-8, an id/retry pair, and the chat
/// terminal marker. The client must receive these bytes **concatenated
/// verbatim** — comparing anything other than the raw byte sequence
/// (counts, parsed JSON) would hide re-framing.
const UPSTREAM_SSE: &[&str] = &[
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"héllo {braces} \\\"q\\\"\"}}]}\n\n",
    ": keep-alive comment\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"wörld 😀\"},\"id\":\"abc\"}]}\n\n",
    "id: evt-7\nretry: 250\ndata: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
    "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":14409,\"completion_tokens\":111,\"total_tokens\":14520,\"prompt_tokens_details\":{\"cached_tokens\":14400}}}\n\n",
    "data: [DONE]\n\n",
];

fn expected_bytes() -> Vec<u8> {
    UPSTREAM_SSE.concat().into_bytes()
}

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
    api_key_env: CONF13_MOCK_KEY
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

const CLIENT_BODY: &str = r#"{"model":"mock/glm","messages":[{"role":"user","content":"stream me"}],"stream":true,"stream_options":{"include_usage":true}}"#;

fn start_vadis(config_path: &std::path::Path, listen_port: u16) -> tokio::task::JoinHandle<i32> {
    std::env::set_var("CONF13_MOCK_KEY", "sk-conf13");
    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&format!("127.0.0.1:{listen_port}"));
    serve_task
}

/// (a) The main byte-fidelity case: the client's received SSE byte
/// sequence equals the upstream's, event for event, including the
/// keep-alive comment, the `id:`/`retry:` pair and the terminal `[DONE]`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_13_a_event_by_event_byte_equivalence() {
    let dir = testkit::tempdir("conf13a");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    upstream.queue(testkit::CannedResponse::sse(
        UPSTREAM_SSE
            .iter()
            .map(|e| SseChunk::event(e.as_bytes()))
            .collect(),
    ));

    let listen_port = testkit::free_port();
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(upstream.addr.port(), listen_port)).unwrap();
    let serve_task = start_vadis(&config_path, listen_port);

    let (status, body, headers) = testkit::http_post(
        &format!("127.0.0.1:{listen_port}"),
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[],
    );

    assert_eq!(status, 200, "vadis status");
    let ct = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    assert!(ct.starts_with("text/event-stream"), "content-type: {ct}");

    // Dechunk the transport framing, then compare the *event byte
    // sequence* — not chunk framing, which the vadis may legitimately
    // re-frame at the HTTP layer, but never the SSE event bytes.
    let events = testkit::dechunk(&body);

    // Byte-for-byte equality of the whole sequence...
    assert_eq!(
        events,
        expected_bytes(),
        "client-received SSE bytes must equal the upstream's verbatim"
    );
    // ...and event-by-event equality (no merged/split/reordered events).
    let got = testkit::sse_events(&events);
    let want = testkit::sse_events(&expected_bytes());
    assert_eq!(got.len(), want.len(), "same event count");
    for (i, (g, w)) in got.iter().zip(want.iter()).enumerate() {
        assert_eq!(g, w, "event {i} must be byte-identical");
    }
    // The terminal marker survived: no dropped end-event.
    assert!(
        events.ends_with(b"data: [DONE]\n\n"),
        "the upstream's terminal [DONE] must be the last event relayed"
    );
    // Nothing was appended after the upstream's end.
    assert_eq!(events.len(), expected_bytes().len());

    // The upstream saw exactly one request with the byte-faithful body
    // (stream: true travels verbatim; router_meta removal is CONF-10's
    // concern, asserted there).
    let requests = upstream.requests();
    assert_eq!(requests.len(), 1, "exactly one upstream attempt");
    let body_str = String::from_utf8_lossy(&requests[0].body).into_owned();
    assert!(body_str.contains("\"stream\":true"));

    serve_task.abort();
    let _ = serve_task.await;
}

/// (b) R6 after-first-byte: the upstream aborts mid-stream after one
/// relayed event. The relay must truncate — the client receives the
/// first event and **no** `[DONE]`, and the upstream sees exactly one
/// attempt (no silent retry).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_13_b_midstream_failure_truncates_never_retries() {
    let dir = testkit::tempdir("conf13b");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    upstream.queue(testkit::CannedResponse::sse(vec![
        SseChunk::event(UPSTREAM_SSE[0].as_bytes()),
        // Gap, then abrupt close: no further events, no terminal marker.
        SseChunk::abort_after(50),
    ]));

    let listen_port = testkit::free_port();
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(upstream.addr.port(), listen_port)).unwrap();
    let serve_task = start_vadis(&config_path, listen_port);

    let (status, body, _headers) = testkit::http_post(
        &format!("127.0.0.1:{listen_port}"),
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[],
    );

    assert_eq!(status, 200, "the head was already committed");
    let events = testkit::dechunk(&body);
    // Exactly the first event arrived, byte-identical...
    assert_eq!(
        events.as_slice(),
        UPSTREAM_SSE[0].as_bytes(),
        "the relayed prefix must be the upstream's own bytes"
    );
    // ...and the truncation is honest: no [DONE] was fabricated.
    assert!(!events.ends_with(b"data: [DONE]\n\n"));
    // No retry: one attempt only.
    let requests = upstream.requests();
    assert_eq!(
        requests.len(),
        1,
        "a truncated stream must never be retried"
    );

    serve_task.abort();
    let _ = serve_task.await;
}

/// (c) §12.10.3 R5: a client that disconnects mid-stream cancels the upstream —
/// the mock sees its connection drop before the remaining chunks are
/// written. Observable form: the connection-close count rises while the
/// request count stays one, and (deterministically) fewer than all
/// events were consumed by anyone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_13_c_client_disconnect_cancels_upstream() {
    let dir = testkit::tempdir("conf13c");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    // Slow stream: the client bails after the first event's bytes; the
    // upstream would otherwise keep writing for ~1.5s more.
    let mut chunks = vec![SseChunk::event(UPSTREAM_SSE[0].as_bytes())];
    for e in &UPSTREAM_SSE[1..] {
        chunks.push(SseChunk::event_after(e.as_bytes(), 300));
    }
    upstream.queue(testkit::CannedResponse::sse(chunks));

    let listen_port = testkit::free_port();
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(upstream.addr.port(), listen_port)).unwrap();
    let serve_task = start_vadis(&config_path, listen_port);

    let first_event_len = UPSTREAM_SSE[0].len();
    let (status, body, _headers) = testkit::http_post_with_opts(
        &format!("127.0.0.1:{listen_port}"),
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[],
        ReadOpts {
            stop_after_body_bytes: Some(first_event_len),
        },
    );

    assert_eq!(status, 200);
    let events = testkit::dechunk(&body);
    assert!(
        !events.is_empty() && events.len() <= first_event_len,
        "the client read the first event's bytes then dropped"
    );
    // The upstream's write side saw the cancellation: its peer (the
    // vadis) closed within the stream window, so fewer chunks were
    // written than queued. Give the propagation a bounded wait.
    let disconnected = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if upstream.peer_aborts() >= 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(
        disconnected.is_ok(),
        "client disconnect must propagate: the upstream connection must be dropped, not drained"
    );
    assert_eq!(upstream.requests().len(), 1);

    serve_task.abort();
    let _ = serve_task.await;
}

/// (d) The zero-byte branch of R6: the upstream fails before the first
/// body byte. Nothing observable left the vadis, so a fallback route may
/// answer; here the chain is empty, so the client gets the §8 error body
/// (not a half-relay) — and still exactly one upstream attempt.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_13_d_pre_relay_failure_is_an_error_not_a_half_relay() {
    let dir = testkit::tempdir("conf13d");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    // A non-SSE error head: R6 column 1, the ordinary error path.
    upstream.queue(testkit::CannedResponse::json(
        500,
        "Internal Server Error",
        b"{\"error\":{\"message\":\"boom\"}}",
    ));

    let listen_port = testkit::free_port();
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(upstream.addr.port(), listen_port)).unwrap();
    let serve_task = start_vadis(&config_path, listen_port);

    let (status, body, _headers) = testkit::http_post(
        &format!("127.0.0.1:{listen_port}"),
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[],
    );

    assert_eq!(status, 502, "a failure head maps to the §8 error body");
    let text = String::from_utf8_lossy(&body).into_owned();
    assert!(text.contains("\"error\""), "unified error body: {text}");
    assert!(
        !text.contains("[DONE]"),
        "no terminal marker may be fabricated on the error path"
    );
    assert_eq!(upstream.requests().len(), 1);

    serve_task.abort();
    let _ = serve_task.await;
}
