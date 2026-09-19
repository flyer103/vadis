//! CONF-12: tool-call round trip across the three protocols (chat
//! `tool_calls` ↔ responses `function_call*` ↔ anthropic `tool_use/result`),
//! ids and order preserved.

#![forbid(unsafe_code)]

#[ignore = "CONF-12: depends on the translation matrix"]
#[tokio::test]
async fn conf_12_tool_call_roundtrip() {
    unimplemented!("the cross-protocol translation matrix is not implemented in v0.1");
}
