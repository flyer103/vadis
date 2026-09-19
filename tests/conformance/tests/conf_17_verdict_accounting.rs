//! CONF-17：每条 `TransformRecord.verdict ∈ {Verified, Inferred}` 且 gate 只读 `Verified`。

#![forbid(unsafe_code)]

#[ignore = "CONF-17: 依赖计账实现（R2）"]
#[tokio::test]
async fn conf_17_verdict_accounting() {
    unimplemented!("accounting implementation lands in Round 2");
}
