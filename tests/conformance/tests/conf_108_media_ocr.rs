//! CONF-108 (spec §4.22, ADR-065): **the media tool surface** —
//! `POST /v1/tools/{ocr,transcribe,synthesize}` on the real `serve`
//! assembly over loopback HTTP.
//!
//! Limbs:
//! - (a) **OCR is served** — a `media:` section names the route; the call
//!   answers `200` with the tool's own `{"text"}` shape (never the
//!   upstream's chat envelope), and the upstream-visible request is the
//!   *synthesized* body: `model` = `media.model`, `stream` false, one
//!   text part carrying the caller's prompt and one `image_url` part
//!   carrying the file as a `data:` URI. The caller's prompt wins over
//!   `media.prompt`.
//! - (b) **no `media:` ⇒ 501** for every one of the three paths, with §8's
//!   body and no upstream dial (the tools are opt-in).
//! - (c) **audio needs its own prerequisite** — with `media:` written but no
//!   `tools:` block, `transcribe` and `synthesize` answer `501
//!   not_implemented`: the arms dial the provider's own endpoints through
//!   `tools:` (spec §4.22.9), and a fabricated answer would be a lie the
//!   caller's model reads next.
//! - (d) **an unusable body is a `400`** naming the field, never a
//!   best-effort guess.
//! - (e) **the media body bound** — a request over `media.max_body_bytes`
//!   answers `413` even though the protocol routes' bound is larger;
//!   the two bounds are separate on purpose (a 5 MiB photo is ~7 MiB of
//!   base64 JSON).
//! - (f) **the guard covers the surface** — no token ⇒ `401` and no
//!   upstream dial.
//!
//! Offline: a canned loopback mock upstream only.

#![forbid(unsafe_code)]

use serde_json::{json, Value};
use vadis_conformance::testkit::{self, CannedResponse};

/// The upstream's canned answer: a chat completion whose content is a
/// string (the shape OCR reads as the transcript).
const CANNED: &str = r#"{"id":"ok","choices":[{"index":0,"message":{"role":"assistant","content":"图中写着：你好世界"},"finish_reason":"stop"}],"usage":{"prompt_tokens":12,"completion_tokens":4,"total_tokens":16}}"#;

/// The image the caller sends: a 1x1-ish JPEG prefix, base64 in the tool
/// shape (the surface never decodes it — the bytes ride to the model).
const IMAGE_B64: &str = "/9j/4AAQSkZJRgABAQEAYABgAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAACf/EABQQAQAAAAAAAAAAAAAAAAAAAAD/2gAIAQEAAD8AKp//2Q==";

const MEDIA: &str =
    "media: { model: \"mock/glm\", max_body_bytes: 8388608, prompt: \"describe it\" }\n";

fn config_yaml(upstream_port: u16, listen_port: u16, token_env: &str, media: &str) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s, auth_token_env: {token_env} }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: mock
    urls:
      chat: http://127.0.0.1:{upstream_port}/v1/chat/completions
    api_key_env: CONF108_MOCK_KEY
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
{media}"#
    )
}

/// One rig: mock upstream + tempdir + config + the real serve spawned.
/// Returns (dir, listen_addr, upstream, serve_task).
async fn rig(
    tag: &str,
    media: &str,
    token: &str,
) -> (
    std::path::PathBuf,
    String,
    testkit::MockUpstream,
    tokio::task::JoinHandle<i32>,
) {
    let dir = testkit::tempdir(tag);
    let upstream = testkit::MockUpstream::start().await.unwrap();
    upstream.queue(CannedResponse::json(200, "OK", CANNED.as_bytes()));
    upstream.queue(CannedResponse::json(200, "OK", CANNED.as_bytes()));
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let token_env = "CONF108_TOKEN";
    std::fs::write(
        dir.join("config.yaml"),
        config_yaml(upstream.addr.port(), listen_port, token_env, media),
    )
    .unwrap();
    std::env::set_var("CONF108_MOCK_KEY", "sk-conf108");
    std::env::set_var(token_env, token);
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    (dir, listen_addr, upstream, serve_task)
}

