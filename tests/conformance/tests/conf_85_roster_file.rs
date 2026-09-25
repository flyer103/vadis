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

// =========================================================================
// Half B — the identity (spec §4.14/§6/§9.1; ADR-037 D6; DESIGN §12.8's
// CONF-85 row): one byte digest over the pair, recomputed here
// **independently of the product** (`shasum -a 256`, the spec's own
// one-liner), must equal the value on the trace row, in the
// `config.applied` event and in `/health`'s `config` member — with the
// discriminators: a comment-only roster edit moves the digest and nothing
// else (no decision, no wire byte, no Nano figure); a price edit moves the
// digest AND the cost; the inline shape reports `roster_path: null` /
// `roster_sha16: ""` and still hashes to a stable digest.
//
// Red at the round's base `3db485c`: no surface carried the digest there
// (the record, the event and the health member all lacked the key), and
// the split root did not load at all.
// =========================================================================

/// The serving roster for half B: provider `p` on a loopback mock, one
/// model `m1`, the price parameterized so the price-edit leg can move it.
fn serving_roster(upstream_port: u16, input_miss: &str) -> String {
    format!(
        r#"providers:
  - name: p
    urls:
      chat: http://127.0.0.1:{upstream_port}/v1/chat/completions
    api_key_env: CONF85_P_KEY
    wire_api: chat
    supports: [chat]
    models:
      - id: m1
        context: 128k
        price:
          input_miss: {input_miss}
          input_hit: 0.0001
          cache_write: 0.0
          output: 0.002
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "mock upstream (no price; test fixture)"
"#
    )
}

/// A rigged `serve` on a fresh tempdir: the mock upstream, the config
/// (inline or pair), the env key present, one canned 200 with usage
/// queued. Returns the pieces the legs assert on.
struct Rig {
    addr: String,
    dir: std::path::PathBuf,
    upstream: testkit::MockUpstream,
    serve: tokio::task::JoinHandle<i32>,
    root: std::path::PathBuf,
    roster: Option<std::path::PathBuf>,
}

/// The **inline** rig: the roster lives in the root (the pre-R43 shape,
/// unchanged), served once so the trace row exists.
async fn identity_rig(tag: &str) -> Rig {
    let dir = testkit::tempdir(tag);
    let upstream = testkit::MockUpstream::start().await.unwrap();
    upstream.queue(testkit::CannedResponse::json(
        200,
        "OK",
        br#"{"id":"ok","choices":[{"index":0,"message":{"role":"assistant","content":"served"},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":5,"total_tokens":105,"prompt_tokens_details":{"cached_tokens":20}}}"#,
    ));
    let listen_port = testkit::free_port();
    let addr = format!("127.0.0.1:{listen_port}");
    let root = dir.join("config.yaml");
    std::fs::write(
        &root,
        format!(
            "{}{}{}",
            root_head(&addr),
            serving_roster(upstream.addr.port(), "0.001"),
            "aliases: {}\nplugins: []\nfallback: []\n"
        ),
    )
    .unwrap();
    std::env::set_var("CONF85_P_KEY", "sk-conf85");
    let cfg = root.to_string_lossy().into_owned();
    let serve = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
    testkit::wait_listening(&addr);
    Rig {
        addr,
        dir,
        upstream,
        serve,
        root,
        roster: None,
    }
}

/// The root's head (server/session/cache/trace), listen address spliced.
fn root_head(addr: &str) -> String {
    format!(
        "server:   {{ addr: \"{addr}\", upstream_attempt_timeout: 10s, request_timeout: 30s }}\nsession:  {{ key_sources: [\"prompt_cache_key\"], ttl: 11h }}\ncache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}\ntrace:    {{ dir: \"./state/traces\", rollover: hourly }}\n\n"
    )
}

/// One chat request for the rig's route, with a session key so the record
/// is the ordinary served class.
fn serve_one(rig: &Rig) -> u16 {
    let (status, _body, _headers) = testkit::http_post(
        &rig.addr,
        "/v1/chat/completions",
        br#"{"model":"p/m1","messages":[{"role":"user","content":"hi"}],"prompt_cache_key":"sess-85","stream":false}"#,
        &[],
    );
    assert_eq!(status, 200, "the rig serves its one request");
    status
}

