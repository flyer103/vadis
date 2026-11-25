//! CONF-124 (spec §4.22, ADR-065): **the media audio arms** —
//! POST /v1/tools/{transcribe,synthesize} on the real serve assembly over
//! loopback HTTP.
//!
//! Limbs:
//! - (a) **transcribe is adapted onto the provider's asr form** — the
//!   caller's JSON {audio:{media_type,base64}} is decoded, converted to
//!   the 16-bit PCM WAV the provider takes, and sent as a multipart body
//!   whose boundary lives in the content type; the caller answers
//!   {"text"} (never the provider's own envelope).
//! - (b) **synthesize returns the provider's audio untouched** for wav and
//!   pcm: the answer is the tool's {audio:{base64,media_type}}, the
//!   provider only ever asked for wav or pcm, and the caller's voice rides
//!   as voice_id (the default voice when it is absent).
//! - (c) **mp3 is transcoded from the provider's wav** — the provider
//!   cannot answer mp3, so vadis converts with media.ffmpeg and answers
//!   audio/mpeg; the base64 is the converted bytes.
//! - (d) **an upstream refusal is relayed, never re-shaped** — a 422
//!   arrives with the provider's own error body, not §8's envelope.
//! - (e) **without tools: the audio arms are 501** — the provider dial
//!   rides the tools section, and an absent section is not a silent
//!   fallback.
//! - (f) **a verb without its URL is a 501** naming tools.asr or
//!   tools.tts, with no dial.
//! - (g) **a missing converter is a 501 before the dial** — and only the
//!   arms that convert (transcribe, mp3 answers) need it: a wav answer is
//!   served with a broken media.ffmpeg.
//! - (h) **an unusable body is a 4xx** naming the field; no dial.
//! - (i) **the media bound covers the arms** — a body over
//!   media.max_body_bytes answers 413.
//! - (j) **the guard covers the arms** — no token, no call, 401.
//! - (k) **the record is the media word plus the tools group** —
//!   protocol_in "media", tools.verb the route's verb, and the call is
//!   unpriced (priced false, cost zero), never read as free.
//!
//! Offline: a canned loopback mock upstream and a shell-script ffmpeg
//! stand-in written by the test itself, so nothing here reads a host
//! converter.

#![forbid(unsafe_code)]

use serde_json::{json, Value};
use vadis_conformance::testkit::{self, CannedResponse};

const ASR_CANNED: &str = r#"{"request_id":"asr-1","text":"这句话被听见了","duration_ms":1234}"#;

const TOOLS: &str = "tools: { provider: mock, max_body_bytes: 65536, attempt_timeout: 30s }\n";

/// A canonical 16-bit PCM, mono, 16 kHz WAV: 0.1 s of silence, so the
/// route's duration bound accepts it.
fn wav_fixture() -> Vec<u8> {
    let data_len: u32 = 3_200;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&16_000u32.to_le_bytes());
    wav.extend_from_slice(&32_000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(&vec![0u8; data_len as usize]);
    wav
}

/// Standard RFC 4648 base64, padded — what the tool shape carries.
fn b64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b1 = chunk[0] as u32;
        let b2 = *chunk.get(1).unwrap_or(&0) as u32;
        let b3 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b1 << 16) | (b2 << 8) | b3;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// A converter stand-in: -version succeeds, and any conversion copies the
/// fixture WAV (or writes an mp3 marker) to the output path the last
/// argument names. Nothing outside the test's own temp dir is read.
fn stub_ffmpeg(tag: &str) -> String {
    let dir = testkit::tempdir(&format!("{tag}-ffmpeg"));
    let fixture = dir.join("fixture.wav");
    std::fs::write(&fixture, wav_fixture()).unwrap();
    let script = dir.join("ffmpeg-stub.sh");
    let body = format!(
        "#!/bin/sh\n\
         if [ \"$1\" = \"-version\" ]; then\n\
           echo 'ffmpeg version 0.0-stub'\n\
           exit 0\n\
         fi\n\
         for last; do :; done\n\
         case \"$last\" in\n\
           *.wav) cp '{}' \"$last\" ;;\n\
           *.mp3) printf 'ID3stub' > \"$last\" ;;\n\
           *) exit 1 ;;\n\
         esac\n",
        fixture.display()
    );
    std::fs::write(&script, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    script.to_string_lossy().into_owned()
}

fn media_line(ffmpeg: &str, max_body_bytes: i64) -> String {
    format!(
        "media: {{ model: \"mock/glm\", max_body_bytes: {max_body_bytes}, ffmpeg: \"{ffmpeg}\" }}\n"
    )
}

fn config_yaml(
    upstream_port: u16,
    listen_port: u16,
    provider_tools: &str,
    tools: &str,
    media: &str,
) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s, auth_token_env: CONF124_TOKEN }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: mock
    urls:
      chat: http://127.0.0.1:{upstream_port}/v1/chat/completions
{provider_tools}    api_key_env: CONF124_MOCK_KEY
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
{tools}{media}"#
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

