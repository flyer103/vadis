//! The CONF-120 test double: a stand-in client that consumes **the client's
//! own header mechanism** exactly the way the real client does, so the case
//! can drive `vadis wrap`'s wiring without an out-of-repository binary.
//!
//! Four behaviours, selected by `argv[0]`'s basename (what the process is
//! *exec'd as* — this is how the double recognises the exec `wrap` performs)
//! falling back to `WRAP_DOUBLE_ROLE` when invoked under its own name:
//!
//! - **as `codex`** — scans argv for the `-c model_providers.<id>.
//!   env_http_headers={"X-Vadis-Transform"="<ENV>"}` override `wrap`
//!   injects, reads that env var at request time (codex's own semantics),
//!   and sends its value as the header;
//! - **as `claude`** — reads `ANTHROPIC_CUSTOM_HEADERS` (`Name: Value`,
//!   comma separated), the surface Claude Code itself reads, and extracts
//!   the `X-Vadis-Transform` entry if present;
//! - `WRAP_DOUBLE_ROLE=wrap-codex` / `wrap-claude` — becomes a launcher:
//!   calls the real `vadis_cli::wrap::run`, which **execs** the client
//!   (found via the PATH shim), turning this process into the `codex`/
//!   `claude` behaviour above. This must run in its own process precisely
//!   because exec replaces the image — the case never calls `wrap::run`
//!   in-process on the success arms.
//!
//! In every client behaviour the double POSTs one request to
//! `WRAP_DOUBLE_ADDR` at `/v1/chat/completions` with `WRAP_DOUBLE_BODY`,
//! sending the header only if the mechanism supplied one, and prints the
//! status. A double that merely checked "the env var was set" would assert
//! plumbing; this double makes the request, so the trace record — not the
//! double — is the witness.

#![forbid(unsafe_code)]

use vadis_conformance::testkit;

fn main() {
    let argv0 = std::env::args().next().unwrap_or_default();
    let invoked_as = argv0.rsplit('/').next().unwrap_or_default().to_string();
    let role = match invoked_as.as_str() {
        "codex" => "codex".to_string(),
        "claude" => "claude".to_string(),
        _ => std::env::var("WRAP_DOUBLE_ROLE").unwrap_or_default(),
    };
    match role.as_str() {
        "wrap-codex" => std::process::exit(vadis_cli::wrap::run("codex", &[])),
        "wrap-claude" => std::process::exit(vadis_cli::wrap::run("claude", &[])),
        "codex" => client(codex_header()),
        "claude" => client(claude_header()),
        other => panic!("wrap double: unknown role {other}"),
    }
}

/// Makes the one request, the header riding only if the client mechanism
/// supplied it; prints the status.
fn client(header: Option<String>) {
    let addr = std::env::var("WRAP_DOUBLE_ADDR").expect("WRAP_DOUBLE_ADDR");
    let body = std::env::var("WRAP_DOUBLE_BODY").expect("WRAP_DOUBLE_BODY");
    let headers: Vec<(&str, &str)> = match header.as_deref() {
        Some(v) => vec![("x-vadis-transform", v)],
        None => Vec::new(),
    };
    let (status, _body, _h) =
        testkit::http_post(&addr, "/v1/chat/completions", body.as_bytes(), &headers);
    println!("{status}");
}

/// Claude Code's mechanism: `ANTHROPIC_CUSTOM_HEADERS`.
fn claude_header() -> Option<String> {
    let raw = std::env::var("ANTHROPIC_CUSTOM_HEADERS").ok()?;
    raw.split(',').find_map(|entry| {
        let (name, value) = entry.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("x-vadis-transform")
            .then(|| value.trim().to_string())
    })
}

/// codex's mechanism: the `-c … env_http_headers={…}` override on argv,
/// whose table maps the header name to the env var read at request time.
fn codex_header() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    for (i, arg) in args.iter().enumerate() {
        if arg != "-c" {
            continue;
        }
        let Some(v) = args.get(i + 1) else { continue };
        let Some(rest) = v.strip_prefix("model_providers.") else {
            continue;
        };
        let Some((_provider, table)) = rest.split_once(".env_http_headers=") else {
            continue;
        };
        let inner = table.trim().trim_start_matches('{').trim_end_matches('}');
        for pair in inner.split(',') {
            let Some((k, env_name)) = pair.split_once('=') else {
                continue;
            };
            let name_matches = k
                .trim()
                .trim_matches('"')
                .eq_ignore_ascii_case("x-vadis-transform");
            if name_matches {
                // codex reads the value at request time, not at config time.
                return std::env::var(env_name.trim().trim_matches('"')).ok();
            }
        }
    }
    None
}