/// The first 16 hex chars of `shasum -a 256` over a file's bytes — the
/// spec §4.14 recipe's first half, computed by the OS tool, not by the
/// product under test (DESIGN §12.8's CONF-85 half-B wording).
fn shasum16_of_file(path: &std::path::Path) -> String {
    let out = std::process::Command::new("shasum")
        .arg("-a")
        .arg("256")
        .arg(path)
        .output()
        .expect("shasum runs (the spec's recipe tool)");
    assert!(out.status.success(), "shasum failed on {}", path.display());
    let stdout = String::from_utf8(out.stdout).unwrap();
    stdout[..16].to_string()
}

/// `printf '%s:%s' <root_sha16> <roster_sha16> | shasum -a 256 | cut -c1-16`
/// — the spec's one-liner, run as written: the composition input carries
/// no trailing newline, and the roster half is the empty string when the
/// roster is inline.
fn recipe_digest(root: &std::path::Path, roster: Option<&std::path::Path>) -> String {
    let root_sha16 = shasum16_of_file(root);
    let roster_sha16 = roster.map(shasum16_of_file).unwrap_or_default();
    let mut child = std::process::Command::new("shasum")
        .arg("-a")
        .arg("256")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("shasum runs");
    use std::io::Write as _;
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(format!("{root_sha16}:{roster_sha16}").as_bytes())
        .unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap()[..16].to_string()
}

/// A blocking GET (the CONF-25 shape): status and body.
fn http_get(addr: &str, path: &str) -> (u16, String) {
    use std::io::{Read as _, Write as _};
    let mut stream = std::net::TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nconnection: close\r\n\r\n").as_bytes(),
        )
        .unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    let text = String::from_utf8_lossy(&buf).into_owned();
    let status: u16 = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("status line");
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}

/// Every trace record in the rig's dir, in write order (CONF-83's shape).
fn trace_records(dir: &std::path::Path) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir.join("state/traces")).expect("trace dir") {
        let entry = entry.unwrap();
        if entry.path().extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        for line in std::fs::read_to_string(entry.path()).unwrap().lines() {
            if !line.trim().is_empty() {
                out.push(serde_json::from_str(line).expect("record json"));
            }
        }
    }
    out
}

/// The `config.applied` event's payload (DESIGN §12.10.5 row 13), read
/// from the store after serve has stopped (the writer lock is the
/// process's own while it runs, CONF-23).
fn config_applied_payload(dir: &std::path::Path) -> serde_json::Value {
    let store = router_store::SqliteStore::open(&dir.join("state/router.db")).unwrap();
    use router_core::store::{Query, QueryRow, Store as _};
    let QueryRow::Events(events) = store.query(Query::AllEvents).unwrap() else {
        panic!("events");
    };
    let applied: Vec<_> = events
        .iter()
        .filter(|e| e.kind_raw == "config.applied")
        .collect();
    assert_eq!(applied.len(), 1, "exactly one config.applied row per boot");
    applied[0].payload.clone()
}

// (g) the split pair: the recipe's digest equals the value on the trace
// row, in the config.applied event and on /health's config member — one
// value, three surfaces, with the roster's path named on the two surface
// payloads, and schema_version unmoved at 2.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_85g_identity_agrees_across_the_three_surfaces() {
    let rig = identity_rig_with_roster("85g", "0.001").await;
    serve_one(&rig);

    // The independent recipe, from the two files alone.
    let digest = recipe_digest(&rig.root, rig.roster.as_deref());
    let root_sha16 = shasum16_of_file(&rig.root);
    let roster_sha16 = shasum16_of_file(rig.roster.as_ref().unwrap());

    // Surface 1: /health's config member (spec §9.1's five keys).
    let (status, body) = http_get(&rig.addr, "/health");
    assert_eq!(status, 200);
    let h: serde_json::Value = serde_json::from_str(&body).expect("health json");
    assert_eq!(
        h["config"]["root_path"],
        rig.root.to_string_lossy().as_ref()
    );
    assert_eq!(
        h["config"]["roster_path"],
        rig.roster.as_ref().unwrap().to_string_lossy().as_ref()
    );
    assert_eq!(h["config"]["root_sha16"], root_sha16);
    assert_eq!(h["config"]["roster_sha16"], roster_sha16);
    assert_eq!(h["config"]["config_digest"], digest);

    rig.serve.abort();
    drop(rig.serve);

    // Surface 2: the trace row (spec §6) — additive beside schema_version,
    // which stays 2.
    let records = trace_records(&rig.dir);
    assert_eq!(records.len(), 1, "one served request, one record");
    assert_eq!(records[0]["config_digest"], digest);
    assert_eq!(
        records[0]["schema_version"], 2,
        "an additive field never moves the version (ADR-037 D6)"
    );

    // Surface 3: the config.applied event (DESIGN §12.10.5 row 13).
    let payload = config_applied_payload(&rig.dir);
    assert_eq!(payload["config_digest"], digest);
    assert_eq!(payload["root_sha16"], root_sha16);
    assert_eq!(payload["roster_sha16"], roster_sha16);
    assert_eq!(payload["root_path"], rig.root.to_string_lossy().as_ref());
    assert_eq!(
        payload["roster_path"],
        rig.roster.as_ref().unwrap().to_string_lossy().as_ref()
    );
    // The pre-existing keys are unmoved (additive only).
    assert_eq!(payload["config_path"], rig.root.to_string_lossy().as_ref());
    assert!(payload["schema_version"].is_u64());
}

