//! CONF-14: upstream 400/401/429/5xx → normalized error body; 5xx/429 trigger
//! fallback and record `failover_from`.

#![forbid(unsafe_code)]

#[ignore = "CONF-14: depends on proxy + guard/fallback (R2)"]
#[tokio::test]
async fn conf_14_error_mapping_fallback() {
    unimplemented!("proxy + fallback chain lands in Round 2");
}