async fn rig(
    tag: &str,
    verbs: &[&str],
    tools: &str,
    media: &str,
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
    std::fs::write(
        dir.join("config.yaml"),
        config_yaml(
            upstream.addr.port(),
            listen_port,
            &declared(verbs, upstream.addr.port()),
            tools,
            media,
        ),
    )
    .unwrap();
    std::env::set_var("CONF124_MOCK_KEY", "sk-conf124");
    std::env::set_var("CONF124_TOKEN", "tok-124");
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
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
    token: Option<&str>,
) -> (u16, Vec<u8>, Vec<(String, String)>) {
    let mut extra: Vec<(&str, &str)> = Vec::new();
    let auth;
    if let Some(t) = token {
        auth = format!("Bearer {t}");
        extra.push(("authorization", auth.as_str()));
    }
    testkit::http_post(addr, &format!("/v1/tools/{verb}"), body, &extra)
}

fn post_json(
    addr: &str,
    verb: &str,
    body: &[u8],
    token: Option<&str>,
) -> (u16, Value, Vec<(String, String)>) {
    let (status, bytes, headers) = post(addr, verb, body, token);
    let parsed = serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        panic!(
            "{verb} answered non-JSON ({status}): {e}: {:?}",
            String::from_utf8_lossy(&bytes)
        )
    });
    (status, parsed, headers)
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

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.len() <= haystack.len() && haystack.windows(needle.len()).any(|w| w == needle)
}

fn transcribe_body(media_type: &str, bytes: &[u8], language: Option<&str>) -> Vec<u8> {
    let mut body = json!({
        "audio": { "media_type": media_type, "base64": b64(bytes) }
    });
    if let Some(language) = language {
        body["language"] = json!(language);
    }
    serde_json::to_vec(&body).unwrap()
}

fn synthesize_body(text: &str, format: &str, voice: Option<&str>) -> Vec<u8> {
    let mut body = json!({ "text": text, "format": format });
    if let Some(voice) = voice {
        body["voice"] = json!(voice);
    }
    serde_json::to_vec(&body).unwrap()
}

/// (a) The caller's JSON is adapted onto the provider's multipart asr form,
/// and the transcript comes back in the tool's own shape.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transcribe_is_adapted_to_the_asr_form() {
    let ffmpeg = stub_ffmpeg("conf124-asr");
    let (_dir, addr, upstream, _task) = rig(
        "conf124-asr",
        &["asr"],
        TOOLS,
        &media_line(&ffmpeg, 8_388_608),
    )
    .await;
    upstream.queue(CannedResponse::json(200, "OK", ASR_CANNED.as_bytes()));

    let caller = b"m4a-container-bytes";
    let body = transcribe_body("audio/mp4", caller, Some("zh"));
    let (status, answer, headers) = post_json(&addr, "transcribe", &body, Some("tok-124"));
    assert_eq!(
        status,
        200,
        "the arm answers 200: {}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(
        answer["text"], "这句话被听见了",
        "the tool's own shape, never the provider's envelope: {answer}"
    );
    assert_eq!(
        header(&headers, "content-type"),
        Some("application/json"),
        "the transcript is JSON"
    );

    let seen = upstream.requests();
    assert_eq!(seen.len(), 1, "exactly one upstream call");
    let content_type = seen[0].header("content-type").unwrap_or("");
    assert!(
        content_type.starts_with("multipart/form-data; boundary="),
        "the asr form is multipart and its boundary is in the header: {content_type}"
    );
    assert_eq!(
        seen[0].header("authorization"),
        Some("Bearer sk-conf124"),
        "the provider's key is vadis's, never the caller's token"
    );
    let sent = &seen[0].body;
    assert!(
        contains(sent, b"name=\"file\"; filename=\"audio.wav\""),
        "the file field is the provider's contract"
    );
    assert!(
        contains(sent, b"Content-Type: audio/wav"),
        "the converted part declares its own type"
    );
    assert!(
        contains(sent, b"name=\"language\""),
        "the caller's language hint rides as its own field"
    );
    assert!(
        contains(sent, b"zh"),
        "the language value reaches the provider"
    );
    assert!(
        contains(sent, b"RIFF"),
        "the body carries the converted WAV, not the caller's container"
    );
    assert!(
        !contains(sent, caller),
        "the caller's original container never reaches the provider"
    );
}

