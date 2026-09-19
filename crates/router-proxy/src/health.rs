use router_core::error::{ErrorBody, ErrorCode};
use serde_json::{json, Value};

/// `/health` 响应体（spec：存活 + 已装载插件/服务）。
/// R1-2 占位：config 解析与插件装载在后续轮次接入，这里只稳定输出形状。
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
