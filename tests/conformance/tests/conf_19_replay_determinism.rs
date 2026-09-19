//! CONF-19：同一 trace 两次 `router replay` 的成本/缓存报表逐字段一致。

#![forbid(unsafe_code)]

#[ignore = "CONF-19: 依赖 replay 子命令（R2+）"]
#[tokio::test]
async fn conf_19_replay_determinism() {
    unimplemented!("replay subcommand lands after Round 2");
}