/// (b) wav and pcm answers are the provider's own bytes: base64 in the tool
/// shape, with the container the caller asked for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn synthesize_returns_wav_and_pcm_untouched() {
    let ffmpeg = stub_ffmpeg("conf124-tts");
    let (_dir, addr, upstream, _task) = rig(
        "conf124-tts",
        &["tts"],
        TOOLS,
        &media_line(&ffmpeg, 8_388_608),
    )
    .await;
    let wav = wav_fixture();
    upstream.queue(CannedResponse::json(200, "OK", &wav).with_header("content-type", "audio/wav"));
    upstream.queue(
        CannedResponse::json(200, "OK", b"pcm-raw-1").with_header("content-type", "audio/pcm"),
    );

    let (status, answer, headers) = post_json(
        &addr,
        "synthesize",
        &synthesize_body("你好", "wav", None),
        Some("tok-124"),
    );
    assert_eq!(status, 200, "the arm answers 200: {answer}");
    assert_eq!(answer["audio"]["media_type"], "audio/wav");
    assert_eq!(
        answer["audio"]["base64"],
        b64(&wav),
        "the provider's wav is the caller's audio"
    );
    assert_eq!(header(&headers, "content-type"), Some("application/json"));

    let (status, answer, _h) = post_json(
        &addr,
        "synthesize",
        &synthesize_body("你好", "pcm", None),
        Some("tok-124"),
    );
    assert_eq!(status, 200);
    assert_eq!(answer["audio"]["media_type"], "audio/pcm");
    assert_eq!(
        answer["audio"]["base64"],
        b64(b"pcm-raw-1"),
        "raw pcm rides through untouched"
    );

    let seen = upstream.requests();
    assert_eq!(seen.len(), 2, "one dial per call");
    let first: Value = serde_json::from_slice(&seen[0].body).unwrap();
    assert_eq!(
        first,
        json!({ "text": "你好", "format": "wav", "voice_id": "stella" }),
        "the provider only ever makes wav, and the default voice is named"
    );
    let second: Value = serde_json::from_slice(&seen[1].body).unwrap();
    assert_eq!(second["format"], "pcm", "pcm needs no conversion");
    assert_eq!(
        seen[0].header("content-type"),
        Some("application/json"),
        "the tts request is JSON"
    );
}

/// (c) mp3 is not a provider container: vadis transcodes the provider's wav
/// and answers audio/mpeg.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn synthesize_transcodes_mp3_from_the_providers_wav() {
    let ffmpeg = stub_ffmpeg("conf124-mp3");
    let (_dir, addr, upstream, _task) = rig(
        "conf124-mp3",
        &["tts"],
        TOOLS,
        &media_line(&ffmpeg, 8_388_608),
    )
    .await;
    upstream.queue(
        CannedResponse::json(200, "OK", &wav_fixture()).with_header("content-type", "audio/wav"),
    );

    let (status, answer, _h) = post_json(
        &addr,
        "synthesize",
        &synthesize_body("转成 mp3", "mp3", Some("tide")),
        Some("tok-124"),
    );
    assert_eq!(status, 200, "the transcoded answer is served: {answer}");
    assert_eq!(answer["audio"]["media_type"], "audio/mpeg");
    assert_eq!(
        answer["audio"]["base64"],
        b64(b"ID3stub"),
        "the answer is the converter's mp3, not the provider's wav"
    );

    let seen = upstream.requests();
    assert_eq!(seen.len(), 1);
    let sent: Value = serde_json::from_slice(&seen[0].body).unwrap();
    assert_eq!(
        sent["format"], "wav",
        "the provider is asked for wav: it has no mp3"
    );
    assert_eq!(sent["voice_id"], "tide", "the caller's voice rides through");
}

