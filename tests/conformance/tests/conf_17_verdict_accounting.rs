//! CONF-17: every `TransformRecord.verdict ∈ {Verified, Inferred}` and gates
//! read only `Verified`.

#![forbid(unsafe_code)]

#[ignore = "CONF-17: depends on the accounting implementation (R2)"]
#[tokio::test]
async fn conf_17_verdict_accounting() {
    unimplemented!("accounting implementation lands in Round 2");
}
