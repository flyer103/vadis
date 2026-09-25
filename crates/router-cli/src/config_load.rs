//! Config file loading: YAML bytes → validated [`RouterConfig`] plus the
//! resolved-path form (DESIGN §12.10.2). File I/O lives here, not in
//! `router-core`, so the domain crate stays pure.
//!
//! One resolution rule, stated once: a relative path is resolved against
//! **the directory containing the config file** — never the CWD — for
//! `trace.dir`, `providers_file` and `state/` alike (spec §4.1/§4.14;
//! ADR-009 item 6).
//!
//! This module is the only place in the tree that knows the configuration
//! can be a **pair** of files (ADR-037): the root names its roster with
//! `providers_file`, [`RootFile::join`] fills the one domain field, and
//! every roster-side failure names the roster's own resolved path — a
//! broken roster never reads as a broken root (ADR-037 D5).

use std::path::{Component, Path, PathBuf};

use router_core::config::{RootFile, RosterFile, RouterConfig};

/// The resolved form handed to `router-proxy` (DESIGN §12.10.2).
#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    /// The anchor for every relative path (spec §4.1).
    pub config_dir: PathBuf,
    /// `<config_dir>/<trace.dir>`
    pub trace_dir: PathBuf,
    /// `<config_dir>/state/router.db` (fixed in v0.1, spec §4.5).
    pub state_db: PathBuf,
    /// The validated file itself.
    pub router: RouterConfig,
}

/// Parse the root's own shape — the first half of every entry point. The
/// exactly-one-of *decision* is [`RootFile::join`]'s one home; this is only
/// the YAML boundary, with the same message shape as before the split.
fn parse_root(text: &str) -> Result<RootFile, String> {
    serde_yaml::from_str(text).map_err(|e| format!("does not parse: {e}"))
}

/// Parse the roster file's own shape (spec §4.14): exactly one top-level
/// `providers:` key. The refusal names the roster's **own resolved path**
/// and the offending key, never the root's (ADR-037 D5 shape 4).
fn parse_roster(text: &str, resolved: &Path) -> Result<RosterFile, String> {
    serde_yaml::from_str(text)
        .map_err(|e| format!("roster file {}: does not parse: {e}", resolved.display()))
}

/// The pair's finish: the pure join, then the one existing
/// [`RouterConfig::validate`]. When a roster took part, every post-join
/// refusal carries the roster's resolved path: a root key that fails to
/// resolve in the roster names the file it failed in (ADR-037 D5 shape 5),
/// and a roster entry that breaks a per-entry rule names the file it was
/// read from (shape 4's per-entry arm).
fn finish(root: RootFile, roster: Option<(RosterFile, PathBuf)>) -> Result<RouterConfig, String> {
    let roster_path = roster.as_ref().map(|(_, p)| p.clone());
    let router = root
        .join(roster.map(|(r, _)| r))
        .map_err(|e| e.to_string())?;
    router.validate().map_err(|e| match &roster_path {
        Some(p) => format!(
            "{e} — the roster these keys resolve against is {} (providers_file; spec §4.14)",
            p.display()
        ),
        None => e.to_string(),
    })?;
    Ok(router)
}

/// The shared parse + validate entry point (spec §4.11 step 8, DESIGN
/// §12.14 step 8) for the **inline** form: the same calls `load()` makes,
/// extracted mechanically so the `serve` startup and `setup`'s candidate
/// gate are literally the same code — the two cannot drift. `load()`'s own
/// messages keep their shape (the path-prefixed wrapper below).
///
/// A root that names `providers_file` is a **pair** and is refused here by
/// [`RootFile::join`] pointing at the pair's entry points: the pair is
/// validated together, never as a root alone (spec §4.14).
pub fn validate_text(text: &str) -> Result<RouterConfig, String> {
    finish(parse_root(text)?, None)
}

