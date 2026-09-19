//! Unified stub for a protocol endpoint that is not served: a stable 501 + a
//! clear JSON body, never a panic. Real forwarding (byte fidelity + SSE
//! passthrough) is the buffered / streaming engine's job (DESIGN §2/§7).

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
