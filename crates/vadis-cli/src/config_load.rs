//! Config file loading: YAML bytes → validated [`VadisConfig`] plus the
//! resolved-path form (DESIGN §12.10.2). File I/O lives here, not in
//! `vadis-core`, so the domain crate stays pure.
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

use vadis_core::config::{RootFile, RosterFile, VadisConfig};
use vadis_core::prefix::body_sha16;

/// The resolved form handed to `vadis-proxy` (DESIGN §12.10.2).
#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    /// The anchor for every relative path (spec §4.1).
    pub config_dir: PathBuf,
    /// `<config_dir>/<trace.dir>`
    pub trace_dir: PathBuf,
    /// `<config_dir>/state/router.db` (fixed in v0.1, spec §4.5).
    pub state_db: PathBuf,
    /// The validated file itself.
    pub vadis: VadisConfig,
    /// The byte-digest identity of what was loaded (ADR-037 D6): computed
    /// once here, over the bytes exactly as read, and carried unchanged to
    /// the trace row, the `config.applied` event and `/health` (spec §6,
    /// §9.1 — one value, three surfaces).
    pub identity: ConfigIdentity,
}

/// Which configuration a process is serving, stated so an outsider can
/// recompute it from the two files alone (spec §4.14's recipe):
///
/// ```text
/// root_sha16    = sha16(bytes of the root file)
/// roster_sha16  = sha16(bytes of the roster file); "" when the roster is inline
/// config_digest = sha16(root_sha16 + ":" + roster_sha16)
/// ```
///
/// `sha16` is the repository's one hash convention (`vadis-core`'s
/// `prefix.rs`). A byte digest, deliberately: a comment-only edit moves it,
/// because the comments are where the price citations live (§4.0). The two
/// null/empty spellings are §9.1's: a path that does not exist is
/// `roster_path: None`; a hash input that is not there is `roster_sha16:
/// ""`.
#[derive(Debug, Clone)]
pub struct ConfigIdentity {
    /// The root file's own path, lexically normalized the way `trace_dir`
    /// and `state_db` already are — never canonicalized: the spelling the
    /// §4.12 order selected is the fact reported (a symlink stays a
    /// symlink; the digest, not the path, identifies the bytes).
    pub root_path: PathBuf,
    /// The roster's resolved absolute path when the root names one.
    pub roster_path: Option<PathBuf>,
    /// First 16 hex chars of SHA-256 over the root file's bytes.
    pub root_sha16: String,
    /// The same over the roster file's bytes; the empty string when the
    /// roster is inline (the value the recipe hashes).
    pub roster_sha16: String,
    /// `sha16(root_sha16 + ":" + roster_sha16)`.
    pub config_digest: String,
}

