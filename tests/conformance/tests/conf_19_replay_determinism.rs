//! CONF-19: two `vadis replay` runs over the same trace produce
//! field-identical cost/cache reports.

#![forbid(unsafe_code)]

#[ignore = "CONF-19: depends on the replay subcommand"]
#[tokio::test]
async fn conf_19_replay_determinism() {
    unimplemented!("the replay subcommand is not implemented in v0.1");
}