fn header<'h>(headers: &'h [(String, String)], name: &str) -> Option<&'h str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// POSTs a tool call with the guard token; returns (status, json, headers).
fn post_tool(
    addr: &str,
    tool: &str,
    body: &Value,
    token: Option<&str>,
) -> (u16, Value, Vec<(String, String)>) {
    let raw = serde_json::to_vec(body).unwrap();
    let mut extra: Vec<(&str, &str)> = Vec::new();
    let auth;
    if let Some(t) = token {
        auth = format!("Bearer {t}");
        extra.push(("authorization", auth.as_str()));
    }
    let (status, bytes, headers) =
        testkit::http_post(addr, &format!("/v1/tools/{tool}"), &raw, &extra);
    let parsed = serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        panic!(
            "{tool} answered non-JSON ({status}): {e}: {:?}",
            String::from_utf8_lossy(&bytes)
        )
    });
    (status, parsed, headers)
}

fn error_type(v: &Value) -> &str {
    v.get("error")
        .and_then(|e| e.get("type"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("§8 body without error.type: {v}"))
}

/// (a) OCR end to end: the tool answer, the synthesized upstream body, and
/// the caller-prompt precedence over `media.prompt`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ocr_answers_text_and_forwards_the_synthesized_chat_body() {
    let (_dir, addr, upstream, _task) = rig("conf108-ocr", MEDIA, "tok-108").await;
    let caller_prompt = "这张图里写了什么？";
    let (status, body, _h) = post_tool(
        &addr,
        "ocr",
        &json!({
            "image": { "media_type": "image/jpeg", "base64": IMAGE_B64 },
            "prompt": caller_prompt
        }),
        Some("tok-108"),
    );
    assert_eq!(status, 200, "the tool answers the verb: {body}");
    assert_eq!(
        body,
        json!({ "text": "图中写着：你好世界" }),
        "the answer is the tool's own shape, not the upstream chat envelope"
    );

    let seen = upstream.requests();
    assert_eq!(seen.len(), 1, "exactly one upstream call");
    let outbound: Value =
        serde_json::from_slice(&seen[0].body).expect("the forwarded body is JSON");
    assert_eq!(
        outbound["model"], "glm",
        "the route is media.model (the provider-native id after the one permitted mutation)"
    );
    assert_eq!(outbound["stream"], false, "a tool answer is never streamed");
    let content = outbound["messages"][0]["content"]
        .as_array()
        .expect("content parts");
    assert_eq!(
        content.len(),
        2,
        "one text part and one image part: {content:?}"
    );
    assert_eq!(content[0]["type"], "text");
    assert_eq!(
        content[0]["text"], caller_prompt,
        "the caller's prompt wins over media.prompt"
    );
    assert_eq!(content[1]["type"], "image_url");
    assert_eq!(
        content[1]["image_url"]["url"],
        format!("data:image/jpeg;base64,{IMAGE_B64}"),
        "the file rides as a data: URI carrying the caller's media type"
    );
}

/// (b) No `media:` section ⇒ every path answers §8's `501`, and nothing
/// is dialled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_absent_media_section_answers_501_on_every_path() {
    let (_dir, addr, upstream, _task) = rig("conf108-off", "", "tok-108").await;
    for tool in ["ocr", "transcribe", "synthesize"] {
        let (status, body, h) = post_tool(
            &addr,
            tool,
            &json!({ "image": { "media_type": "image/jpeg", "base64": IMAGE_B64 } }),
            Some("tok-108"),
        );
        assert_eq!(status, 501, "{tool} is opt-in: {body}");
        assert_eq!(error_type(&body), "not_implemented", "{tool}: {body}");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("media:"),
            "{tool} names the missing section: {body}"
        );
        assert!(
            header(&h, "x-vadis-request-id").is_some(),
            "{tool}: §8's formatter stamps the request id"
        );
    }
    assert!(
        upstream.requests().is_empty(),
        "an off surface never dials the upstream"
    );
}

