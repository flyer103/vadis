//! CONF-16: re-run the cache regression with each transform enabled
//! individually, not below baseline (parameterized per plugin).
//!
//! Still `#[ignore]`d by design, with the reason stated rather than
//! glossed: the v0.1 transform chain does not exist yet (this round's
//! chain is passthrough; the chain itself lands in R2-3). There is no
//! transform to enable, so "each transform individually" has an empty
//! parameter list — a test body here would be vacuously green
//! (constraint: never write an always-true test). The honest object that
//! EXISTS today is the empty chain's own prefix behavior, which CONF-15
//! already measures end to end (two-turn passthrough continuity 1.0).
//! Un-ignore when the first real transform lands.

#![forbid(unsafe_code)]

#[ignore = "CONF-16: no transform exists to enable until R2-3 (empty chain measured by CONF-15); enabling this with a vacuous body would be an always-true test"]
#[tokio::test]
async fn conf_16_transform_cache_regression() {
    unimplemented!("re-enable when the first real transform lands (R2-3)")
}
