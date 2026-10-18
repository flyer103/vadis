//! `vadis wrap <client> -- <client args…>` — the launcher that gives the
//! transform lever (spec §2.1, ADR-019) its trigger.
//!
//! Why a launcher exists when the contract says "the client asks": the
//! transform mode is opt-in **by request header** and stays that way (ADR-019;
//! `book/plugins.md`: *there is no config key*). But no shipped client sends
//! `X-Vadis-Transform` — measured on 14 days of the owner's own traffic,
//! 8,461 decision records, `transforms[]` non-empty in 0 — so the lever is
//! finished and unreachable. `wrap` closes exactly that gap and nothing else:
//! it starts the client with the header supplied **by the client's own
//! configuration mechanism**, so the request that reaches vadis asks for the
//! mode in the one way the contract admits. vadis still edits nothing unless
//! the client asked; `wrap` makes the client ask, it never makes vadis
//! decide. No vadis-side switch exists; the user's real client config is
//! never written.
//!
//! The mechanism per client, recorded with its evidence (2026-10-18, both
//! verified live against a loopback mock that logs request headers):
//!
//! - **claude** (Claude Code 2.1.300) — the documented env var
//!   `ANTHROPIC_CUSTOM_HEADERS` (`Name: Value`, comma-separated). Pure
//!   environment: no file, nothing to restore. An existing value is
//!   preserved and the vadis header appended (idempotently: a value that
//!   already names `X-Vadis-Transform` is left alone).
//! - **codex** (codex-cli 0.163.0) — no env var of its own carries a request
//!   header; the provider entry's `env_http_headers` map does (header name →
//!   env var name, the value read at request time). `wrap` therefore passes
//!   the wiring as `-c` argument overrides — `model_providers.<id>.
//!   env_http_headers={"X-Vadis-Transform"="VADIS_TRANSFORM"}` — and sets
//!   `VADIS_TRANSFORM=transform` in the child's environment. No file is
//!   written and the user's `config.toml` is only ever **read** (once, to
//!   learn the provider id from the top-level `model_provider` key;
//!   `$CODEX_HOME` respected). `wrap`'s overrides come first, so the user's
//!   own later `-c` on the same key still wins.
//!
//! Process semantics: `wrap` **execs** the client (it replaces its own
//! process image) rather than spawning a child. Everything the interactive
//! client needs then holds by construction: stdin/stdout/stderr are
//! untouched (nobody sat between them), the client stays in this process
//! group, a signal delivered to the parent *is* delivered to the client
//! (same pid), the client cannot be orphaned (there is no second process),
//! and its exit code is the process's own exit code — forwarded exactly.
//!
//! Refusals (exit `2`, the CLI's convention): an unknown client name (the
//! message lists the supported set) and a codex config that names no
//! top-level `model_provider` (with a builtin default there is no provider
//! entry to hang the header on — refusing beats a silent no-header run).

#![forbid(unsafe_code)]

use std::os::unix::process::CommandExt;
use std::process::Command as StdCommand;

/// The request header that opts a request into transform mode (spec §2.1).
const TRANSFORM_HEADER: &str = "X-Vadis-Transform";
/// The only value that asks for the mode.
const TRANSFORM_VALUE: &str = "transform";
/// Claude Code's documented "send additional headers" env var.
const CLAUDE_HEADER_ENV: &str = "ANTHROPIC_CUSTOM_HEADERS";
/// The env var `wrap` names in the codex `env_http_headers` override; the
/// header's *value* lives here, read by the client at request time.
const CODEX_VALUE_ENV: &str = "VADIS_TRANSFORM";

/// The clients `wrap` knows how to ask. The refusal message names this set.
pub const SUPPORTED_CLIENTS: &[&str] = &["codex", "claude"];

/// How `wrap` asks one client: the program to start, the argument overrides
/// to place before the user's own args, and the environment to add.
struct Wiring {
    program: &'static str,
    leading_args: Vec<String>,
    envs: Vec<(String, String)>,
}

/// Runs `vadis wrap <client> -- <args…>`. Resolves the client's own header
/// mechanism, then execs the client: this function returns **only** on a
/// refusal (`2`) or a failed exec (`127`) — on success the caller's process
/// image has been replaced.
pub fn run(client: &str, args: &[String]) -> i32 {
    let wiring = match wiring_for(client) {
        Ok(w) => w,
        Err(reason) => {
            eprintln!("vadis: {reason}");
            return 2;
        }
    };

    let mut cmd = StdCommand::new(wiring.program);
    cmd.args(&wiring.leading_args).args(args);
    for (k, v) in &wiring.envs {
        cmd.env(k, v);
    }
    // exec(3): this process becomes the client — inherited stdio, inherited
    // process group, signals and exit code forwarded by construction. The
    // call returns only when the exec itself failed.
    let err = cmd.exec();
    eprintln!("vadis: cannot start `{}`: {err}", wiring.program);
    127
}

