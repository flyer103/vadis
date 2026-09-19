//! CONF-19: two `router replay` runs over the same trace produce
//! field-identical cost/cache reports.

#![forbid(unsafe_code)]

#[ignore = "CONF-19: depends on the replay subcommand (R2+)"]
#[tokio::test]
async fn conf_19_replay_determinism() {
    unimplemented!("replay subcommand lands after Round 2");
}