/// (c) Audio verbs need a `tools:` block too — `media:` alone is not enough.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_audio_verbs_answer_501_without_a_tools_section() {
    let (_dir, addr, upstream, _task) = rig("conf108-audio", MEDIA, "tok-108").await;
    for tool in ["transcribe", "synthesize"] {
        let (status, body, _h) = post_tool(
            &addr,
            tool,
            &json!({ "audio": { "media_type": "audio/wav", "base64": "AAAA" } }),
            Some("tok-108"),
        );
        assert_eq!(status, 501, "{tool} has no tools: block: {body}");
        assert_eq!(error_type(&body), "not_implemented", "{tool}: {body}");
    }
    assert!(
        upstream.requests().is_empty(),
        "a reserved verb never dials an upstream"
    );
}

/// (d) An unusable body is a `400` naming the field — never a guess.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unusable_body_is_a_400_and_never_reaches_the_upstream() {
    let (_dir, addr, upstream, _task) = rig("conf108-bad", MEDIA, "tok-108").await;

    // No image at all.
    let (status, body, _h) = post_tool(&addr, "ocr", &json!({ "prompt": "hi" }), Some("tok-108"));
    assert_eq!(status, 400, "a missing image is the caller's fault: {body}");
    assert_eq!(error_type(&body), "invalid_request", "{body}");

    // An audio type on the image verb.
    let (status, body, _h) = post_tool(
        &addr,
        "ocr",
        &json!({ "image": { "media_type": "audio/wav", "base64": "AAAA" } }),
        Some("tok-108"),
    );
    assert_eq!(status, 400, "ocr reads an image: {body}");
    assert_eq!(error_type(&body), "invalid_request", "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("transcribe"),
        "the refusal points at the verb that owns audio: {body}"
    );

    // Empty bytes.
    let (status, body, _h) = post_tool(
        &addr,
        "ocr",
        &json!({ "image": { "media_type": "image/png", "base64": "" } }),
        Some("tok-108"),
    );
    assert_eq!(status, 400, "the file's bytes are the request: {body}");
    assert_eq!(error_type(&body), "invalid_request", "{body}");

    assert!(
        upstream.requests().is_empty(),
        "no unusable body ever became an upstream call"
    );
}

/// (e) The media bound is its own knob: a body over `media.max_body_bytes`
/// answers `413`, below it the same call is served.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_media_body_bound_is_a_413() {
    let media = "media: { model: \"mock/glm\", max_body_bytes: 4096 }\n";
    let (_dir, addr, upstream, _task) = rig("conf108-big", media, "tok-108").await;

    let big = "A".repeat(8192);
    let (status, body, _h) = post_tool(
        &addr,
        "ocr",
        &json!({ "image": { "media_type": "image/jpeg", "base64": big } }),
        Some("tok-108"),
    );
    assert_eq!(status, 413, "over media.max_body_bytes: {body}");
    assert_eq!(error_type(&body), "request_too_large", "{body}");
    assert!(
        upstream.requests().is_empty(),
        "an oversize body is refused before any upstream call"
    );

    // The same shape under the bound is served (the limit is a bound, not
    // a blanket refusal).
    let (status, body, _h) = post_tool(
        &addr,
        "ocr",
        &json!({ "image": { "media_type": "image/jpeg", "base64": "AAAA" } }),
        Some("tok-108"),
    );
    assert_eq!(status, 200, "under the bound the call is served: {body}");
    assert_eq!(upstream.requests().len(), 1);
}

/// (f) The guard covers the surface: no token ⇒ `401`, no dial.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_media_surface_is_guarded() {
    let (_dir, addr, upstream, _task) = rig("conf108-guard", MEDIA, "tok-108").await;
    let (status, body, h) = post_tool(
        &addr,
        "ocr",
        &json!({ "image": { "media_type": "image/jpeg", "base64": IMAGE_B64 } }),
        None,
    );
    assert_eq!(status, 401, "the guard runs before the handler: {body}");
    assert_eq!(error_type(&body), "unauthorized", "{body}");
    assert!(
        header(&h, "x-vadis-request-id").is_some(),
        "the guard's refusal is §8's body, request id included"
    );
    assert!(
        upstream.requests().is_empty(),
        "an unauthenticated call never dials the upstream"
    );
}
