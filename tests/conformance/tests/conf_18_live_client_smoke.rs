//! CONF-18: live one-round smoke with real codex/hermes clients (including
//! the `NO_PROXY` precondition check). Manual / networked-CI only.

#![forbid(unsafe_code)]

#[ignore = "CONF-18: depends on the end-to-end data plane + real clients (networked, manual run)"]
#[tokio::test]
async fn conf_18_live_client_smoke() {
    unimplemented!("end-to-end smoke with real clients; run manually with network access");
}
