//! axum data plane: byte-faithful forwarding and SSE passthrough (DESIGN §2).

#![forbid(unsafe_code)]

mod accounting;
mod auth;
mod availability;
mod body_limit;
mod forward;
mod health;
mod revision;
mod stream_forward;
mod stubs;

pub use accounting::{
    quota_plan_from_cfg, route_accounting, trace_error_for_failure, AccountCtx, AccountResult,
    Accountant, RouteAccounting,
};
pub use auth::{refused_message, refused_record, AuthGate, AuthVerdict};
pub use body_limit::{check_declared, too_large_message, too_large_record};
pub use forward::{
    mode_refused_record, resolve_transform_mode, BoxedAttempt, ForwardFailure, ForwardOutcome,
    ForwardSuccess, Forwarder, ProviderSend, ProviderTransport,
};
pub use health::{health_json, AppState, ConfigIdentity, KeyFact, ProviderKeyFacts};
pub use revision::{Revision, RevisionCell, SharedRevision};
pub use stream_forward::{StreamOutcome, StreamSuccess};
pub use stubs::protocol_stub;
