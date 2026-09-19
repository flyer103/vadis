//! CONF-10: after removing router-owned fields, all remaining bytes are
//! byte-identical to the client's; the `router_meta` echo never reaches the
//! upstream.
//!
//! The byte-level assertions already landed as **really executed** cases (the
//! three below) driving `router_core::RawBody::remove_top_level_keys`
//! (ADR-007 single-pass span scan; DESIGN §12.3.1). The remaining assertions
//! of the full semantics (router_meta echo never reaching the upstream = a
//! proxy-chain-level check that depends on R2's forwarding path) are still
//! `#[ignore]`d and will be appended here when that lands.

#![forbid(unsafe_code)]

use router_core::{RawBody, ROUTER_OWNED_TOP_LEVEL_KEYS};

/// Main case: a realistic client request (escapes, multi-byte UTF-8, nested
/// structure, trailing newline); after deleting the whitelisted key, the
/// output is **byte-identical** except for the deleted member and its
/// separator comma.
#[tokio::test]
async fn conf_10_byte_exact_after_router_field_removal() {
    // The trailing \n stays in the literal on purpose: trailing newlines
    // must pass through verbatim.
    let input = "{\
        \"model\": \"provider/model\",\
        \"messages\": [{\"role\": \"system\", \"content\": \"a{b},\\\"c\\\\\\\"\\\\ud83d\\\\ude00\"}],\
        \"tools\": [{\"x\": [1, {\"y\": \"},]\"}]}],\
        \"temperature\": 1e-9,\
        \"router_meta\": {\"echo\": true, \"nested\": [{\"k\": \"v\"}]},\
        \"stream\": true\
    }\n";
    let raw = RawBody::new(input.as_bytes().to_vec());

    let out = raw
        .remove_top_level_keys(ROUTER_OWNED_TOP_LEVEL_KEYS)
        .expect("well-formed body must succeed");

    // Expected = the same input minus the router_meta member + its leading
    // comma; every other byte untouched.
    let expected = "{\
        \"model\": \"provider/model\",\
        \"messages\": [{\"role\": \"system\", \"content\": \"a{b},\\\"c\\\\\\\"\\\\ud83d\\\\ude00\"}],\
        \"tools\": [{\"x\": [1, {\"y\": \"},]\"}]}],\
        \"temperature\": 1e-9,\
        \"stream\": true\
    }\n";
    assert_eq!(out.as_bytes(), expected.as_bytes());

    // Semantic cross-check: router_meta is gone, every other key is present.
    let v: serde_json::Value = serde_json::from_slice(out.as_bytes()).unwrap();
    assert!(v.get("router_meta").is_none());
    assert_eq!(v["model"], "provider/model");
    assert_eq!(v["stream"], true);
}

/// With no matching key it is a strict no-op: output **byte-identical** to
/// input (including all whitespace).
#[tokio::test]
async fn conf_10_noop_when_no_router_fields_present() {
    let input = "{\n  \"model\": \"m\",\n  \"n\": [1, 2, {\"deep\": \"},\"}]\n}\r\n";
    let raw = RawBody::new(input.as_bytes().to_vec());
    let out = raw
        .remove_top_level_keys(ROUTER_OWNED_TOP_LEVEL_KEYS)
        .expect("well-formed body must succeed");
    assert_eq!(out.as_bytes(), input.as_bytes());
}

/// Idempotent: two consecutive removals == one removal (a corollary of
/// AGENTS hard constraint 2's content determinism).
#[tokio::test]
async fn conf_10_removal_is_idempotent() {
    let input = "{\"a\":1,\"router_meta\":{\"b\":[2,{\"c\":\"},\"}],\"d\":null},\"e\":true}";
    let raw = RawBody::new(input.as_bytes().to_vec());
    let once = raw
        .remove_top_level_keys(ROUTER_OWNED_TOP_LEVEL_KEYS)
        .unwrap();
    let twice = once
        .remove_top_level_keys(ROUTER_OWNED_TOP_LEVEL_KEYS)
        .unwrap();
    assert_eq!(once.as_bytes(), twice.as_bytes());
    assert_eq!(once.as_bytes(), b"{\"a\":1,\"e\":true}");
}

/// The proxy-chain-level assertion (the router_meta echo is injected on the
/// response side and never enters the upstream request) depends on R2's
/// forwarding path; the byte-level removal semantics are already covered by
/// the three really-executed cases above.
#[ignore = "CONF-10 (chain-level): depends on the R2 forwarding path — router_meta echo is injected by the proxy response side"]
#[tokio::test]
async fn conf_10_router_meta_echo_never_reaches_upstream() {
    unimplemented!("proxy forwarding path lands in Round 2");
}
