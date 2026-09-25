//! CONF-85 (DESIGN §12.8; spec §4/§4.14/§4.12; ADR-037 D1/D2/D5): **half A
//! — the refusal ladder of the roster as its own file.** Half B (the
//! byte-digest identity) is R43-3's and lands in this same file; the two
//! cards are serial because one path has one writer at a time (§12.8's
//! CONF-85 allocation).
//!
//! Against the real `serve` loader (`router_cli::config_load::load` — the
//! one entry point `serve` itself uses, `router-cli/src/lib.rs`), each
//! shape of spec §4.14's ladder is refused naming **its own key**, with
//! the inline control green on the same rig:
//!
//! - (a) both `providers:` and `providers_file:` written → both keys named
//!   (and the refusal fires with the named roster **absent**: shape 1
//!   precedes any roster I/O);
//! - (b) neither written → both keys named — with the presence
//!   discriminators of the opener's 2026-09-25 ruling (STATE.md "Opener
//!   rulings on R43-1b's two findings", limb (i)): `providers: []` is
//!   legal, `providers: null` is refused *by `providers:`* and never as
//!   "neither written", `providers_file: null` symmetric;
//! - (c) `providers_file` naming an unreadable path → the key, the value
//!   as written, and the resolved path;
//! - (d) a roster file whose top-level key is not `providers:` → the
//!   roster's own resolved path (and the offending key where there is
//!   one), with the symmetric discriminator that a roster of
//!   `providers: []` is legal;
//! - (e) a root reference the roster does not define (`aliases`,
//!   `fallback[0]`, `plan_policy.primary`) → the key **and** the roster
//!   file it failed to resolve in;
//! - (f) the no-candidate arm: a `providers.yaml` sitting beside an
//!   inline root is never read (§4.12's table gained no row), and a
//!   fixture **pair** written into the case's own temp dir parses with
//!   its joined `providers` deep-equal to the inline form's.
//!
//! Red at the round's base `3db485c` by construction: `providers_file`
//! was an unknown field there, so no split root loaded at all.

#![forbid(unsafe_code)]

use router_conformance::testkit;

/// The server-section head every root below shares (the CONF-53 shape).
const ROOT_HEAD: &str = r#"
server:   { addr: "127.0.0.1:39785", upstream_attempt_timeout: 45s, request_timeout: 9m }
session:  { key_sources: ["prompt_cache_key"], ttl: 11h }
cache:    { sticky: true, breakeven: { enabled: true, min_remaining_turns: 2, safety_factor: 1.1 } }
trace:    { dir: "./state/traces", rollover: hourly }

"#;

/// The roster block, written **once** here and moved into whichever shape
/// a leg needs — inline under the root, or byte-moved into the roster
/// file (ADR-037 D3).
const ROSTER: &str = r#"providers:
  - name: p
    urls:
      chat: https://only.example/v1/chat/completions
    api_key_env: CONF85_P_KEY
    wire_api: chat
    supports: [chat]
    models:
      - id: m1
        context: 128k
        price:
          input_miss: 0.001
          input_hit: 0.0001
          cache_write: 0.0
          output: 0.002
          peak: { multiplier: 1.0, windows: [] }
        source: "mock upstream (no price; test fixture)"

"#;

/// The root's own tail (aliases/plugins/fallback), patched per leg.
const ROOT_TAIL: &str = "aliases: {}\nplugins: []\nfallback: []\n";

fn inline_config() -> String {
    format!("{ROOT_HEAD}{ROSTER}{ROOT_TAIL}")
}

fn split_root() -> String {
    format!("{ROOT_HEAD}providers_file: ./providers.yaml\n\n{ROOT_TAIL}")
}

fn rosterless_root() -> String {
    format!("{ROOT_HEAD}{ROOT_TAIL}")
}

fn load_err(root_text: &str, roster: Option<&str>) -> String {
    let dir = testkit::tempdir("85");
    let root = dir.join("config.yaml");
    std::fs::write(&root, root_text).unwrap();
    if let Some(body) = roster {
        std::fs::write(dir.join("providers.yaml"), body).unwrap();
    }
    router_cli::config_load::load(&root).unwrap_err()
}

