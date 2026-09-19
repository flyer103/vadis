//! axum data plane: byte-faithful forwarding and SSE passthrough (DESIGN §2).
//! R1-2 contains only the endpoint table for the serve stub.

#![forbid(unsafe_code)]

mod forward;
mod health;
mod stream_forward;
mod stubs;

pub use forward::{
    BoxedAttempt, ForwardFailure, ForwardOutcome, ForwardSuccess, Forwarder, ProviderSend,
    ProviderTransport,
};
pub use health::{health_json, AppState};
pub use stream_forward::{StreamOutcome, StreamSuccess};
pub use stubs::protocol_stub;
