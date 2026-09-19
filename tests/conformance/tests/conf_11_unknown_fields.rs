//! CONF-11：顶层与嵌套未知字段原样透传。

#![forbid(unsafe_code)]

#[ignore = "CONF-11: 依赖 RawBody + 编码器（R2）"]
#[tokio::test]
async fn conf_11_unknown_fields_passthrough() {
    unimplemented!("RawBody + encoder lands in Round 2");
}
