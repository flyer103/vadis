//! axum data plane: byte-faithful forwarding and SSE passthrough (DESIGN §2).

#![forbid(unsafe_code)]

mod accounting;
mod auth;
mod availability;
mod forward;
mod health;
mod stream_forward;
mod stubs;

pub use accounting::{
    quota_plan_from_cfg, route_accounting, trace_error_for_failure, AccountCtx, AccountResult,
    Accountant, RouteAccounting,
};
pub use auth::{refused_message, refused_record, AuthGate, AuthVerdict};
pub use forward::{
    mode_refused_record, resolve_transform_mode, BoxedAttempt, ForwardFailure, ForwardOutcome,
    ForwardSuccess, Forwarder, ProviderSend, ProviderTransport,
};
pub use health::{health_json, AppState, ProviderKeyFacts};
pub use stream_forward::{StreamOutcome, StreamSuccess};
pub use stubs::protocol_stub;
