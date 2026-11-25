//! Spec §4.22: the media tool surface — `POST /v1/tools/{ocr,transcribe,synthesize}`.
//!
//! The three paths answer a **verb**, not a chat completion: the caller sends
//! a file and an optional instruction, and gets back what the file says. The
//! surface carries no client-chosen model (spec §4.22) — `media.model` names
//! the one route the deployment serves these calls with — so the request the
//! pipeline actually forwards is *synthesized* here, in one place, and the
//! answer is parsed back into the tool shape. Everything in this module is
//! pure: bytes in, bytes out, no clock, no randomness, no I/O. The HTTP
//! wiring (the guard layer, the body bound, the 501 arms) belongs to
//! `vadis-cli`, exactly as `models_read` splits from its route.
//!
//! The bytes this module builds are a vadis-owned request, not a client's:
//! the byte-faithfulness rule (§2.1, AGENTS.md) governs *passthrough*, and
//! this surface has no client request to preserve. The synthesized body is
//! the observable input of the call, and the trace records it like any other
//! upstream request.

use serde_json::{json, Value};
use vadis_core::config::MediaCfg;
use vadis_core::error::ErrorCode;

/// The instruction an OCR call carries when neither the caller nor
/// `media.prompt` names one. It is a *transcription* instruction, not a
/// "describe this" one: the transcript is what the caller's model reads
/// next, and a description would silently replace the file's own words with
/// a paraphrase (spec §4.22).
pub const DEFAULT_OCR_PROMPT: &str = "Transcribe every piece of text in this image exactly as it appears. If the image contains no text, describe what it shows in one or two sentences.";

/// §4.22's error shape: §8's vocabulary plus the message, so the handler can
/// build one `ErrorBody` without guessing which code an arm is.
#[derive(Debug, Clone)]
pub struct MediaToolError {
    pub code: ErrorCode,
    pub message: String,
}

impl MediaToolError {
    pub(crate) fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidRequest, message)
    }
}

/// §4.22: the request → the chat body the pipeline forwards. The model is
/// the deployment's (`media.model`); `stream` is pinned false because the
/// tool answer is one JSON object, and a streamed answer would have to be
/// reassembled before it could be parsed. An unusable body is a `400
/// invalid_request` naming the field, never a best-effort guess.
pub fn ocr_chat_body(raw: &[u8], cfg: &MediaCfg) -> Result<Vec<u8>, MediaToolError> {
    let body: Value = serde_json::from_slice(raw)
        .map_err(|e| MediaToolError::invalid(format!("the request body is not JSON: {e}")))?;
    let image = body
        .get("image")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            MediaToolError::invalid(
                "missing image: expected an object with media_type and base64 (spec §4.22)",
            )
        })?;
    let media_type = image
        .get("media_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            MediaToolError::invalid(
                "missing image.media_type: the file's media type is required and is never guessed from the bytes (spec §4.22)",
            )
        })?;
    if !media_type.starts_with("image/") {
        return Err(MediaToolError::invalid(format!(
            "image.media_type is '{media_type}': /v1/tools/ocr reads an image (an image/* type); audio belongs to /v1/tools/transcribe (spec §4.22)"
        )));
    }
    let base64 = image
        .get("base64")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            MediaToolError::invalid(
                "missing image.base64: the file's bytes are the request (spec §4.22)",
            )
        })?;
    let prompt = body
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .or_else(|| {
            cfg.prompt
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
        })
        .unwrap_or(DEFAULT_OCR_PROMPT);
    let chat = json!({
        "model": cfg.model,
        "stream": false,
        "messages": [{
            "role": "user",
            "content": [
                { "type": "text", "text": prompt },
                {
                    "type": "image_url",
                    "image_url": { "url": format!("data:{media_type};base64,{base64}") }
                }
            ]
        }]
    });
    serde_json::to_vec(&chat).map_err(|e| {
        MediaToolError::new(
            ErrorCode::Internal,
            format!("cannot encode the media request: {e}"),
        )
    })
}

