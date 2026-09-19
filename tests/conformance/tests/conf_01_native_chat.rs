//! CONF-01（DESIGN §10 conformance·保真）：chat 入站 → `wire_api: chat` native passthrough，
//! 上游可见 body 与客户端 body 逐字节相同。

#![forbid(unsafe_code)]

#[ignore = "CONF-01: 依赖 router-protocol native 路径（R2）"]
#[tokio::test]
async fn conf_01_native_chat_passthrough() {
    unimplemented!("router-protocol native passthrough lands in Round 2");
}
