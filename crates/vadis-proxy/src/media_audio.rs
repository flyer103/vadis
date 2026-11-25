//! Spec §4.22's audio arms: `POST /v1/tools/transcribe` and
//! `POST /v1/tools/synthesize`.
//!
//! The media surface speaks one shape — a JSON body whose media member is
//! `{media_type, base64}` (ADR-061's client contract) — while the provider's
//! own endpoints speak others: `asr` takes a multipart 16-bit PCM WAV, and
//! `tts` answers raw audio. This module is the adapter between the two, and
//! nothing else: it parses the caller's body, converts the container with
//! the configured `ffmpeg`, builds the provider's own request, and shapes
//! the provider's answer back into the caller's JSON. It performs no HTTP
//! and makes no routing decision — the route handler does both, through the
//! same `tools:` credential pool the relay surface uses (spec §4.22.9).
//!
//! Every function that can refuse answers a [`MediaToolError`]: the §8 code
//! plus the sentence naming the field, the bound or the environment fact
//! that refused. A refusal here is always before the provider is dialled,
//! except for the answer-shape checks, which run on bytes already received.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};
use vadis_core::error::ErrorCode;

use crate::media_tools::MediaToolError;

/// The provider's `asr` upload bound, 8 MiB. The converted WAV of the
/// longest accepted clip is ~3.8 MiB, so this bound only speaks when a
/// caller hands over a container the converter could not shrink.
pub const MAX_ASR_BYTES: usize = 8 * 1024 * 1024;

/// The provider's `asr` duration bound, 120 s. Checked on the converted
/// WAV: a longer clip is refused, never truncated — a transcript of the
/// first two minutes presented as the file's transcript is a lie.
pub const MAX_AUDIO_SECONDS: f64 = 120.0;

/// The provider's per-call `tts` text bound, 10,000 characters.
pub const MAX_TTS_CHARS: usize = 10_000;

/// The voice a synthesis call uses when neither the caller nor the
/// deployment names one. Two voices exist (`stella`, `tide`); the pick is
/// the provider's documented default pair's first member.
pub const DEFAULT_TTS_VOICE: &str = "stella";

/// Spec §4.22's synthesis containers, the closed set `kian`'s media config
/// validates against. `mp3` is served by transcoding the provider's WAV
/// (the provider answers `wav` or raw `pcm` only).
pub const SPEECH_FORMATS: [&str; 3] = ["mp3", "wav", "pcm"];

/// One parsed `transcribe` request.
#[derive(Debug, Clone)]
pub struct TranscribeRequest {
    pub media_type: String,
    pub bytes: Vec<u8>,
    pub language: Option<String>,
}

/// [§4.22]: the caller's JSON → the bytes to convert and the optional
/// language hint. `media_type` is required and must be `audio/*`: the
/// surface never guesses a container from the bytes.
pub fn transcribe_request(raw: &[u8]) -> Result<TranscribeRequest, MediaToolError> {
    let body: Value = serde_json::from_slice(raw)
        .map_err(|e| MediaToolError::invalid(format!("the request body is not JSON: {e}")))?;
    let audio = body
        .get("audio")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            MediaToolError::invalid(
                "missing audio: expected an object with media_type and base64 (spec §4.22)",
            )
        })?;
    let media_type = audio
        .get("media_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            MediaToolError::invalid(
                "missing audio.media_type: the file's media type is required and is never \
                 guessed from the bytes (spec §4.22)",
            )
        })?;
    if !media_type.starts_with("audio/") {
        return Err(MediaToolError::invalid(format!(
            "audio.media_type is '{media_type}': /v1/tools/transcribe reads audio (an audio/* \
             type); images belong to /v1/tools/ocr (spec §4.22)"
        )));
    }
    let encoded = audio
        .get("base64")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            MediaToolError::invalid(
                "missing audio.base64: the file's bytes are the request (spec §4.22)",
            )
        })?;
    let bytes = decode_base64(encoded).map_err(|e| {
        MediaToolError::invalid(format!("audio.base64 is not RFC 4648 base64: {e}"))
    })?;
    if bytes.is_empty() {
        return Err(MediaToolError::invalid(
            "audio.base64 decodes to zero bytes: there is nothing to transcribe (spec §4.22)",
        ));
    }
    if bytes.len() > MAX_ASR_BYTES {
        return Err(MediaToolError::new(
            ErrorCode::RequestTooLarge,
            format!(
                "the audio is {} bytes after decoding, above the provider's {MAX_ASR_BYTES}-byte \
                 upload bound (spec §4.22)",
                bytes.len()
            ),
        ));
    }
    let language = match body.get("language") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let hint = value.as_str().map(str::trim).ok_or_else(|| {
                MediaToolError::invalid("language must be a string when it is written (spec §4.22)")
            })?;
            if hint.is_empty() {
                None
            } else {
                if hint.chars().count() > 35 {
                    return Err(MediaToolError::invalid(format!(
                        "language is {} characters, above the 35-character bound: it is a tag \
                         such as zh-CN, not a sentence (spec §4.22)",
                        hint.chars().count()
                    )));
                }
                if !hint
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                {
                    return Err(MediaToolError::invalid(
                        "language carries characters outside [A-Za-z0-9-_]: it is a language \
                         tag such as zh-CN (spec §4.22)",
                    ));
                }
                Some(hint.to_string())
            }
        }
    };
    Ok(TranscribeRequest {
        media_type: media_type.to_string(),
        bytes,
        language,
    })
}

