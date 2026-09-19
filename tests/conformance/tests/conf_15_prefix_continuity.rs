//! CONF-15：同 session 两轮 passthrough，`prefix_continuity == 1.0`。

#![forbid(unsafe_code)]

#[ignore = "CONF-15: 依赖 cache 账本 + 粘性表（R2）"]
#[tokio::test]
async fn conf_15_prefix_continuity() {
    unimplemented!("cache ledger + sticky table land in Round 2");
}