/// The **pair**'s entry point (DESIGN §12.5; spec §4.11's candidate-is-the-
/// pair rule): the root's text plus the roster's own text and the resolved
/// path its refusals name. `load()` lands here after reading both files;
/// `setup --check`'s candidate gate (R43-4) validates the candidate pair
/// through the same call, so the two cannot drift.
pub fn validate_pair(
    root_text: &str,
    roster_text: &str,
    roster_resolved: &Path,
) -> Result<RouterConfig, String> {
    let root = parse_root(root_text)?;
    let roster = parse_roster(roster_text, roster_resolved)?;
    finish(root, Some((roster, roster_resolved.to_path_buf())))
}

/// Read, parse, validate and resolve. Any failure is terminal for `serve`
/// (non-zero exit, reason printed by the caller) — there is no silent
/// fallback to defaults (DESIGN §12.10.2, CONF-25's counterpart).
pub fn load(path: &Path) -> Result<ResolvedConfig, String> {
    let name_err = |reason: &str| format!("config file {}: {reason}", path.display());
    let bytes = std::fs::read(path).map_err(|e| name_err(&format!("cannot be read: {e}")))?;
    let text = std::str::from_utf8(&bytes).map_err(|_| name_err("is not valid UTF-8"))?;

    let root = parse_root(text).map_err(|e| name_err(&e))?;

    let config_dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let config_dir = lexical_absolute(&config_dir);

    // The roster is read only when the root names one and does not also
    // carry it inline: a both-written root is refused by the join with no
    // roster I/O at all, so shape 1 always fires ahead of shape 3
    // (ADR-037 D5 — the loader refuses *before* the join).
    let roster = if root.providers.is_none() && root.providers_file.is_some() {
        let written = root.providers_file.as_deref().unwrap_or_default();
        let resolved = resolve(&config_dir, written);
        let roster_err = |reason: &str| {
            format!(
                "providers_file '{written}' (resolved: {}): {reason}",
                resolved.display()
            )
        };
        let rbytes = std::fs::read(&resolved)
            .map_err(|e| name_err(&roster_err(&format!("cannot be read: {e}"))))?;
        let rtext = std::str::from_utf8(&rbytes)
            .map_err(|_| name_err(&roster_err("is not valid UTF-8")))?;
        let parsed = parse_roster(rtext, &resolved).map_err(|e| name_err(&e))?;
        Some((parsed, resolved))
    } else {
        None
    };

    let router = finish(root, roster).map_err(|e| name_err(&e))?;

    Ok(ResolvedConfig {
        trace_dir: resolve(&config_dir, &router.trace.dir),
        state_db: resolve(&config_dir, "state/router.db"),
        config_dir,
        router,
    })
}

/// Anchor a possibly relative path at the config directory, then normalize
/// away `.` components (and non-leading `..` pairs) so `/health` and logs
/// report one stable spelling. Absolute inputs are only normalized.
/// Resolve a config-relative path (the trace.dir / rules_file discipline:
/// absolute wins, else `<config_dir>/<value>`, lexically absolute).
pub fn resolve(config_dir: &Path, value: &str) -> PathBuf {
    let joined = if Path::new(value).is_absolute() {
        PathBuf::from(value)
    } else {
        config_dir.join(value)
    };
    lexical_absolute(&joined)
}

