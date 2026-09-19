//! CONF-01 (DESIGN §10 conformance · fidelity): chat inbound → `wire_api:
//! chat` native passthrough; the upstream-visible body is byte-identical to
//! the client body.

#![forbid(unsafe_code)]

#[ignore = "CONF-01: depends on the router-protocol native path (R2)"]
#[tokio::test]
async fn conf_01_native_chat_passthrough() {
    unimplemented!("router-protocol native passthrough lands in Round 2");
}