fn wiring_for(client: &str) -> Result<Wiring, String> {
    match client {
        "claude" => claude_wiring(),
        "codex" => codex_wiring(),
        other => Err(format!(
            "unknown client `{other}`; wrap supports: {}",
            SUPPORTED_CLIENTS.join(", ")
        )),
    }
}

fn claude_wiring() -> Result<Wiring, String> {
    // Append, never clobber: the user's own extra headers ride along.
    let joined = joined_header_value(&std::env::var(CLAUDE_HEADER_ENV).unwrap_or_default());
    Ok(Wiring {
        program: "claude",
        leading_args: Vec::new(),
        envs: vec![(CLAUDE_HEADER_ENV.to_string(), joined)],
    })
}

/// The value `wrap` gives `ANTHROPIC_CUSTOM_HEADERS`: the existing entries
/// preserved, the transform header appended — unless it is already there
/// (idempotence: wrapping twice must not duplicate the entry).
fn joined_header_value(existing: &str) -> String {
    let already = existing.split(',').any(|entry| {
        entry
            .split_once(':')
            .is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case("x-vadis-transform"))
    });
    if already {
        existing.to_string()
    } else if existing.trim().is_empty() {
        format!("{TRANSFORM_HEADER}: {TRANSFORM_VALUE}")
    } else {
        format!("{existing}, {TRANSFORM_HEADER}: {TRANSFORM_VALUE}")
    }
}

fn codex_wiring() -> Result<Wiring, String> {
    let provider = codex_model_provider().ok_or_else(|| {
        "the codex config names no top-level `model_provider`; wrap has no \
         provider entry to hang the header on (set it, or wrap `claude`)"
            .to_string()
    })?;
    if !provider
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "model_provider `{provider}` is not a bare TOML key; refusing to \
             build a codex override from it"
        ));
    }
    // codex reads the header's value from the named env var at request time;
    // the wiring itself is an argument override, not a file, and comes first
    // so the user's own `-c` on the same key still wins.
    let override_value = format!(r#"{{"{TRANSFORM_HEADER}"="{CODEX_VALUE_ENV}"}}"#);
    Ok(Wiring {
        program: "codex",
        leading_args: vec![
            "-c".to_string(),
            format!("model_providers.{provider}.env_http_headers={override_value}"),
        ],
        envs: vec![(CODEX_VALUE_ENV.to_string(), TRANSFORM_VALUE.to_string())],
    })
}

/// The provider id codex will use: the top-level `model_provider` key of
/// `$CODEX_HOME/config.toml` (default `~/.codex/config.toml`). Read-only —
/// `wrap` never writes the user's client config. Only the top-level table
/// is scanned: the first `[section]` header ends the lookup.
fn codex_model_provider() -> Option<String> {
    let codex_home = match std::env::var("CODEX_HOME") {
        Ok(v) if !v.is_empty() => std::path::PathBuf::from(v),
        _ => {
            let home = std::env::var("HOME").ok()?;
            std::path::PathBuf::from(home).join(".codex")
        }
    };
    let text = std::fs::read_to_string(codex_home.join("config.toml")).ok()?;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            break; // left the top-level table
        }
        if let Some(rest) = trimmed.strip_prefix("model_provider") {
            if let Some(value) = rest.trim_start().strip_prefix('=') {
                let value = value.trim().trim_matches('"').trim_matches('\'');
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_header_append_preserves_and_is_idempotent() {
        assert_eq!(
            joined_header_value("X-Other: v"),
            "X-Other: v, X-Vadis-Transform: transform"
        );
        assert_eq!(joined_header_value(""), "X-Vadis-Transform: transform");
        assert_eq!(
            joined_header_value("x-vadis-transform: transform"),
            "x-vadis-transform: transform"
        );
    }

    #[test]
    fn codex_provider_scan_reads_only_the_top_level_table() {
        let dir = std::env::temp_dir().join("vadis-wrap-test-codex-home");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.toml"),
            "# comment\nmodel_provider = \"router\"\n[profiles.x]\nmodel_provider = \"other\"\n",
        )
        .unwrap();
        std::env::set_var("CODEX_HOME", &dir);
        assert_eq!(codex_model_provider().as_deref(), Some("router"));
        std::env::remove_var("CODEX_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