// (a) both keys written → refused naming both keys and the root — with
// the named roster present AND with it missing, because shape 1 is the
// loader's pre-join refusal and no roster I/O may precede or mask it.
#[test]
fn conf_85a_both_written_is_refused_naming_both_keys() {
    let both = format!("{ROOT_HEAD}{ROSTER}providers_file: ./providers.yaml\n\n{ROOT_TAIL}");
    for (tag, roster) in [("roster-present", Some(ROSTER)), ("roster-missing", None)] {
        let dir = testkit::tempdir("85a");
        let (root, _) = match roster {
            Some(body) => testkit::write_config_pair(&dir, &both, body),
            None => {
                let root = dir.join("config.yaml");
                std::fs::write(&root, &both).unwrap();
                (root, dir.join("providers.yaml"))
            }
        };
        let err = router_cli::config_load::load(&root).unwrap_err();
        assert!(err.contains("both"), "{tag}: got: {err}");
        assert!(err.contains("providers:"), "{tag}: got: {err}");
        assert!(err.contains("providers_file"), "{tag}: got: {err}");
        assert!(
            err.contains(&root.display().to_string()),
            "{tag}: the root path is named, got: {err}"
        );
    }
}

// (b) neither key written → refused naming both keys and the root — plus
// limb (i)'s presence discriminators: the explicit-empty roster is legal,
// the explicit nulls are refused by their own key, never as "neither".
#[test]
fn conf_85b_neither_written_is_refused_naming_both_keys() {
    let dir = testkit::tempdir("85b");
    let root = dir.join("config.yaml");
    std::fs::write(&root, rosterless_root()).unwrap();
    let err = router_cli::config_load::load(&root).unwrap_err();
    assert!(err.contains("neither"), "got: {err}");
    assert!(err.contains("providers:"), "got: {err}");
    assert!(err.contains("providers_file"), "got: {err}");
    assert!(err.contains(&root.display().to_string()), "got: {err}");

    // providers: [] — an empty roster is a decision, and it loads.
    let decided = rosterless_root().replace("aliases:", "providers: []\naliases:");
    let dir = testkit::tempdir("85b-empty");
    let root = dir.join("config.yaml");
    std::fs::write(&root, decided).unwrap();
    let rc = router_cli::config_load::load(&root).expect("providers: [] loads");
    assert!(rc.router.providers.is_empty());

    // providers: null — present and wrong, refused by `providers:`.
    let null_inline = rosterless_root().replace("aliases:", "providers: null\naliases:");
    let err = load_err(&null_inline, None);
    assert!(err.contains("providers"), "got: {err}");
    assert!(
        !err.contains("neither"),
        "a written null never reads as 'neither written', got: {err}"
    );

    // providers_file: null — symmetric.
    let null_file = rosterless_root().replace("aliases:", "providers_file: null\naliases:");
    let err = load_err(&null_file, None);
    assert!(err.contains("providers_file"), "got: {err}");
    assert!(!err.contains("neither"), "got: {err}");
}

// (c) `providers_file` naming a path that cannot be read → the key, the
// value as written, and the resolved path (never a bare "file not found").
#[test]
fn conf_85c_missing_roster_names_key_value_and_resolved_path() {
    let dir = testkit::tempdir("85c");
    let root = dir.join("config.yaml");
    std::fs::write(&root, split_root()).unwrap();
    let err = router_cli::config_load::load(&root).unwrap_err();
    assert!(err.contains("providers_file"), "got: {err}");
    assert!(err.contains("'./providers.yaml'"), "got: {err}");
    let resolved = dir.join("providers.yaml");
    assert!(
        err.contains(&resolved.display().to_string()),
        "the resolved path is named, got: {err}"
    );
    assert!(err.contains("cannot be read"), "got: {err}");
}

// (d) a roster file that is not the roster block → refused naming the
// roster's own resolved path (and the offending key where there is one) —
// never the root's path alone, never silently ignored.
#[test]
fn conf_85d_not_the_roster_block_names_the_rosters_own_path() {
    for (tag, body, key) in [
        (
            "server-block",
            "server: { addr: \"127.0.0.1:1\" }\n",
            Some("server"),
        ),
        ("bare-sequence", "- name: p\n", None),
        ("empty-document", "", None),
        (
            "second-top-level-key",
            "providers: []\nserver: {}\n",
            Some("server"),
        ),
    ] {
        let err = load_err(&split_root(), Some(body));
        assert!(
            err.contains("providers.yaml"),
            "{tag}: the roster's own path is named, got: {err}"
        );
        assert!(
            err.contains("does not parse"),
            "{tag}: refused as not-the-roster-block, got: {err}"
        );
        if let Some(key) = key {
            assert!(
                err.contains(key),
                "{tag}: the offending key is named, got: {err}"
            );
        }
        assert!(
            !err.contains("config file") || err.matches("config file").count() <= 1,
            "{tag}: a broken roster never reads as a broken root, got: {err}"
        );
    }

    // The symmetric discriminator (limb (i)): a roster file whose one key
    // carries the empty list is the same legal decision as `providers: []`.
    let dir = testkit::tempdir("85d-empty");
    let (root, _) = testkit::write_config_pair(&dir, &split_root(), "providers: []\n");
    let rc = router_cli::config_load::load(&root).expect("a roster of providers: [] loads");
    assert!(rc.router.providers.is_empty());
}