fn lexical_absolute(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                // Best effort: a `..` that cannot be popped (e.g. it would
                // climb above the config dir's absolute root) is kept
                // verbatim rather than silently rewritten.
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use router_core::config::AccountKind;

    fn write_temp(name: &str, body: &str) -> (tempdir::TempDirGuard, PathBuf) {
        tempdir::write(name, body)
    }

    // A minimal valid config; the full-tree happy path is covered by
    // router-core's unit tests. Here we exercise the loader's own concerns:
    // path anchoring, missing file, YAML syntax, validation pass-through.
    const MINIMAL: &str = r#"
server:   { addr: "127.0.0.1:8790", upstream_attempt_timeout: 60s, request_timeout: 10m }
session:  { key_sources: ["prompt_cache_key"], ttl: 12h }
cache:    { sticky: true, breakeven: { enabled: true, min_remaining_turns: 3, safety_factor: 1.2 } }
trace:    { dir: "./state/traces", rollover: hourly }
providers:
  - name: p
    urls: { chat: https://x.example/v1/chat/completions }
    api_key_env: P_KEY
    wire_api: chat
    supports: [chat]
    models:
      - id: m1
        context: 200k
        price:
          input_miss: 0.00066
          input_hit: 0.000022
          cache_write: 0.0
          output: 0.00198
          peak: { multiplier: 1.0, windows: [] }
        source: "https://x.example/pricing @2026-09-19"
aliases:  {}
plugins: []
fallback: []
"#;

    // A tiny temp-dir helper so the loader tests need no dev-dependency.
    mod tempdir {
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicU64, Ordering};

        pub struct TempDirGuard(pub PathBuf);

        impl Drop for TempDirGuard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        pub fn write(name: &str, body: &str) -> (TempDirGuard, PathBuf) {
            static N: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "router-cfg-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).expect("temp dir");
            let path = dir.join(name);
            std::fs::write(&path, body).expect("write config");
            (TempDirGuard(dir), path)
        }
    }

    #[test]
    fn happy_config_loads_and_resolves_paths_against_config_dir() {
        let (_g, path) = write_temp("config.yaml", MINIMAL);
        let rc = load(&path).expect("loads");
        assert!(rc.trace_dir.ends_with("state/traces"));
        assert!(rc.state_db.ends_with("state/router.db"));
        assert_eq!(rc.trace_dir.parent(), rc.state_db.parent());
        assert_eq!(rc.router.providers.len(), 1);
    }

    #[test]
    fn missing_file_is_named_not_silent() {
        let err = load(Path::new("/definitely/not/here.yaml")).unwrap_err();
        assert!(err.contains("/definitely/not/here.yaml"), "got: {err}");
        assert!(err.contains("cannot be read"), "got: {err}");
    }

    #[test]
    fn yaml_syntax_error_is_named() {
        let (_g, path) = write_temp("config.yaml", "server: [oops");
        let err = load(&path).unwrap_err();
        assert!(err.contains("does not parse"), "got: {err}");
    }

    #[test]
    fn validation_error_names_the_key() {
        // alias pointing at a route that does not exist
        let bad = MINIMAL.replace("aliases:  {}", "aliases:  { fast: p/nope }");
        let (_g, path) = write_temp("config.yaml", &bad);
        let err = load(&path).unwrap_err();
        assert!(err.contains("aliases.fast"), "got: {err}");
        assert!(err.contains("p/nope"), "got: {err}");
    }

    #[test]
    fn trace_dir_ignores_the_cwd() {
        let (_g, path) = write_temp("nested-config.yaml", MINIMAL);
        // The resolved trace dir must be anchored at the config's directory
        // no matter what the CWD is — assert it is a sibling of the file.
        let rc = load(&path).expect("loads");
        assert_eq!(
            rc.trace_dir.ancestors().nth(2),
            path.parent(),
            "trace_dir should live under the config file's directory"
        );
    }

    /// The shipped example **is** the file an implementation reads directly
    /// (spec §4), so it must always parse with the parser of the same commit:
    /// `deny_unknown_fields` turns a key the parser does not know into an
    /// unservable example, which is why the keys and the example land together
    /// (GAP-Q15). This is the regression that keeps the two in step, and it
    /// asserts the example exercises ADR-014's keys — a key that exists only in
    /// the spec is a key nothing has ever parsed.
    #[test]
    fn the_shipped_example_parses_and_carries_the_plan_first_keys() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config.example.yaml");
        let rc = load(&path).unwrap_or_else(|e| panic!("config.example.yaml: {e}"));
        assert!(!rc.router.providers.is_empty());

        let policy = rc
            .router
            .plan_policy
            .as_ref()
            .expect("the example exercises `plan_policy` (spec §4.6)");
        assert_eq!(policy.family, "glm-5.3");
        assert!(rc
            .router
            .providers
            .iter()
            .any(|p| p.account == AccountKind::CodingPlan));
        assert!(rc
            .router
            .providers
            .iter()
            .any(|p| p.account == AccountKind::Api));
    }

    // -----------------------------------------------------------------
    // The split (spec §4.14; ADR-037): the root names a roster file, the
    // join fills the one domain field, every refusal names its own file.
    // The conformance-level ladder is CONF-85; these are the loader's own
    // units over the same entry points.
    // -----------------------------------------------------------------

    /// MINIMAL's `providers:` block as the roster file's own text — the
    /// same block, byte-moved under its one top-level key (ADR-037 D3).
    fn roster_text() -> String {
        let (_head, rest) = MINIMAL.split_once("providers:\n").unwrap();
        let (block, _tail) = rest.split_once("aliases:").unwrap();
        format!("providers:\n{block}")
    }

    /// MINIMAL with the roster block replaced by the one naming key.
    fn split_root_text() -> String {
        let (head, rest) = MINIMAL.split_once("providers:\n").unwrap();
        let (_block, tail) = rest.split_once("aliases:").unwrap();
        format!("{head}providers_file: ./providers.yaml\naliases:{tail}")
    }

    /// MINIMAL with the roster block removed and no naming key (shape 2).
    fn rosterless_root_text() -> String {
        let (head, rest) = MINIMAL.split_once("providers:\n").unwrap();
        let (_block, tail) = rest.split_once("aliases:").unwrap();
        format!("{head}aliases:{tail}")
    }

    fn write_pair(root_text: &str, roster: Option<&str>) -> (tempdir::TempDirGuard, PathBuf) {
        let (guard, root) = write_temp("config.yaml", root_text);
        if let Some(body) = roster {
            std::fs::write(root.parent().unwrap().join("providers.yaml"), body).unwrap();
        }
        (guard, root)
    }

    #[test]
    fn split_pair_joins_the_same_roster_as_the_inline_form() {
        let (_gi, inline_path) = write_temp("config.yaml", MINIMAL);
        let inline = load(&inline_path).expect("inline loads");

        let (_gp, pair_path) = write_pair(&split_root_text(), Some(&roster_text()));
        let pair = load(&pair_path).expect("the pair loads");

        // Deep-equal (the config types carry Debug, not PartialEq): one
        // representation, whichever shape named it (ADR-037 D4).
        assert_eq!(
            format!("{:?}", pair.router.providers),
            format!("{:?}", inline.router.providers),
        );
        assert_eq!(pair.router.providers.len(), 1);
        assert_eq!(pair.router.providers[0].name, "p");
        // The resolution rule is unchanged: trace.dir and the store anchor
        // at the pair's own config directory (never the CWD).
        assert!(pair.trace_dir.ends_with("state/traces"));
        assert_eq!(pair.trace_dir.parent(), pair.state_db.parent());
        assert!(pair.trace_dir.starts_with(pair_path.parent().unwrap()));
    }

    #[test]
    fn both_written_is_refused_naming_both_keys_and_the_root() {
        // The named roster deliberately does NOT exist: shape 1 refuses
        // before any roster I/O, so a missing file cannot mask it.
        let both = format!("{MINIMAL}providers_file: ./providers.yaml\n");
        let (_g, path) = write_temp("config.yaml", &both);
        let err = load(&path).unwrap_err();
        assert!(err.contains("both"), "got: {err}");
        assert!(err.contains("providers:"), "got: {err}");
        assert!(err.contains("providers_file"), "got: {err}");
        assert!(err.contains(&path.display().to_string()), "got: {err}");
    }

    #[test]
    fn neither_written_is_refused_naming_both_keys() {
        let (_g, path) = write_temp("config.yaml", &rosterless_root_text());
        let err = load(&path).unwrap_err();
        assert!(err.contains("neither"), "got: {err}");
        assert!(err.contains("providers:"), "got: {err}");
        assert!(err.contains("providers_file"), "got: {err}");
        assert!(err.contains(&path.display().to_string()), "got: {err}");
    }

    #[test]
    fn explicit_empty_roster_is_a_decision_not_a_default() {
        let empty = rosterless_root_text().replace("aliases:", "providers: []\naliases:");
        let (_g, path) = write_temp("config.yaml", &empty);
        let rc = load(&path).expect("providers: [] loads");
        assert!(rc.router.providers.is_empty());

        // Symmetric (the opener's limb (i)): a roster file whose one key
        // carries the empty list is the same decision in the split form.
        let (_gp, pair_path) = write_pair(&split_root_text(), Some("providers: []\n"));
        let rc = load(&pair_path).expect("a roster of providers: [] loads");
        assert!(rc.router.providers.is_empty());
    }

    #[test]
    fn null_is_refused_by_its_own_key_never_as_neither_written() {
        // providers: null — the key is present and wrong; the refusal
        // names providers:, not the neither-written shape (the opener's
        // limb (i)).
        let null_inline = rosterless_root_text().replace("aliases:", "providers: null\naliases:");
        let (_g, path) = write_temp("config.yaml", &null_inline);
        let err = load(&path).unwrap_err();
        assert!(err.contains("providers"), "got: {err}");
        assert!(!err.contains("neither"), "got: {err}");

        // providers_file: null, symmetric.
        let null_file =
            rosterless_root_text().replace("aliases:", "providers_file: null\naliases:");
        let (_g2, path2) = write_temp("config.yaml", &null_file);
        let err2 = load(&path2).unwrap_err();
        assert!(err2.contains("providers_file"), "got: {err2}");
        assert!(!err2.contains("neither"), "got: {err2}");
    }

    #[test]
    fn missing_roster_names_the_key_the_value_and_the_resolved_path() {
        let (_g, path) = write_pair(&split_root_text(), None);
        let err = load(&path).unwrap_err();
        assert!(err.contains("providers_file"), "got: {err}");
        assert!(err.contains("'./providers.yaml'"), "got: {err}");
        let resolved = path.parent().unwrap().join("providers.yaml");
        assert!(err.contains(&resolved.display().to_string()), "got: {err}");
        assert!(err.contains("cannot be read"), "got: {err}");
    }

    #[test]
    fn a_roster_that_is_not_the_roster_block_names_its_own_path() {
        for (tag, body) in [
            ("server-block", "server: { addr: \"127.0.0.1:1\" }\n"),
            ("bare-sequence", "- name: p\n"),
            ("empty", ""),
            ("second-key", "providers: []\nserver: {}\n"),
        ] {
            let (_g, path) = write_pair(&split_root_text(), Some(body));
            let err = load(&path).unwrap_err();
            let resolved = path.parent().unwrap().join("providers.yaml");
            assert!(
                err.contains(&resolved.display().to_string()),
                "{tag}: the refusal names the roster's own path, got: {err}"
            );
        }
        // The offending key is named where there is one.
        let (_g, path) = write_pair(&split_root_text(), Some("server: {}\n"));
        let err = load(&path).unwrap_err();
        assert!(err.contains("server"), "got: {err}");
    }

    #[test]
    fn an_unresolved_reference_names_the_key_and_the_roster_file() {
        let bad = split_root_text().replace("aliases:  {}", "aliases:  { fast: p/nope }");
        let (_g, path) = write_pair(&bad, Some(&roster_text()));
        let err = load(&path).unwrap_err();
        assert!(err.contains("aliases.fast"), "got: {err}");
        assert!(err.contains("p/nope"), "got: {err}");
        let resolved = path.parent().unwrap().join("providers.yaml");
        assert!(
            err.contains(&resolved.display().to_string()),
            "the reference error names the roster it failed to resolve in, got: {err}"
        );
    }

    #[test]
    fn validate_text_and_validate_pair_are_the_two_shapes_entry_points() {
        // The inline form through validate_text (unchanged signature).
        let inline = validate_text(MINIMAL).expect("inline validates");

        // A split root alone is refused pointing at the pair's entries.
        let err = validate_text(&split_root_text()).unwrap_err();
        assert!(err.contains("providers_file"), "got: {err}");

        // The pair from text, deep-equal to the inline form.
        let roster_path = Path::new("/pair/providers.yaml");
        let joined = validate_pair(&split_root_text(), &roster_text(), roster_path)
            .expect("the pair validates");
        assert_eq!(
            format!("{:?}", joined.providers),
            format!("{:?}", inline.providers),
        );

        // A broken roster from text names the path the caller handed.
        let err = validate_pair(&split_root_text(), "server: {}\n", roster_path).unwrap_err();
        assert!(err.contains("/pair/providers.yaml"), "got: {err}");
        assert!(err.contains("server"), "got: {err}");
    }
}