/// The split-shape rig whose roster text the caller controls (the
/// comment/price legs): the mock's port is spliced into the roster.
async fn identity_rig_with_roster(tag: &str, input_miss: &str) -> Rig {
    let dir = testkit::tempdir(tag);
    let upstream = testkit::MockUpstream::start().await.unwrap();
    upstream.queue(testkit::CannedResponse::json(
        200,
        "OK",
        br#"{"id":"ok","choices":[{"index":0,"message":{"role":"assistant","content":"served"},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":5,"total_tokens":105,"prompt_tokens_details":{"cached_tokens":20}}}"#,
    ));
    let listen_port = testkit::free_port();
    let addr = format!("127.0.0.1:{listen_port}");
    let root_text = format!(
        "{}providers_file: ./providers.yaml\n\naliases: {{}}\nplugins: []\nfallback: []\n",
        root_head(&addr)
    );
    let roster_body = serving_roster(upstream.addr.port(), input_miss);
    let (root, roster) = testkit::write_config_pair(&dir, &root_text, &roster_body);
    std::env::set_var("CONF85_P_KEY", "sk-conf85");
    let cfg = root.to_string_lossy().into_owned();
    let serve = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
    testkit::wait_listening(&addr);
    Rig {
        addr,
        dir,
        upstream,
        serve,
        root,
        roster: Some(roster),
    }
}

// (h) the discriminator pair: a comment-only roster edit moves the digest
// and NOTHING else (decision, wire bytes, Nano figures all unmoved); a
// price edit moves the digest AND the cost.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_85h_comment_moves_only_the_digest_and_price_moves_cost() {
    // Rig A: the plain pair. Rig B: the same pair with a comment-only
    // roster edit. Rig C: the same pair with a price edit.
    let a = identity_rig_with_roster("85h-a", "0.001").await;
    serve_one(&a);
    let b = identity_rig_with_roster_edited("85h-b", RosterEdit::Comment).await;
    let c = identity_rig_with_roster_edited("85h-c", RosterEdit::Price).await;
    serve_one(&b);
    serve_one(&c);

    let digest_a = recipe_digest(&a.root, a.roster.as_deref());
    let digest_b = recipe_digest(&b.root, b.roster.as_deref());
    let digest_c = recipe_digest(&c.root, c.roster.as_deref());

    for rig in [&a, &b, &c] {
        rig.serve.abort();
    }
    drop(a.serve);
    drop(b.serve);
    drop(c.serve);

    let rec_a = trace_records(&a.dir).pop().expect("a record");
    let rec_b = trace_records(&b.dir).pop().expect("b record");
    let rec_c = trace_records(&c.dir).pop().expect("c record");

    // The digest on each row equals the recipe over that rig's files.
    assert_eq!(rec_a["config_digest"], digest_a);
    assert_eq!(rec_b["config_digest"], digest_b);
    assert_eq!(rec_c["config_digest"], digest_c);
    // Both edits move the digest.
    assert_ne!(digest_a, digest_b, "a comment-only edit moves the digest");
    assert_ne!(digest_a, digest_c, "a price edit moves the digest");

    // The comment edit moves NOTHING else: same decision, same bytes on
    // the wire, same Nano figures.
    assert_eq!(rec_a["decision"], rec_b["decision"], "decision unmoved");
    assert_eq!(rec_a["cost"], rec_b["cost"], "no Nano figure moved");
    assert_eq!(rec_a["usage"], rec_b["usage"], "usage unmoved");
    let wire_a = &a.upstream.requests()[0].body;
    let wire_b = &b.upstream.requests()[0].body;
    assert_eq!(wire_a, wire_b, "the upstream-visible bytes are identical");

    // The price edit moves the digest AND the cost — the pair
    // discriminates: cost follows the price, not the comment.
    assert_eq!(
        rec_a["decision"], rec_c["decision"],
        "decision unmoved by a price"
    );
    let wire_c = &c.upstream.requests()[0].body;
    assert_eq!(
        wire_a, wire_c,
        "the wire bytes are identical under a price edit"
    );
    assert!(
        rec_c["cost"]["total"].as_u64().unwrap() > rec_a["cost"]["total"].as_u64().unwrap(),
        "the higher input_miss prices higher: {} vs {}",
        rec_c["cost"]["total"],
        rec_a["cost"]["total"]
    );
    assert!(
        rec_a["cost"]["total"].as_u64().unwrap() > 0,
        "the control is priced at all"
    );
}

