//! CONF-86 (spec §4.11's `--check` row + the D9 paragraph at
//! `docs/spec.md:1079-1083`; ADR-038 D9; **R45-1-F2**): **`vadis setup
//! --check` states the roster fact for an inline root, and only for an
//! inline root — pinned on the binary's wire output.**
//!
//! Over a root whose roster is inline, `vadis setup --check` prints one
//! `roster: inline in this file — a writing run moves it to <name>` line
//! above the key rows and carries a `roster` member in `--json`; over an
//! already-split root it prints NEITHER, and the key rows, the export
//! snippets and the exit codes are identical in both shapes. The behavior
//! is ADR-038 D9's (shipped with R45); what R45-1-F2 measured missing is
//! the assertion — no case pinned the line, so a regression of it would
//! have been invisible. Both directions bite here: (a) inline ⇒ the line
//! and the member; (b) split ⇒ their absence, with the inline arm's own
//! output minus its first line as the identical-rows relation.
//!
//! **This case was authorized by the human on 2026-09-26** (the gate side
//! is normally the owner's: ADR-012, AGENTS constraint 9). It ADDS an
//! assertion and moves no existing one: no `TRACE_SCHEMA_VERSION` move, no
//! other conformance case touched, no fixture edited — the fixtures below
//! are built in the case's own temp dirs.
//!
//! The surface is the binary's stdout, so the case drives the real
//! `vadis` binary of THIS build (found beside the test executable — the
//! same `cargo test --workspace` run builds it) as a subprocess, the one
//! way to assert on the wire output. Green at the round's base by
//! construction: the behavior it pins shipped with R45.

#![forbid(unsafe_code)]

use vadis_conformance::testkit;

/// The server-section head every root below shares (the CONF-85 shape).
const ROOT_HEAD: &str = r#"
server:   { addr: "127.0.0.1:39785", upstream_attempt_timeout: 45s, request_timeout: 9m }
session:  { key_sources: ["prompt_cache_key"], ttl: 11h }
cache:    { sticky: true, breakeven: { enabled: true, min_remaining_turns: 2, safety_factor: 1.1 } }
trace:    { dir: "./state/traces", rollover: hourly }

"#;

/// One provider entry naming one environment variable — the same variable
/// in both shapes, so the key rows are the relation's invariant half.
const ROSTER: &str = r#"providers:
  - name: p
    urls:
      chat: https://only.example/v1/chat/completions
    api_key_env: CONF86_P_KEY
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

const ROOT_TAIL: &str = "aliases: {}\nplugins: []\nfallback: []\n";

/// The roster name the shipped root template names — the `<name>` the
/// fact line states (ADR-038 D2: the default comes from the shipped
/// template, never from a constant in code; pinned here as the wire
/// fact's own word).
const SHIPPED_ROSTER_NAME: &str = "providers.example.yaml";

/// The `vadis` binary of this build, beside the test executable
/// (`target/<profile>/deps/conf_86-…` → `target/<profile>/vadis`).
fn router_binary() -> std::path::PathBuf {
    let exe = std::env::current_exe().expect("the test executable's path");
    let bin = exe
        .parent()
        .and_then(|deps| deps.parent())
        .map(|profile| profile.join(format!("vadis{}", std::env::consts::EXE_SUFFIX)))
        .expect("the test executable lives under target/<profile>/deps");
    assert!(
        bin.is_file(),
        "the vadis binary of THIS build must exist at {} — it is built by the same \
         `cargo build --workspace` / `cargo test --workspace` run this case rides on",
        bin.display()
    );
    bin
}

/// One `vadis setup --check` run over `root`, with the roster's one
/// named variable present in the environment (so the exit code is the
/// load's verdict, 0, never the environment's, 4): (exit, stdout, stderr).
fn check(bin: &std::path::Path, root: &std::path::Path, json: bool) -> (i32, String, String) {
    let mut cmd = std::process::Command::new(bin);
    cmd.arg("setup").arg("--check").arg("--config").arg(root);
    if json {
        cmd.arg("--json");
    }
    let out = cmd.output().expect("the binary runs");
    (
        out.status.code().expect("an exit code"),
        String::from_utf8(out.stdout).expect("stdout is UTF-8"),
        String::from_utf8(out.stderr).expect("stderr is UTF-8"),
    )
}

/// The inline rig: the roster block lives in the root. Returns (dir,
/// root); `dir` must outlive the runs (it is the root's parent).
fn inline_root(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = testkit::tempdir(tag);
    let root = dir.join("config.yaml");
    std::fs::write(&root, format!("{ROOT_HEAD}{ROSTER}{ROOT_TAIL}")).unwrap();
    (dir, root)
}

