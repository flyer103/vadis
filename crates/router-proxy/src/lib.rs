//! axum data plane: byte-faithful forwarding and SSE passthrough (DESIGN §2).
//! R1-2 contains only the endpoint table for the serve stub.

#![forbid(unsafe_code)]

mod health;
mod stubs;

pub use health::{health_json, AppState};
pub use stubs::protocol_stub;