/// [§4.22]: one of the caller's synthesis containers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechFormat {
    Mp3,
    Wav,
    Pcm,
}

impl SpeechFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Wav => "wav",
            Self::Pcm => "pcm",
        }
    }

    /// The provider's own `format` value for this container: `mp3` is not
    /// one of them, so it is served as `wav` plus a transcode.
    fn provider_format(self) -> &'static str {
        match self {
            Self::Mp3 | Self::Wav => "wav",
            Self::Pcm => "pcm",
        }
    }

    /// The `media_type` a successful answer carries.
    pub fn answer_media_type(self) -> &'static str {
        match self {
            Self::Mp3 => "audio/mpeg",
            Self::Wav => "audio/wav",
            Self::Pcm => "audio/pcm",
        }
    }
}

/// One parsed `synthesize` request.
#[derive(Debug, Clone)]
pub struct SynthesizeRequest {
    pub text: String,
    pub format: SpeechFormat,
    pub voice: Option<String>,
}

/// [§4.22]: the caller's JSON → text, container and voice. `format`
/// defaults to `mp3` — the container `kian`'s own config defaults to, and
/// the one a chat client can play without a second step.
pub fn synthesize_request(raw: &[u8]) -> Result<SynthesizeRequest, MediaToolError> {
    let body: Value = serde_json::from_slice(raw)
        .map_err(|e| MediaToolError::invalid(format!("the request body is not JSON: {e}")))?;
    let text = body
        .get("text")
        .and_then(Value::as_str)
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| {
            MediaToolError::invalid(
                "missing text: the speech is the request, and an empty one has nothing to say \
                 (spec §4.22)",
            )
        })?;
    let chars = text.chars().count();
    if chars > MAX_TTS_CHARS {
        return Err(MediaToolError::new(
            ErrorCode::RequestTooLarge,
            format!(
                "text is {chars} characters, above the provider's {MAX_TTS_CHARS}-character \
                 bound (spec §4.22)"
            ),
        ));
    }
    let format = match body.get("format").and_then(Value::as_str) {
        None => SpeechFormat::Mp3,
        Some(raw) => match raw.trim() {
            "mp3" => SpeechFormat::Mp3,
            "wav" => SpeechFormat::Wav,
            "pcm" => SpeechFormat::Pcm,
            other => {
                return Err(MediaToolError::invalid(format!(
                    "format is '{other}': the supported containers are {} (spec §4.22)",
                    SPEECH_FORMATS.join(", ")
                )))
            }
        },
    };
    let voice = match body.get("voice") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let voice = value.as_str().map(str::trim).ok_or_else(|| {
                MediaToolError::invalid("voice must be a string when it is written (spec §4.22)")
            })?;
            if voice.is_empty() {
                None
            } else {
                if voice.chars().count() > 64 {
                    return Err(MediaToolError::invalid(
                        "voice is longer than 64 characters: the provider's vocabulary is a \
                         short name, not a sentence (spec §4.22)",
                    ));
                }
                Some(voice.to_string())
            }
        }
    };
    Ok(SynthesizeRequest {
        text: text.to_string(),
        format,
        voice,
    })
}

