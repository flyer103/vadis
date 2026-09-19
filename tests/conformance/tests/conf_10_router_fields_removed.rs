//! CONF-10：删 router 自有字段后其余字节与客户端逐字节相同；`router_meta` 回显不进上游。

#![forbid(unsafe_code)]

#[ignore = "CONF-10: 依赖 RawBody::remove_top_level_keys 单遍 span 扫描（R2）"]
#[tokio::test]
async fn conf_10_router_fields_removed_byte_exact() {
    unimplemented!("RawBody span-preserving removal lands in Round 2");
}
