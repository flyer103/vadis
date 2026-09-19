//! CONF-13: native streaming SSE passes through event-by-event equivalently;
//! end-event semantics preserved.

#![forbid(unsafe_code)]

#[ignore = "CONF-13: depends on router-proxy SSE passthrough (R2)"]
#[tokio::test]
async fn conf_13_sse_passthrough() {
    unimplemented!("router-proxy SSE passthrough lands in Round 2");
}
