//! 三个协议端点的统一桩：稳定 501 + 明确 JSON body，绝不 panic（R1-2 契约）。
//! 实际转发（字节保真 + SSE 透传）在 R2 落地（DESIGN §2/§7）。

use router_core::error::ErrorCode;

pub fn protocol_stub(
    request_id: String,
    endpoint: &'static str,
) -> (u16, router_core::error::ErrorBody) {
    (
        ErrorCode::NotImplemented.http_status(),
        super::health::not_implemented_body(request_id, endpoint),
    )
}