/// [§4.22]: the provider's `tts` request body. The voice is passed
/// through as written — the provider owns its vocabulary and answers a
/// `4xx` for a name it does not know, which the route relays verbatim.
pub fn tts_body(req: &SynthesizeRequest) -> Vec<u8> {
    let body = json!({
        "text": req.text,
        "format": req.format.provider_format(),
        "voice_id": req.voice.as_deref().unwrap_or(DEFAULT_TTS_VOICE),
    });
    serde_json::to_vec(&body).expect("the tts body is serialisable")
}

/// [§4.22]: the multipart `asr` request. One `file` part carrying the
/// converted WAV, plus the optional `language` hint as its own field when
/// the caller wrote one. The boundary is the route's business; this
/// function only builds the bytes around it.
pub fn multipart_audio(wav: &[u8], boundary: &str, language: Option<&str>) -> Vec<u8> {
    let mut out = Vec::with_capacity(wav.len() + 512);
    out.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .as_bytes(),
    );
    out.extend_from_slice(wav);
    out.extend_from_slice(b"\r\n");
    if let Some(language) = language {
        out.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"language\"\r\n\r\n\
                 {language}\r\n"
            )
            .as_bytes(),
        );
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    out
}

/// The multipart boundary one call uses. Derived from the request id so the
/// trace and the wire agree, and stripped to characters a header can carry
/// (the id is generated here, but the boundary is a header value either
/// way).
pub fn boundary_for(request_id: &str) -> String {
    let sanitized: String = request_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    format!("vadis-{sanitized}")
}

/// The caller's answer for a transcript: `{"text": ...}`.
pub fn transcribe_answer(text: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({ "text": text })).expect("the transcript answer is serialisable")
}

/// [§4.22]: the provider's `asr` answer → the transcript. A body without
/// a usable `text` is an upstream failure, never an empty transcript: an
/// empty string would reach the caller's model as "the audio says nothing",
/// which is a different claim from "the provider did not answer".
pub fn transcribe_text(body: &[u8]) -> Result<String, MediaToolError> {
    let parsed: Value = serde_json::from_slice(body).map_err(|e| {
        MediaToolError::new(
            ErrorCode::UpstreamError,
            format!("the asr endpoint's answer is not JSON: {e}"),
        )
    })?;
    let text = parsed
        .get("text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            MediaToolError::new(
                ErrorCode::UpstreamError,
                "the asr endpoint answered without text: there is no transcript to return \
                 (spec §4.22)"
                    .to_string(),
            )
        })?;
    Ok(text.to_string())
}

/// The caller's answer for synthesized speech:
/// `{"audio": {"base64": ..., "media_type": ...}}`.
pub fn synthesize_answer(audio: &[u8], media_type: &str) -> Vec<u8> {
    let body = json!({
        "audio": {
            "base64": encode_base64(audio),
            "media_type": media_type,
        }
    });
    serde_json::to_vec(&body).expect("the speech answer is serialisable")
}