/// The split rig: the same roster byte-moved into the file the root
/// names (ADR-037 D3's move, performed by hand here).
fn split_pair(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = testkit::tempdir(tag);
    let root = dir.join("config.yaml");
    std::fs::write(
        &root,
        format!("{ROOT_HEAD}providers_file: ./providers.yaml\n\n{ROOT_TAIL}"),
    )
    .unwrap();
    std::fs::write(dir.join("providers.yaml"), ROSTER).unwrap();
    (dir, root)
}

// (a) the inline shape ⇒ the fact line heads the key rows, the `--json`
// document carries the `roster` member, the exit code is the load's, and
// the read-only run writes nothing (no roster file appears).
#[test]
fn conf_86a_inline_root_states_the_roster_fact() {
    std::env::set_var("CONF86_P_KEY", "sk-conf86");
    let bin = router_binary();
    let (dir, root) = inline_root("86a");
    let line =
        format!("roster: inline in this file — a writing run moves it to {SHIPPED_ROSTER_NAME}");

    let (code, stdout, _stderr) = check(&bin, &root, false);
    assert_eq!(code, 0, "the pair loads and its one variable is present");
    assert_eq!(
        stdout.matches("roster:").count(),
        1,
        "exactly one roster-fact line, got: {stdout:?}"
    );
    assert_eq!(
        stdout.lines().next(),
        Some(line.as_str()),
        "the fact line heads the key rows (one line above them), got: {stdout:?}"
    );
    assert!(
        stdout.contains("CONF86_P_KEY: present\n"),
        "the key rows follow, got: {stdout:?}"
    );

    let (code, stdout, _) = check(&bin, &root, true);
    assert_eq!(code, 0);
    let doc: serde_json::Value = serde_json::from_str(&stdout).expect("--json parses");
    assert_eq!(
        doc["roster"],
        serde_json::json!({ "inline": true, "splits_on_write_to": SHIPPED_ROSTER_NAME }),
        "the --json document carries the roster member, got: {doc}"
    );

    // The read-only surface writes nothing: no roster file appears, and
    // the root's bytes are unmoved.
    assert!(
        !dir.join(SHIPPED_ROSTER_NAME).exists(),
        "a --check run moves nothing"
    );
    assert_eq!(
        std::fs::read(&root).unwrap(),
        format!("{ROOT_HEAD}{ROSTER}{ROOT_TAIL}").as_bytes()
    );
}

// (b) the split shape ⇒ NEITHER the line nor the member — and the rest
// of the surface is identical in both shapes: the inline arm's stdout
// minus its first line IS the split arm's stdout, the `--json` documents
// differ only in the `roster` member, and the exit codes are equal.
#[test]
fn conf_86b_split_root_states_nothing_and_the_rows_are_identical() {
    std::env::set_var("CONF86_P_KEY", "sk-conf86");
    let bin = router_binary();
    let (_dir_i, root_i) = inline_root("86b-inline");
    let (_dir_s, root_s) = split_pair("86b-split");

    let (code_i, text_i, _) = check(&bin, &root_i, false);
    let (code_s, text_s, _) = check(&bin, &root_s, false);
    assert_eq!(code_s, 0, "the split pair loads");
    assert_eq!(
        text_s.matches("roster:").count(),
        0,
        "an already-split root prints no roster-fact line, got: {text_s:?}"
    );
    // The identical-rows relation: the inline output without its one fact
    // line is the split output, byte for byte (same key rows, same export
    // snippets — here none, both variables present).
    let inline_minus_fact = text_i
        .strip_prefix(&format!(
            "roster: inline in this file — a writing run moves it to {SHIPPED_ROSTER_NAME}\n"
        ))
        .expect("the inline output heads with its one fact line");
    assert_eq!(
        inline_minus_fact, text_s,
        "the key rows are identical in both shapes"
    );
    assert_eq!(code_i, code_s, "the exit codes are identical");

    let (_, json_i, _) = check(&bin, &root_i, true);
    let (_, json_s, _) = check(&bin, &root_s, true);
    let doc_i: serde_json::Value = serde_json::from_str(&json_i).expect("inline --json parses");
    let doc_s: serde_json::Value = serde_json::from_str(&json_s).expect("split --json parses");
    assert!(
        doc_s.get("roster").is_none(),
        "an already-split root carries no roster member, got: {doc_s}"
    );
    assert!(doc_i.get("roster").is_some(), "the inline arm carries it");
    // The documents differ ONLY in the roster member (the paths are the
    // rigs' own; normalized out before the comparison).
    let mut norm_i = doc_i.clone();
    let mut norm_s = doc_s.clone();
    for d in [&mut norm_i, &mut norm_s] {
        let o = d.as_object_mut().expect("an object");
        o.remove("config");
        o.remove("roster");
    }
    assert_eq!(
        norm_i, norm_s,
        "the key rows and selected_by are identical in both shapes"
    );
}