/// (d) An upstream refusal is the provider's answer, not §8's envelope.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_upstream_refusal_is_relayed_verbatim() {
    let ffmpeg = stub_ffmpeg("conf124-relay");
    let (_dir, addr, upstream, _task) = rig(
        "conf124-relay",
        &["tts"],
        TOOLS,
        &media_line(&ffmpeg, 8_388_608),
    )
    .await;
    let refused =
        br#"{"error":{"type":"content_filter","message":"this text will not be spoken"}}"#;
    upstream.queue(
        CannedResponse::json(422, "Unprocessable Entity", refused)
            .with_header("content-type", "application/json"),
    );

    let (status, body, headers) = post(
        &addr,
        "synthesize",
        &synthesize_body("不许说", "wav", None),
        Some("tok-124"),
    );
    assert_eq!(status, 422, "the provider's status is the caller's status");
    assert_eq!(
        body, refused,
        "the provider's own error body is relayed byte for byte"
    );
    assert_eq!(
        header(&headers, "content-type"),
        Some("application/json"),
        "the provider's content type is relayed"
    );
    let parsed = error_body(&body);
    assert_eq!(
        parsed["error"]["type"], "content_filter",
        "not §8's upstream_error: the caller reads the provider's refusal"
    );
}

/// (e) Without a tools: section the audio arms are not served.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_the_tools_section_the_audio_arms_answer_501() {
    let ffmpeg = stub_ffmpeg("conf124-notools");
    let (_dir, addr, upstream, _task) =
        rig("conf124-notools", &[], "", &media_line(&ffmpeg, 8_388_608)).await;

    for (verb, body) in [
        ("transcribe", transcribe_body("audio/wav", b"x", None)),
        ("synthesize", synthesize_body("你好", "wav", None)),
    ] {
        let (status, answer, _h) = post_json(&addr, verb, &body, Some("tok-124"));
        assert_eq!(status, 501, "{verb} without tools: {answer}");
        assert_eq!(error_type(&answer), "not_implemented", "{answer}");
        assert!(
            answer.to_string().contains("tools"),
            "{verb} names the missing section: {answer}"
        );
    }
    assert!(
        upstream.requests().is_empty(),
        "an unserved arm never dials the provider"
    );
}

/// (f) Declaring a different verb does not promise this one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_verb_without_its_url_is_a_501() {
    let ffmpeg = stub_ffmpeg("conf124-nourl");
    let (_dir, addr, upstream, _task) = rig(
        "conf124-nourl",
        &["search"],
        TOOLS,
        &media_line(&ffmpeg, 8_388_608),
    )
    .await;

    for (verb, body) in [
        ("transcribe", transcribe_body("audio/wav", b"x", None)),
        ("synthesize", synthesize_body("你好", "wav", None)),
    ] {
        let (status, answer, _h) = post_json(&addr, verb, &body, Some("tok-124"));
        assert_eq!(status, 501, "{verb} without its URL: {answer}");
        assert_eq!(error_type(&answer), "not_implemented", "{answer}");
        let text = answer.to_string();
        let expected = format!("tools.{}", if verb == "transcribe" { "asr" } else { "tts" });
        assert!(
            text.contains(&expected),
            "{verb} names {expected}: {answer}"
        );
    }
    assert!(
        upstream.requests().is_empty(),
        "a missing URL never dials anything"
    );
}

/// (g) A missing converter is a 501 before the dial, and only the arms that
/// convert need it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_converter_is_a_501_before_the_dial() {
    let (_dir, addr, upstream, _task) = rig(
        "conf124-noffmpeg",
        &["asr", "tts"],
        TOOLS,
        &media_line("/nonexistent/conf124-ffmpeg", 8_388_608),
    )
    .await;

    let (status, answer, _h) = post_json(
        &addr,
        "transcribe",
        &transcribe_body("audio/wav", b"x", None),
        Some("tok-124"),
    );
    assert_eq!(status, 501, "no converter, no transcription: {answer}");
    assert!(
        answer.to_string().contains("ffmpeg"),
        "the refusal names the converter: {answer}"
    );
    assert_eq!(error_type(&answer), "not_implemented");

    let (status, answer, _h) = post_json(
        &addr,
        "synthesize",
        &synthesize_body("你好", "mp3", None),
        Some("tok-124"),
    );
    assert_eq!(status, 501, "mp3 needs the converter: {answer}");

    upstream.queue(
        CannedResponse::json(200, "OK", &wav_fixture()).with_header("content-type", "audio/wav"),
    );
    let (status, answer, _h) = post_json(
        &addr,
        "synthesize",
        &synthesize_body("你好", "wav", None),
        Some("tok-124"),
    );
    assert_eq!(
        status, 200,
        "a wav answer is never transcoded, so a broken converter is not in its path: {answer}"
    );
    assert_eq!(upstream.requests().len(), 1, "only the wav call dialed");
}

