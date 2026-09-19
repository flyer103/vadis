//! axum 数据面：字节保真转发与 SSE 透传（DESIGN §2）。R1-2 只含 serve 桩的端点表。

#![forbid(unsafe_code)]

mod health;
mod stubs;

pub use health::{health_json, AppState};
pub use stubs::protocol_stub;
