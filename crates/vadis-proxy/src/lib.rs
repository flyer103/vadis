//! axum data plane: byte-faithful forwarding and SSE passthrough (DESIGN §2).

#![forbid(unsafe_code)]

mod accounting;
mod auth;
mod availability;
mod body_limit;
mod cancel;
mod forward;
mod health;
mod media_audio;
mod media_tools;
mod models_read;
mod revision;
mod state_read;
mod stream_forward;
mod stubs;
mod tools_surface;
mod upstream_body;

pub use accounting::{
    quota_plan_from_cfg, route_accounting, trace_error_for_failure, AccountCtx, AccountResult,
    Accountant, RouteAccounting,
};
pub use auth::{refused_message, refused_record, AuthGate, AuthVerdict};
pub use body_limit::{check_declared, too_large_message, too_large_record, tool_refused_record};
pub use cancel::set_shutdown_draining;
pub use forward::{
    mode_refused_record, resolve_transform_mode, BoxedAttempt, ForwardFailure, ForwardOutcome,
    ForwardSuccess, Forwarder, ProviderSend, ProviderTransport,
};
pub use health::{health_json, AppState, ConfigIdentity, KeyFact, ProviderKeyFacts};
pub use media_audio::{
    boundary_for, decode_base64, encode_base64, ffmpeg_unavailable, mp3_from_wav, multipart_audio,
    synthesize_answer, synthesize_request, to_wav16k, transcribe_answer, transcribe_request,
    transcribe_text, tts_body, SpeechFormat, SynthesizeRequest, TranscribeRequest,
    DEFAULT_TTS_VOICE, MAX_ASR_BYTES, MAX_AUDIO_SECONDS, MAX_TTS_CHARS, SPEECH_FORMATS,
};
pub use media_tools::{ocr_chat_body, ocr_text, MediaToolError, DEFAULT_OCR_PROMPT};
pub use models_read::{models_body, models_body_from_config};
pub use revision::{Revision, RevisionCell, SharedRevision};
pub use state_read::{
    count_unknown_outcomes, handle as state_events_handle, parse_window, state_events_json,
    StateWindow, WINDOW_MAX_MS, WINDOW_MIN_MS,
};
pub use stream_forward::{StreamOutcome, StreamSuccess};
pub use stubs::protocol_stub;
pub use tools_surface::{tool_record, ToolCallFacts, ToolSurface, ToolsMount};
