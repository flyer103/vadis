//! Config file loading: YAML bytes → validated [`RouterConfig`] plus the
//! resolved-path form (DESIGN §12.10.2). File I/O lives here, not in
//! `router-core`, so the domain crate stays pure.
//!
//! One resolution rule, stated once: a relative path is resolved against
//! **the directory containing the config file** — never the CWD — for
//! `trace.dir` and `state/` alike (spec §4.1; ADR-009 item 6).

use std::path::{Component, Path, PathBuf};

use router_core::config::RouterConfig;

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

/// Read, parse, validate and resolve. Any failure is terminal for `serve`
/// (non-zero exit, reason printed by the caller) — there is no silent
/// fallback to defaults (DESIGN §12.10.2, CONF-25's counterpart).
pub fn load(path: &Path) -> Result<ResolvedConfig, String> {
    let name_err = |reason: &str| format!("config file {}: {reason}", path.display());
    let bytes = std::fs::read(path).map_err(|e| name_err(&format!("cannot be read: {e}")))?;
    let text = std::str::from_utf8(&bytes).map_err(|_| name_err("is not valid UTF-8"))?;

    let router: RouterConfig =
        serde_yaml::from_str(text).map_err(|e| name_err(&format!("does not parse: {e}")))?;
    router.validate().map_err(|e| name_err(&e.to_string()))?;

    let config_dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let config_dir = lexical_absolute(&config_dir);

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
    base_url: https://x.example/v1
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
}
