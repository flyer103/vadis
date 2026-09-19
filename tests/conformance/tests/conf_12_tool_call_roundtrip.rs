//! CONF-12：tool call 三协议往返（chat `tool_calls` ↔ responses `function_call*` ↔ anthropic `tool_use/result`），id 与顺序保留。

#![forbid(unsafe_code)]

#[ignore = "CONF-12: 依赖翻译矩阵（R2）"]
#[tokio::test]
async fn conf_12_tool_call_roundtrip() {
    unimplemented!("translation matrix lands in Round 2");
}