/// §4.22: the upstream answer → the tool's `text`. Both content shapes an
/// OpenAI-compatible route may answer with are read (a string, or the parts
/// array a multimodal model returns); an answer with no text in it is an
/// honest `502 upstream_error` — a tool call that returns nothing must not
/// look like a successful empty transcript.
pub fn ocr_text(body: &[u8]) -> Result<String, MediaToolError> {
    let answer: Value = serde_json::from_slice(body).map_err(|e| {
        MediaToolError::new(
            ErrorCode::UpstreamError,
            format!("the upstream answer is not JSON: {e}"),
        )
    })?;
    let content = answer
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .ok_or_else(|| {
            MediaToolError::new(
                ErrorCode::UpstreamError,
                "the upstream answer carried no choices[0].message.content",
            )
        })?;
    let text = match content {
        Value::String(s) => s.trim().to_string(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    };
    if text.is_empty() {
        return Err(MediaToolError::new(
            ErrorCode::UpstreamError,
            "the upstream answered with no text content (an empty tool answer is not a successful transcript, spec §4.22)",
        ));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> MediaCfg {
        MediaCfg {
            model: "vision/ocr".to_string(),
            max_body_bytes: 8_388_608,
            prompt: None,
            ffmpeg: "ffmpeg".to_string(),
        }
    }

    fn ocr_request(prompt: Option<&str>) -> Vec<u8> {
        let mut body = json!({
            "image": { "media_type": "image/jpeg", "base64": "/9j/4AAQSkZJRg==" }
        });
        if let Some(p) = prompt {
            body["prompt"] = json!(p);
        }
        serde_json::to_vec(&body).expect("test body encodes")
    }

    #[test]
    fn the_chat_body_names_the_deployments_model_and_pins_stream_false() {
        let chat: Value =
            serde_json::from_slice(&ocr_chat_body(&ocr_request(None), &cfg()).unwrap()).unwrap();
        assert_eq!(chat["model"], "vision/ocr");
        assert_eq!(chat["stream"], false);
        assert_eq!(chat["messages"][0]["role"], "user");
        assert_eq!(chat["messages"][0]["content"][0]["type"], "text");
        assert_eq!(
            chat["messages"][0]["content"][0]["text"],
            DEFAULT_OCR_PROMPT
        );
        assert_eq!(
            chat["messages"][0]["content"][1]["image_url"]["url"],
            "data:image/jpeg;base64,/9j/4AAQSkZJRg=="
        );
    }

    #[test]
    fn the_callers_prompt_wins_over_the_configured_one() {
        let mut c = cfg();
        c.prompt = Some("configured instruction".to_string());
        let chat: Value = serde_json::from_slice(
            &ocr_chat_body(&ocr_request(Some("what does the sign say")), &c).unwrap(),
        )
        .unwrap();
        assert_eq!(
            chat["messages"][0]["content"][0]["text"],
            "what does the sign say"
        );
        let chat: Value =
            serde_json::from_slice(&ocr_chat_body(&ocr_request(None), &c).unwrap()).unwrap();
        assert_eq!(
            chat["messages"][0]["content"][0]["text"],
            "configured instruction"
        );
    }

    #[test]
    fn an_unusable_body_is_a_named_invalid_request() {
        let cases = [
            (b"not json".to_vec(), "not JSON"),
            ("{}".as_bytes().to_vec(), "missing image"),
            (
                serde_json::to_vec(&json!({"image": {"media_type": "image/png"}})).unwrap(),
                "missing image.base64",
            ),
            (
                serde_json::to_vec(&json!({"image": {"base64": "AA=="}})).unwrap(),
                "missing image.media_type",
            ),
            (
                serde_json::to_vec(
                    &json!({"image": {"media_type": "audio/ogg", "base64": "AA=="}}),
                )
                .unwrap(),
                "image/*",
            ),
        ];
        for (raw, needle) in cases {
            let err = ocr_chat_body(&raw, &cfg()).expect_err("must refuse");
            assert_eq!(err.code, ErrorCode::InvalidRequest, "{needle}");
            assert!(
                err.message.contains(needle),
                "{} lacks {needle}",
                err.message
            );
        }
    }

    #[test]
    fn the_answer_text_is_read_from_a_string_or_a_parts_array() {
        let s = serde_json::to_vec(&json!({"choices": [{"message": {"content": "  hello  "}}]}))
            .unwrap();
        assert_eq!(ocr_text(&s).unwrap(), "hello");
        let parts = serde_json::to_vec(&json!({"choices": [{"message": {"content": [
            {"type": "text", "text": "line one"},
            {"type": "text", "text": " "},
            {"type": "text", "text": "line two"}
        ]}}]}))
        .unwrap();
        assert_eq!(ocr_text(&parts).unwrap(), "line one\nline two");
    }

    #[test]
    fn an_answer_without_text_is_an_upstream_error() {
        for raw in [
            serde_json::to_vec(&json!({"choices": [{"message": {"content": "   "}}]})).unwrap(),
            serde_json::to_vec(&json!({"choices": [{"message": {"content": []}}]})).unwrap(),
            serde_json::to_vec(&json!({"choices": []})).unwrap(),
            b"not json".to_vec(),
        ] {
            let err = ocr_text(&raw).expect_err("must refuse");
            assert_eq!(err.code, ErrorCode::UpstreamError);
        }
    }
}
