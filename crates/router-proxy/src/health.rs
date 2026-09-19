use router_core::error::{ErrorBody, ErrorCode};
use serde_json::{json, Value};

/// `/health` response body (spec: liveness + loaded plugins/services).
/// R1-2 placeholder: config parsing and plugin loading land in later rounds;
/// this only pins a stable output shape.
#[derive(Clone, Default)]
pub struct AppState {
    pub addr: String,
    pub plugins: Vec<String>,
}

pub fn health_json(state: &AppState) -> Value {
    json!({
        "status": "ok",
        "addr": state.addr,
        "plugins": state.plugins,
    })
}

pub(crate) fn not_implemented_body(request_id: String, endpoint: &'static str) -> ErrorBody {
    ErrorBody::new(
        ErrorCode::NotImplemented,
        format!("endpoint {endpoint} is a stub in this build; forwarding lands in Round 2"),
        request_id,
    )
}