// (e) a root key that references the roster and does not resolve there →
// the existing reference error, naming the key path, the value found,
// AND the roster file it failed to resolve in (the reader may be holding
// only the root).
#[test]
fn conf_85e_unresolved_reference_names_the_key_and_the_roster_file() {
    for (tag, tail, key, value) in [
        (
            "aliases",
            "aliases: { fast: p/nope }\nplugins: []\nfallback: []\n",
            "aliases.fast",
            "p/nope",
        ),
        (
            "fallback",
            "aliases: {}\nplugins: []\nfallback: [p/nope]\n",
            "fallback[0]",
            "p/nope",
        ),
        (
            "plan-policy",
            "aliases: {}\nplugins: []\nfallback: []\nplan_policy:\n  family: m1\n  primary: p/ghost\n  overflow: p/m1\n  on_primary_exhausted: spill\n  recover: probe\n  cooldown: 0s\n",
            "plan_policy.primary",
            "p/ghost",
        ),
    ] {
        let root_text = format!("{ROOT_HEAD}providers_file: ./providers.yaml\n\n{tail}");
        let dir = testkit::tempdir("85e");
        let (root, roster) = testkit::write_config_pair(&dir, &root_text, ROSTER);
        let err = router_cli::config_load::load(&root).unwrap_err();
        assert!(err.contains(key), "{tag}: the key path is named, got: {err}");
        assert!(err.contains(value), "{tag}: the value found is named, got: {err}");
        assert!(
            err.contains(&roster.display().to_string()),
            "{tag}: the roster file it failed to resolve in is named, got: {err}"
        );
    }
}

// (f) the no-candidate arm and the join's deep-equality, with the inline
// control green on the same rig: a `providers.yaml` beside an inline root
// is never read (a named roster is not a discovery candidate, §4.12), and
// the pair form joins to exactly the inline form's providers.
#[test]
fn conf_85f_named_not_discovered_and_the_pair_joins_deep_equal() {
    // The inline control: green by construction, the arm the change must
    // not break.
    let dir = testkit::tempdir("85f-inline");
    let root = dir.join("config.yaml");
    std::fs::write(&root, inline_config()).unwrap();
    let inline = router_cli::config_load::load(&root).expect("the inline form loads");
    assert_eq!(inline.router.providers.len(), 1);
    assert_eq!(inline.router.providers[0].name, "p");

    // A stray providers.yaml beside the inline root — carrying a provider
    // the inline roster does not have — is never read: §4.12's four
    // candidates find *the config file*; the roster is named, not found.
    let stray = "providers:\n  - name: stray\n    urls: { chat: https://stray.example/v1 }\n    api_key_env: CONF85_STRAY_KEY\n    wire_api: chat\n    supports: [chat]\n    models:\n      - id: s1\n        context: 8k\n        price: { input_miss: 0.001, input_hit: 0.0001, cache_write: 0.0, output: 0.002, peak: { multiplier: 1.0, windows: [] } }\n        source: \"stray (never read)\"\n";
    std::fs::write(dir.join("providers.yaml"), stray).unwrap();
    let reloaded = router_cli::config_load::load(&root).expect("the inline form still loads");
    assert_eq!(
        reloaded.router.providers.len(),
        1,
        "the stray providers.yaml beside the root was read"
    );
    assert_eq!(reloaded.router.providers[0].name, "p");

    // The pair, written into the case's own temp dir by the helper, joins
    // to exactly the inline form's roster (deep-equal over the Debug
    // form — the config types carry Debug, not PartialEq).
    let dir = testkit::tempdir("85f-pair");
    let (root, _) = testkit::write_config_pair(&dir, &split_root(), ROSTER);
    let pair = router_cli::config_load::load(&root).expect("the pair loads");
    assert_eq!(
        format!("{:?}", pair.router.providers),
        format!("{:?}", inline.router.providers),
        "the joined roster is the inline roster, entry for entry"
    );
}