// ---------------------------------------------------------------------------
// RFC 4648 base64. The workspace carries no base64 crate and this module is
// the only place the bytes must be encoded or decoded (the OCR path never
// touches them: it passes the caller's data URI through), so the pair lives
// here with the vectors that pin it.
// ---------------------------------------------------------------------------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
pub fn encode_base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[((n >> 18) & 63) as usize] as char);
        out.push(B64[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(B64[((n >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(B64[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Strict standard base64: the alphabet only, padding only at the end, and
/// no whitespace (a media body's base64 is machine-written, never wrapped).
pub fn decode_base64(encoded: &str) -> Result<Vec<u8>, String> {
    if !encoded.len().is_multiple_of(4) {
        return Err(format!("length {} is not a multiple of 4", encoded.len()));
    }
    if let Some(first_pad) = encoded.find('=') {
        if encoded.len() - first_pad > 2 || !encoded[first_pad..].chars().all(|c| c == '=') {
            return Err("padding is only allowed as the final one or two characters".to_string());
        }
    }
    let mut out = Vec::with_capacity(encoded.len() / 4 * 3);
    for (chunk_index, chunk) in encoded.as_bytes().chunks(4).enumerate() {
        let mut values = [0u8; 4];
        let mut pad = 0usize;
        for (position, byte) in chunk.iter().enumerate() {
            values[position] = match byte {
                b'A'..=b'Z' => byte - b'A',
                b'a'..=b'z' => byte - b'a' + 26,
                b'0'..=b'9' => byte - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' => {
                    pad += 1;
                    0
                }
                other => {
                    return Err(format!(
                        "byte {} is '{other}', outside the base64 alphabet",
                        chunk_index * 4 + position
                    ))
                }
            };
        }
        if pad > 0 && chunk_index + 1 != encoded.len() / 4 {
            return Err("padding appears before the final group".to_string());
        }
        out.push((values[0] << 2) | (values[1] >> 4));
        if pad < 2 {
            out.push((values[1] << 4) | (values[2] >> 2));
        }
        if pad < 1 {
            out.push((values[2] << 6) | values[3]);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// The converter. ffmpeg is an environment fact: the probe is cached for the
// process (a binary does not appear mid-process), and every failure is
// reported as a sentence the route turns into §8's answer.
// ---------------------------------------------------------------------------

static FFMPEG_PROBES: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();

fn probe_cache() -> &'static Mutex<HashMap<String, Option<String>>> {
    FFMPEG_PROBES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `None` when the binary runs, `Some(sentence)` when it does not. Cached
/// per binary name for the process's lifetime: the probe answers a fact
/// about the environment, and re-probing every call would burn a process
/// spawn per photograph's audio.
pub async fn ffmpeg_unavailable(ffmpeg: &str) -> Option<String> {
    if let Some(cached) = probe_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(ffmpeg)
        .cloned()
    {
        return cached;
    }
    let outcome = tokio::process::Command::new(ffmpeg)
        .arg("-version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await;
    let problem = match outcome {
        Ok(output) if output.status.success() => None,
        Ok(output) => Some(format!(
            "'{ffmpeg} -version' exited {}: {}",
            output.status,
            first_line(&output.stderr)
        )),
        Err(error) => Some(format!(
            "cannot run '{ffmpeg}': {error} (install ffmpeg, or point media.ffmpeg at it)"
        )),
    };
    probe_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(ffmpeg.to_string(), problem.clone());
    problem
}

fn first_line(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

/// A temp directory that removes itself, whatever path the call takes.
struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

static SCRATCH_SEQ: AtomicU64 = AtomicU64::new(0);

fn scratch_dir() -> Result<Scratch, MediaToolError> {
    let seq = SCRATCH_SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("vadis-audio-{}-{seq}", std::process::id()));
    std::fs::create_dir_all(&path).map_err(|e| {
        MediaToolError::new(
            ErrorCode::Internal,
            format!(
                "the audio converter could not create its scratch directory '{}': {e}",
                path.display()
            ),
        )
    })?;
    Ok(Scratch(path))
}

async fn run_ffmpeg(ffmpeg: &str, args: &[&str]) -> Result<(), String> {
    let mut command = tokio::process::Command::new(ffmpeg);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    match tokio::time::timeout(std::time::Duration::from_secs(60), command.output()).await {
        Ok(Ok(output)) if output.status.success() => Ok(()),
        Ok(Ok(output)) => Err(format!(
            "'{ffmpeg}' exited {}: {}",
            output.status,
            first_line(&output.stderr)
        )),
        Ok(Err(error)) => Err(format!("cannot run '{ffmpeg}': {error}")),
        Err(_) => Err(format!("'{ffmpeg}' did not finish within 60 s")),
    }
}

/// [§4.22]: any audio container → the 16-bit PCM, mono, 16 kHz WAV the
/// provider's `asr` endpoint takes. Conversion failure is the caller's
/// `400`: the bytes are not audio, or not audio the converter reads. The
/// output is capped at 121 s by the converter and then checked against the
/// provider's 120 s bound, so an over-long clip is refused rather than
/// silently shortened.
pub async fn to_wav16k(
    ffmpeg: &str,
    input: &[u8],
    request_id: &str,
) -> Result<Vec<u8>, MediaToolError> {
    let scratch = scratch_dir()?;
    let source = scratch.0.join("input");
    let converted = scratch.0.join("output.wav");
    tokio::fs::write(&source, input).await.map_err(|e| {
        MediaToolError::new(
            ErrorCode::Internal,
            format!("the audio converter could not stage the request: {e}"),
        )
    })?;
    let source = source.to_string_lossy().to_string();
    let converted = converted.to_string_lossy().to_string();
    run_ffmpeg(
        ffmpeg,
        &[
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-y",
            "-i",
            &source,
            "-vn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-c:a",
            "pcm_s16le",
            "-t",
            "121",
            "-f",
            "wav",
            &converted,
        ],
    )
    .await
    .map_err(|e| {
        MediaToolError::invalid(format!(
            "the audio cannot be read as the declared container (request {request_id}): {e} \
             (spec §4.22: the converter accepts the containers ffmpeg reads)"
        ))
    })?;
    let wav = tokio::fs::read(&converted).await.map_err(|e| {
        MediaToolError::new(
            ErrorCode::Internal,
            format!("the audio converter produced no readable output: {e}"),
        )
    })?;
    if wav.is_empty() {
        return Err(MediaToolError::invalid(
            "the audio converted to zero bytes: there is nothing to transcribe (spec §4.22)",
        ));
    }
    match wav_seconds(&wav) {
        Some(seconds) if seconds > MAX_AUDIO_SECONDS => Err(MediaToolError::new(
            ErrorCode::RequestTooLarge,
            format!(
                "the audio is {seconds:.1} s, above the provider's {MAX_AUDIO_SECONDS:.0} s \
                 bound (spec §4.22): a transcript of the first two minutes presented as the \
                 file's transcript would be a lie, so the call is refused"
            ),
        )),
        Some(_) => Ok(wav),
        None => Err(MediaToolError::new(
            ErrorCode::Internal,
            "the audio converter's output is not a WAV: vadis cannot bound its duration \
             (spec §4.22)"
                .to_string(),
        )),
    }
}

/// [§4.22]: the provider's WAV → the `mp3` container a caller asked for.
pub async fn mp3_from_wav(
    ffmpeg: &str,
    wav: &[u8],
    request_id: &str,
) -> Result<Vec<u8>, MediaToolError> {
    let scratch = scratch_dir()?;
    let source = scratch.0.join("input.wav");
    let converted = scratch.0.join("output.mp3");
    tokio::fs::write(&source, wav).await.map_err(|e| {
        MediaToolError::new(
            ErrorCode::Internal,
            format!("the audio converter could not stage the answer: {e}"),
        )
    })?;
    let source = source.to_string_lossy().to_string();
    let converted = converted.to_string_lossy().to_string();
    run_ffmpeg(
        ffmpeg,
        &[
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-y",
            "-i",
            &source,
            "-codec:a",
            "libmp3lame",
            "-b:a",
            "64k",
            "-f",
            "mp3",
            &converted,
        ],
    )
    .await
    .map_err(|e| {
        MediaToolError::new(
            ErrorCode::Internal,
            format!("the mp3 transcode failed (request {request_id}): {e}"),
        )
    })?;
    let mp3 = tokio::fs::read(&converted).await.map_err(|e| {
        MediaToolError::new(
            ErrorCode::Internal,
            format!("the audio converter produced no readable mp3: {e}"),
        )
    })?;
    if mp3.is_empty() {
        return Err(MediaToolError::new(
            ErrorCode::Internal,
            "the mp3 transcode produced zero bytes".to_string(),
        ));
    }
    Ok(mp3)
}

/// A canonical PCM WAV's duration, from its own header: the `fmt ` chunk's
/// byte rate and the `data` chunk's length. `None` when the header is not
/// one this parser reads, which the caller treats as an internal failure
/// (the converter wrote it, so it should be readable).
pub fn wav_seconds(wav: &[u8]) -> Option<f64> {
    if wav.len() < 12 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return None;
    }
    let mut position = 12usize;
    let mut byte_rate: Option<f64> = None;
    let mut data_len: Option<usize> = None;
    while position + 8 <= wav.len() {
        let id = &wav[position..position + 4];
        let size = u32::from_le_bytes(wav[position + 4..position + 8].try_into().ok()?) as usize;
        let body = position + 8;
        if id == b"fmt " && size >= 12 && body + 12 <= wav.len() {
            byte_rate = Some(u32::from_le_bytes(wav[body + 8..body + 12].try_into().ok()?) as f64);
        } else if id == b"data" {
            data_len = Some(size.min(wav.len().saturating_sub(body)));
        }
        position = body.checked_add(size)?.checked_add(size % 2)?;
    }
    match (byte_rate, data_len) {
        (Some(rate), Some(length)) if rate > 0.0 => Some(length as f64 / rate),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64_request(bytes: &[u8]) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "audio": { "media_type": "audio/mp4", "base64": encode_base64(bytes) }
        }))
        .expect("the request serialises")
    }

    #[test]
    fn base64_matches_the_rfc_4648_vectors() {
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(encode_base64(plain.as_bytes()), encoded);
            assert_eq!(decode_base64(encoded).expect("decodes"), plain.as_bytes());
        }
    }

    #[test]
    fn base64_decoding_refuses_what_is_not_base64() {
        assert!(decode_base64("Zm9v!").is_err());
        assert!(decode_base64("Zm9").is_err());
        assert!(decode_base64("Zg==Zg==").is_err());
        assert!(decode_base64("Z===").is_err());
    }

    #[test]
    fn a_transcribe_request_needs_audio_and_its_bytes() {
        let bytes = vec![0u8, 1, 2, 3];
        let parsed = transcribe_request(&b64_request(&bytes)).expect("parses");
        assert_eq!(parsed.media_type, "audio/mp4");
        assert_eq!(parsed.bytes, bytes);
        assert_eq!(parsed.language, None);

        let missing = br#"{"audio": {"media_type": "audio/wav"}}"#;
        assert!(transcribe_request(missing)
            .expect_err("refuses")
            .message
            .contains("base64"));

        let image = br#"{"audio": {"media_type": "image/png", "base64": "Zm9v"}}"#;
        assert!(transcribe_request(image)
            .expect_err("refuses")
            .message
            .contains("audio/*"));

        let bad = br#"{"audio": {"media_type": "audio/wav", "base64": "!!"}}"#;
        assert_eq!(
            transcribe_request(bad).expect_err("refuses").code,
            ErrorCode::InvalidRequest
        );
    }

    #[test]
    fn a_transcribe_request_carries_a_language_tag_only() {
        let bytes = vec![0u8; 4];
        let mut body: Value = serde_json::from_slice(&b64_request(&bytes)).expect("json");
        body["language"] = json!("zh-CN");
        let parsed = transcribe_request(&serde_json::to_vec(&body).expect("json")).expect("parses");
        assert_eq!(parsed.language.as_deref(), Some("zh-CN"));

        body["language"] = json!("a language, please");
        assert!(
            transcribe_request(&serde_json::to_vec(&body).expect("json"))
                .expect_err("refuses")
                .message
                .contains("language tag")
        );
    }

    #[test]
    fn a_synthesize_request_defaults_to_mp3() {
        let parsed = synthesize_request(br#"{"text": "hello"}"#).expect("parses");
        assert_eq!(parsed.format, SpeechFormat::Mp3);
        assert_eq!(parsed.voice, None);

        let explicit =
            synthesize_request(br#"{"text": "hello", "format": "pcm", "voice": "tide"}"#)
                .expect("parses");
        assert_eq!(explicit.format, SpeechFormat::Pcm);
        assert_eq!(explicit.voice.as_deref(), Some("tide"));

        let unknown =
            synthesize_request(br#"{"text": "hello", "format": "flac"}"#).expect_err("refuses");
        assert!(unknown.message.contains("mp3, wav, pcm"));

        let blank = synthesize_request(br#"{"text": "   "}"#).expect_err("refuses");
        assert!(blank.message.contains("text"));
    }

    #[test]
    fn the_tts_body_speaks_the_providers_own_format() {
        let mp3 = synthesize_request(br#"{"text": "hello"}"#).expect("parses");
        let body: Value = serde_json::from_slice(&tts_body(&mp3)).expect("json");
        assert_eq!(body["text"], "hello");
        assert_eq!(body["format"], "wav");
        assert_eq!(body["voice_id"], DEFAULT_TTS_VOICE);

        let pcm = synthesize_request(br#"{"text": "hello", "format": "pcm"}"#).expect("parses");
        let body: Value = serde_json::from_slice(&tts_body(&pcm)).expect("json");
        assert_eq!(body["format"], "pcm");
    }

    #[test]
    fn the_multipart_asr_body_wraps_one_file_part() {
        let wav = b"RIFF____WAVE";
        let body = multipart_audio(wav, "vadis-req-1", Some("zh-CN"));
        let text = String::from_utf8_lossy(&body);
        assert!(text.starts_with("--vadis-req-1\r\n"));
        assert!(text.contains("name=\"file\"; filename=\"audio.wav\""));
        assert!(text.contains("Content-Type: audio/wav"));
        assert!(text.contains("name=\"language\"\r\n\r\nzh-CN"));
        assert!(text.ends_with("--vadis-req-1--\r\n"));
        assert!(body.windows(wav.len()).any(|window| window == wav));
    }

    #[test]
    fn a_transcript_answer_is_the_callers_json() {
        assert_eq!(
            transcribe_answer("你好"),
            serde_json::to_vec(&json!({ "text": "你好" })).expect("json")
        );
        assert_eq!(
            transcribe_text(br#"{"request_id": "x", "text": " hi "}"#).expect("parses"),
            "hi"
        );
        assert!(matches!(
            transcribe_text(br#"{"text": ""}"#)
                .expect_err("refuses")
                .code,
            ErrorCode::UpstreamError
        ));
    }

    #[test]
    fn a_synthesize_answer_carries_base64_and_its_media_type() {
        let answer = synthesize_answer(b"ID3", "audio/mpeg");
        let parsed: Value = serde_json::from_slice(&answer).expect("json");
        assert_eq!(parsed["audio"]["base64"], "SUQz");
        assert_eq!(parsed["audio"]["media_type"], "audio/mpeg");
    }

    fn tiny_wav(seconds: f64, sample_rate: u32) -> Vec<u8> {
        let data_len = (sample_rate as f64 * 2.0 * seconds) as usize;
        let mut wav = Vec::with_capacity(44 + data_len);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data_len as u32).to_le_bytes());
        wav.resize(44 + data_len, 0);
        wav
    }

    #[test]
    fn a_wav_header_yields_its_duration() {
        assert_eq!(wav_seconds(&tiny_wav(2.0, 16_000)), Some(2.0));
        assert_eq!(wav_seconds(b"not a wav"), None);
    }

    #[tokio::test]
    async fn the_converter_round_trips_a_wav_when_ffmpeg_is_installed() {
        if ffmpeg_unavailable("ffmpeg").await.is_some() {
            return;
        }
        let input = tiny_wav(1.0, 16_000);
        let converted = to_wav16k("ffmpeg", &input, "req-test")
            .await
            .expect("converts");
        assert_eq!(&converted[0..4], b"RIFF");
        let seconds = wav_seconds(&converted).expect("a wav");
        assert!((seconds - 1.0).abs() < 0.1, "got {seconds}");

        let mp3 = mp3_from_wav("ffmpeg", &converted, "req-test")
            .await
            .expect("transcodes");
        assert!(mp3.len() > 100);
    }

    #[tokio::test]
    async fn the_converter_refuses_bytes_that_are_not_audio() {
        if ffmpeg_unavailable("ffmpeg").await.is_some() {
            return;
        }
        let error = to_wav16k("ffmpeg", b"this is not audio", "req-test")
            .await
            .expect_err("refuses");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }

    #[tokio::test]
    async fn a_missing_converter_is_a_sentence_not_a_panic() {
        let problem = ffmpeg_unavailable("/nonexistent/vadis-ffmpeg").await;
        assert!(problem.is_some());
        assert!(problem.expect("sentence").contains("vadis-ffmpeg"));
    }
}