enum RosterEdit {
    Comment,
    Price,
}

/// The same rig as `identity_rig_with_roster(_, "0.001")` but with one
/// edit applied to the roster's bytes: a comment line (no parsed change)
/// or a price figure (a parsed change).
async fn identity_rig_with_roster_edited(tag: &str, edit: RosterEdit) -> Rig {
    let dir = testkit::tempdir(tag);
    let upstream = testkit::MockUpstream::start().await.unwrap();
    upstream.queue(testkit::CannedResponse::json(
        200,
        "OK",
        br#"{"id":"ok","choices":[{"index":0,"message":{"role":"assistant","content":"served"},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":5,"total_tokens":105,"prompt_tokens_details":{"cached_tokens":20}}}"#,
    ));
    let listen_port = testkit::free_port();
    let addr = format!("127.0.0.1:{listen_port}");
    let root_text = format!(
        "{}providers_file: ./providers.yaml\n\naliases: {{}}\nplugins: []\nfallback: []\n",
        root_head(&addr)
    );
    let base = serving_roster(upstream.addr.port(), "0.001");
    let roster_body = match edit {
        // A comment-only edit: the parsed roster is unchanged, the bytes
        // move — the byte digest is stricter than a parsed-value digest,
        // deliberately (the comments carry the price citations, §4.0).
        RosterEdit::Comment => format!("# a provenance note, not a value\n{base}"),
        RosterEdit::Price => base.replace("input_miss: 0.001", "input_miss: 0.009"),
    };
    let (root, roster) = testkit::write_config_pair(&dir, &root_text, &roster_body);
    std::env::set_var("CONF85_P_KEY", "sk-conf85");
    let cfg = root.to_string_lossy().into_owned();
    let serve = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
    testkit::wait_listening(&addr);
    Rig {
        addr,
        dir,
        upstream,
        serve,
        root,
        roster: Some(roster),
    }
}

// (i) the inline shape: `roster_path: null` and `roster_sha16: ""` (the
// two spellings of one fact, §9.1), and the digest is the stable recipe
// over the root alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_85i_inline_shape_reports_null_roster_and_still_hashes() {
    let rig = identity_rig("85i").await;
    assert!(rig.roster.is_none(), "the inline rig names no roster file");
    serve_one(&rig);

    let digest = recipe_digest(&rig.root, None);
    let root_sha16 = shasum16_of_file(&rig.root);

    let (status, body) = http_get(&rig.addr, "/health");
    assert_eq!(status, 200);
    let h: serde_json::Value = serde_json::from_str(&body).expect("health json");
    assert_eq!(
        h["config"]["roster_path"],
        serde_json::Value::Null,
        "no second file is null, not a guessed path"
    );
    assert_eq!(
        h["config"]["roster_sha16"], "",
        "the hash input that is not there is the empty string"
    );
    assert_eq!(h["config"]["root_sha16"], root_sha16);
    assert_eq!(h["config"]["config_digest"], digest);

    rig.serve.abort();
    drop(rig.serve);

    let records = trace_records(&rig.dir);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["config_digest"], digest);

    let payload = config_applied_payload(&rig.dir);
    assert_eq!(payload["config_digest"], digest);
    assert_eq!(payload["roster_path"], serde_json::Value::Null);
    assert_eq!(payload["roster_sha16"], "");
}