/// The one composition rule (spec §4.14): the digest of the pair of
/// digests, with the inline roster's empty half. Reproducible by hand:
/// `printf '%s:%s' <root_sha16> <roster_sha16> | shasum -a 256 | cut -c1-16`.
fn compose_digest(root_sha16: &str, roster_sha16: &str) -> String {
    body_sha16(format!("{root_sha16}:{roster_sha16}").as_bytes())
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
/// [`VadisConfig::validate`]. When a roster took part, every post-join
/// refusal carries the roster's resolved path: a root key that fails to
/// resolve in the roster names the file it failed in (ADR-037 D5 shape 5),
/// and a roster entry that breaks a per-entry rule names the file it was
/// read from (shape 4's per-entry arm).
fn finish(root: RootFile, roster: Option<(RosterFile, PathBuf)>) -> Result<VadisConfig, String> {
    let roster_path = roster.as_ref().map(|(_, p)| p.clone());
    let vadis = root
        .join(roster.map(|(r, _)| r))
        .map_err(|e| e.to_string())?;
    vadis.validate().map_err(|e| match &roster_path {
        Some(p) => format!(
            "{e} — the roster these keys resolve against is {} (providers_file; spec §4.14)",
            p.display()
        ),
        None => e.to_string(),
    })?;
    Ok(vadis)
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
pub fn validate_text(text: &str) -> Result<VadisConfig, String> {
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
) -> Result<VadisConfig, String> {
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
    // The identity's root half is hashed from the bytes exactly as read
    // (spec §4.14) — before any parsing, so the digest describes the file,
    // never a canonical form of its values (ADR-037 D6).
    let root_sha16 = body_sha16(&bytes);
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
        // The roster half, from the roster's bytes exactly as read.
        let roster_sha16 = body_sha16(&rbytes);
        let rtext = std::str::from_utf8(&rbytes)
            .map_err(|_| name_err(&roster_err("is not valid UTF-8")))?;
        let parsed = parse_roster(rtext, &resolved).map_err(|e| name_err(&e))?;
        Some((parsed, resolved, roster_sha16))
    } else {
        None
    };

    // The identity of the loaded pair (spec §4.14): the inline shape's
    // roster half is the null path and the empty digest input (§9.1's two
    // spellings of one fact).
    let (roster_path, roster_sha16) = match &roster {
        Some((_, resolved, sha)) => (Some(resolved.clone()), sha.clone()),
        None => (None, String::new()),
    };
    let identity = ConfigIdentity {
        root_path: lexical_absolute(path),
        roster_path,
        config_digest: compose_digest(&root_sha16, &roster_sha16),
        root_sha16,
        roster_sha16,
    };

    let vadis = finish(root, roster.map(|(parsed, resolved, _)| (parsed, resolved)))
        .map_err(|e| name_err(&e))?;

    Ok(ResolvedConfig {
        trace_dir: resolve(&config_dir, &vadis.trace.dir),
        state_db: resolve(&config_dir, "state/router.db"),
        config_dir,
        vadis,
        identity,
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
    use vadis_core::config::AccountKind;

    fn write_temp(name: &str, body: &str) -> (tempdir::TempDirGuard, PathBuf) {
        tempdir::write(name, body)
    }

    // A minimal valid config; the full-tree happy path is covered by
    // vadis-core's unit tests. Here we exercise the loader's own concerns:
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
                "vadis-cfg-{}-{}",
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
        assert_eq!(rc.vadis.providers.len(), 1);
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
        assert!(!rc.vadis.providers.is_empty());

        let policy = rc
            .vadis
            .plan_policy
            .as_ref()
            .expect("the example exercises `plan_policy` (spec §4.6)");
        assert_eq!(policy.family, "glm-5.3");
        assert!(rc
            .vadis
            .providers
            .iter()
            .any(|p| p.account == AccountKind::CodingPlan));
        assert!(rc
            .vadis
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
            format!("{:?}", pair.vadis.providers),
            format!("{:?}", inline.vadis.providers),
        );
        assert_eq!(pair.vadis.providers.len(), 1);
        assert_eq!(pair.vadis.providers[0].name, "p");
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
        assert!(rc.vadis.providers.is_empty());

        // Symmetric (the opener's limb (i)): a roster file whose one key
        // carries the empty list is the same decision in the split form.
        let (_gp, pair_path) = write_pair(&split_root_text(), Some("providers: []\n"));
        let rc = load(&pair_path).expect("a roster of providers: [] loads");
        assert!(rc.vadis.providers.is_empty());
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

        // providers_file: null, symmetric — and the F1 control: a valid
        // roster file literally named `null` sits beside the config and
        // must change NOTHING (at the fix's base the written null was
        // coerced to the path "null" and that file was silently loaded —
        // R43-2b F1). The refusal below, with no read attempted, is the
        // proof the sentinel is never touched.
        let null_file =
            rosterless_root_text().replace("aliases:", "providers_file: null\naliases:");
        let (_g2, path2) = write_temp("config.yaml", &null_file);
        std::fs::write(path2.parent().unwrap().join("null"), roster_text()).unwrap();
        let err2 = load(&path2).unwrap_err();
        assert!(err2.contains("providers_file"), "got: {err2}");
        assert!(err2.contains("found null"), "got: {err2}");
        assert!(
            !err2.contains("cannot be read"),
            "no path resolution is attempted, got: {err2}"
        );
        assert!(!err2.contains("neither"), "got: {err2}");

        // providers_file: "" — an empty path is not a path; refused by
        // the key in the same shape, never resolved.
        let empty_file =
            rosterless_root_text().replace("aliases:", "providers_file: \"\"\naliases:");
        let (_g3, path3) = write_temp("config.yaml", &empty_file);
        let err3 = load(&path3).unwrap_err();
        assert!(err3.contains("providers_file"), "got: {err3}");
        assert!(err3.contains("empty"), "got: {err3}");
        assert!(
            !err3.contains("cannot be read"),
            "no path resolution is attempted, got: {err3}"
        );
        assert!(!err3.contains("neither"), "got: {err3}");

        // The positive discriminator: an explicitly written string
        // "./null" is a named path, not a null — it loads the sentinel
        // file. The refusal above is a type check on the VALUE, not a
        // ban on a spelling.
        let named = rosterless_root_text().replace("aliases:", "providers_file: ./null\naliases:");
        let (_g4, path4) = write_temp("config.yaml", &named);
        std::fs::write(path4.parent().unwrap().join("null"), roster_text()).unwrap();
        let rc = load(&path4).expect("an explicit './null' names the file");
        assert_eq!(rc.vadis.providers.len(), 1);
        assert_eq!(rc.vadis.providers[0].name, "p");
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

    // -----------------------------------------------------------------
    // The identity (spec §4.14/§6; ADR-037 D6): a byte digest of the
    // files, computed once here, reproducible from them by one shell
    // line. The conformance-level cross-surface assertion is CONF-85's
    // half B; these are the loader's own units over the recipe.
    // -----------------------------------------------------------------

    /// The recipe's expected value, computed from the files the test
    /// wrote — the same composition the spec's one-liner states, through
    /// the repository's one hash convention (never a second spelling).
    fn expected_identity(root: &Path, roster: Option<&Path>) -> (String, String, String) {
        let root_sha16 = body_sha16(&std::fs::read(root).unwrap());
        let roster_sha16 = roster
            .map(|r| body_sha16(&std::fs::read(r).unwrap()))
            .unwrap_or_default();
        let digest = compose_digest(&root_sha16, &roster_sha16);
        (root_sha16, roster_sha16, digest)
    }

    #[test]
    fn identity_of_the_inline_form_hashes_the_root_alone() {
        let (_g, path) = write_temp("config.yaml", MINIMAL);
        let rc = load(&path).expect("loads");
        let (root_sha16, roster_sha16, digest) = expected_identity(&path, None);
        assert_eq!(rc.identity.root_path, lexical_absolute(&path));
        assert_eq!(rc.identity.roster_path, None);
        assert_eq!(rc.identity.root_sha16, root_sha16);
        assert_eq!(
            rc.identity.roster_sha16, "",
            "the inline shape's roster half is the empty string (§9.1)"
        );
        assert_eq!(roster_sha16, "");
        assert_eq!(rc.identity.config_digest, digest);
        // 16 lowercase hex chars — the one convention's shape.
        assert_eq!(rc.identity.config_digest.len(), 16);
        assert!(rc
            .identity
            .config_digest
            .chars()
            .all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn identity_of_the_pair_hashes_both_files() {
        let (_g, root) = write_pair(&split_root_text(), Some(&roster_text()));
        let roster = root.parent().unwrap().join("providers.yaml");
        let rc = load(&root).expect("the pair loads");
        let (root_sha16, roster_sha16, digest) = expected_identity(&root, Some(&roster));
        assert_eq!(rc.identity.root_path, lexical_absolute(&root));
        assert_eq!(rc.identity.roster_path, Some(roster));
        assert_eq!(rc.identity.root_sha16, root_sha16);
        assert_eq!(rc.identity.roster_sha16, roster_sha16);
        assert_ne!(
            rc.identity.roster_sha16, "",
            "a named roster contributes its own bytes"
        );
        assert_eq!(rc.identity.config_digest, digest);
        // The digest of the pair is not the digest of the root alone.
        assert_ne!(rc.identity.config_digest, compose_digest(&root_sha16, ""));
    }

    #[test]
    fn a_comment_only_roster_edit_moves_the_digest_and_nothing_else() {
        let commented = format!("# a provenance note, not a value\n{}", roster_text());
        let (_g1, root1) = write_pair(&split_root_text(), Some(&roster_text()));
        let (_g2, root2) = write_pair(&split_root_text(), Some(&commented));
        let a = load(&root1).expect("plain pair loads");
        let b = load(&root2).expect("commented pair loads");
        // The byte digest is stricter than a parsed-value digest — and
        // that is deliberate (ADR-037 D6: the comments carry the price
        // citations).
        assert_ne!(a.identity.config_digest, b.identity.config_digest);
        assert_eq!(a.identity.root_sha16, b.identity.root_sha16);
        assert_ne!(a.identity.roster_sha16, b.identity.roster_sha16);
        // …and the edit moves nothing else: the joined config is
        // deep-equal (Debug form; the config types carry no PartialEq).
        assert_eq!(
            format!("{:?}", a.vadis),
            format!("{:?}", b.vadis),
            "a comment moves the identity, never the parsed config"
        );
    }

    #[test]
    fn the_same_bytes_at_two_paths_share_one_digest() {
        // The identity names the bytes, not the location: two copies of
        // one pair at two paths are one configuration (the documented
        // same-bytes-different-path reading of §4.14).
        let (_g1, root1) = write_pair(&split_root_text(), Some(&roster_text()));
        let (_g2, root2) = write_pair(&split_root_text(), Some(&roster_text()));
        let a = load(&root1).unwrap();
        let b = load(&root2).unwrap();
        assert_eq!(a.identity.config_digest, b.identity.config_digest);
        assert_ne!(a.identity.root_path, b.identity.root_path);
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