/// (h) An unusable body is refused naming the field, never a guess.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unusable_body_is_refused() {
    let ffmpeg = stub_ffmpeg("conf124-badbody");
    let (_dir, addr, upstream, _task) = rig(
        "conf124-badbody",
        &["asr", "tts"],
        TOOLS,
        &media_line(&ffmpeg, 8_388_608),
    )
    .await;

    let too_long = "字".repeat(10_001);
    let cases: [(&str, Vec<u8>, u16, &str); 4] = [
        (
            "transcribe",
            transcribe_body("image/jpeg", b"not audio", None),
            400,
            "audio",
        ),
        ("synthesize", synthesize_body("", "wav", None), 400, "text"),
        (
            "synthesize",
            synthesize_body(&too_long, "wav", None),
            413,
            "characters",
        ),
        (
            "synthesize",
            synthesize_body("你好", "flac", None),
            400,
            "format",
        ),
    ];
    for (verb, body, expected, needle) in cases {
        let (status, answer, _h) = post_json(&addr, verb, &body, Some("tok-124"));
        assert_eq!(status, expected, "{verb} with an unusable body: {answer}");
        assert!(
            answer.to_string().contains(needle),
            "{verb}'s refusal names {needle:?}: {answer}"
        );
    }
    assert!(
        upstream.requests().is_empty(),
        "an unusable body never became an upstream call"
    );
}

/// (i) The media body bound covers the audio arms.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_media_body_bound_covers_the_audio_arms() {
    let ffmpeg = stub_ffmpeg("conf124-big");
    let (_dir, addr, upstream, _task) =
        rig("conf124-big", &["asr"], TOOLS, &media_line(&ffmpeg, 4_096)).await;

    let body = transcribe_body("audio/wav", &[0u8; 8_192], None);
    let (status, answer, _h) = post_json(&addr, "transcribe", &body, Some("tok-124"));
    assert_eq!(status, 413, "over media.max_body_bytes: {answer}");
    assert_eq!(error_type(&answer), "request_too_large", "{answer}");
    assert_eq!(
        answer["error"]["details"]["bound"], "media.max_body_bytes",
        "the refusal names the knob it enforced: {answer}"
    );
    assert!(
        upstream.requests().is_empty(),
        "an oversize body is refused before any dial"
    );
}

/// (j) The guard covers the arms: no token, no call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_audio_arms_are_guarded() {
    let ffmpeg = stub_ffmpeg("conf124-guard");
    let (_dir, addr, upstream, _task) = rig(
        "conf124-guard",
        &["asr"],
        TOOLS,
        &media_line(&ffmpeg, 8_388_608),
    )
    .await;
    let (status, answer, _h) = post_json(
        &addr,
        "transcribe",
        &transcribe_body("audio/wav", b"x", None),
        None,
    );
    assert_eq!(status, 401, "the guard runs before the handler: {answer}");
    assert_eq!(error_type(&answer), "unauthorized", "{answer}");
    assert!(upstream.requests().is_empty(), "a refused call never dials");
}

/// (k) The record is the media word plus the tools group, and the call is
/// unpriced — never read as free.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_record_carries_the_media_word_and_the_tools_group() {
    let ffmpeg = stub_ffmpeg("conf124-record");
    let (dir, addr, upstream, task) = rig(
        "conf124-record",
        &["asr"],
        TOOLS,
        &media_line(&ffmpeg, 8_388_608),
    )
    .await;
    upstream.queue(CannedResponse::json(200, "OK", ASR_CANNED.as_bytes()));

    let body = transcribe_body("audio/wav", b"x", None);
    let (status, answer, _h) = post_json(&addr, "transcribe", &body, Some("tok-124"));
    assert_eq!(status, 200, "{answer}");

    task.abort();
    let _ = task.await;
    let records = read_records(&dir.join("state/traces"));
    assert_eq!(records.len(), 1, "exactly one record for the call");
    let rec = &records[0];

    assert_eq!(
        rec["protocol"]["protocol_in"], "media",
        "the audio arms are the media surface's own word: {rec}"
    );
    let tools = rec
        .get("tools")
        .unwrap_or_else(|| panic!("the tools group: {rec}"));
    assert_eq!(tools["verb"], "transcribe");
    assert_eq!(tools["provider"], "mock");
    assert_eq!(tools["upstream_status"], 200);
    assert_eq!(tools["priced"], false, "no tool price table yet");
    assert_eq!(rec["usage_missing"], true, "an asr answer reports no usage");
    assert_eq!(
        rec["cost"]["total"], 0,
        "zero cost, and the priced flag says why"
    );
    assert!(
        rec.get("cache").is_none(),
        "an audio call is never a cache hit"
    );
}
