//! CONF-13：native 流式 SSE 逐事件等价透传；结束事件语义保留。

#![forbid(unsafe_code)]

#[ignore = "CONF-13: 依赖 router-proxy SSE 透传（R2）"]
#[tokio::test]
async fn conf_13_sse_passthrough() {
    unimplemented!("router-proxy SSE passthrough lands in Round 2");
}
