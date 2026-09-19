//! CONF-11: unknown top-level and nested fields pass through verbatim.

#![forbid(unsafe_code)]

#[ignore = "CONF-11: depends on RawBody + encoder (R2)"]
#[tokio::test]
async fn conf_11_unknown_fields_passthrough() {
    unimplemented!("RawBody + encoder lands in Round 2");
}
