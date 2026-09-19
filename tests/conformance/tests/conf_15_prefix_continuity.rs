//! CONF-15: two passthrough turns in the same session, `prefix_continuity ==
//! 1.0`.

#![forbid(unsafe_code)]

#[ignore = "CONF-15: depends on the cache ledger + sticky table (R2)"]
#[tokio::test]
async fn conf_15_prefix_continuity() {
    unimplemented!("cache ledger + sticky table land in Round 2");
}
