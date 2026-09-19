//! CONF-14：上游 400/401/429/5xx → 归一错误体；5xx/429 触发 fallback 并记 `failover_from`。

#![forbid(unsafe_code)]

#[ignore = "CONF-14: 依赖 proxy + guard/fallback（R2）"]
#[tokio::test]
async fn conf_14_error_mapping_fallback() {
    unimplemented!("proxy + fallback chain lands in Round 2");
}
