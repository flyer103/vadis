//! CONF-16: re-run the cache regression with each transform enabled
//! individually, not below baseline (parameterized per plugin).

#![forbid(unsafe_code)]

#[ignore = "CONF-16: depends on the transform chain + ledger (R2)"]
#[tokio::test]
async fn conf_16_transform_cache_regression() {
    unimplemented!("transform chain + cache ledger land in Round 2");
}
