//! CONF-18：真实 codex/hermes 各一轮接入冒烟（含 `NO_PROXY` 前提验证）。手动/带网络的 CI 专属。

#![forbid(unsafe_code)]

#[ignore = "CONF-18: 依赖端到端数据面 + 真实客户端（带网络，手动执行）"]
#[tokio::test]
async fn conf_18_live_client_smoke() {
    unimplemented!("end-to-end smoke with real clients; run manually with network access");
}
