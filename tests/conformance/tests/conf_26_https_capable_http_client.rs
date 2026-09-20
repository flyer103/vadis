//! CONF-26 (invariant, not a change detector): the HTTP client must be built
//! with a TLS backend.
//!
//! Every real provider is `https://`; an http-only `reqwest` fails every
//! upstream call, while every offline conformance mock is plain `http://` and
//! therefore passes. That is exactly how an http-only client shipped unnoticed
//! (a 2026-09-20 live client smoke). The invariant asserted here is "the client can
//! speak TLS", expressed against the one place that decides it: the workspace
//! dependency's feature set.

#![forbid(unsafe_code)]

const WORKSPACE_MANIFEST: &str = include_str!("../../../Cargo.toml");

#[test]
fn conf_26_reqwest_has_a_tls_backend() {
    let line = WORKSPACE_MANIFEST
        .lines()
        .find(|l| l.trim_start().starts_with("reqwest ="))
        .expect("the workspace manifest must declare reqwest");

    assert!(
        [
            "rustls-tls",
            "native-tls",
            "rustls-tls-native-roots",
            "rustls-tls-webpki-roots"
        ]
        .iter()
        .any(|f| line.contains(f)),
        "reqwest is declared without a TLS feature, so the router cannot reach any real \
         (https) provider: {line}"
    );
    assert!(
        !line.contains("default-features = true"),
        "reqwest's default features must stay off (they pull macos-system-configuration, which \
         makes the client follow the macOS system proxy with no opt-out): {line}"
    );
}
