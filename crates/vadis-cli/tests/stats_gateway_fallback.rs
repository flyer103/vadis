//! R72-FIX's witness (ADR-054 §4 / §6's CONF-106 sync-path requirement):
//! the `vadis stats` live-gateway fallback must work from the **sync CLI
//! path** — the real `vadis` binary as a subprocess, where no Tokio
//! runtime exists. A `#[tokio::test]` rig would install a reactor and
//! mask exactly the defect this round repairs (the old fallback built a
//! reqwest future and drove it with `futures::executor::block_on`, which
//! panics with "there is no reactor running" outside a runtime — and the
//! panic escaped through `stats`'s own frame, losing the whole report).
//!
//! Drives the real binary beside the test executable (CONF-86's lookup
//! trick), on a scratch config in the `config.example.yaml` shape, with
//! a live `serve` holding the store — the one trigger (spec §9.2: the
//! read-only open refused Locked/Busy). Three limbs:
//!
//! - **auth off** (the shipped default: `auth_token_env` absent): the
//!   call still goes out with no credential (ADR-054 §4 rule 3 — an
//!   unconfigured key never skips the attempt) and the endpoint answers
//!   `200` unauthenticated; the report carries the figure with its
//!   source marked `(gateway)`, exit 0, stdout non-empty, stderr free
//!   of the omission note;
//! - **auth on, token exported**: the same figure via the fallback,
//!   with the token presented (`Authorization: Bearer ***`;
//! - **`--json`**: `events.source == "gateway"` beside the value.
//!
//! std only; every child process is killed before any assertion.

#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

/// The `vadis` binary of this build, beside the test executable
/// (`target/<profile>/deps/<this-test>-…` → `target/<profile>/vadis`) —
/// CONF-86's lookup, reused because the surface under test is the
/// subprocess's own (no ambient runtime), which only the real binary
/// gives.
fn vadis_binary() -> std::path::PathBuf {
    let exe = std::env::current_exe().expect("the test executable's path");
    exe.parent()
        .and_then(|deps| deps.parent())
        .map(|profile| profile.join(format!("vadis{}", std::env::consts::EXE_SUFFIX)))
        .expect("the test executable lives under target/<profile>/deps")
}

/// A TCP port that is free right now (bound to :0, read, released).
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind 127.0.0.1:0 for a free port")
        .local_addr()
        .expect("the local address")
        .port()
}

/// A scratch landing in the `config.example.yaml` shape — the root of
/// `config.yaml` + `providers.example.yaml` + `rules/tool_output.toml`
/// copied beside it, with `server.addr` pointing at a free loopback
/// port. `auth_token_env` names `VADIS_R72FIX_TOKEN` only in the auth
/// shape (the shipped example keeps the key commented out — that is the
/// D2 limb's whole point).
fn scratch_config(tag: &str, auth: bool, port: u16) -> std::path::PathBuf {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|c| c.parent())
        .expect("the repo root, two levels above vadis-cli");
    let dir = std::env::temp_dir().join(format!(
        "vadis-r72fix-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(dir.join("rules")).unwrap();
    std::fs::copy(
        repo.join("providers.example.yaml"),
        dir.join("providers.example.yaml"),
    )
    .expect("the example roster copies");
    std::fs::copy(
        repo.join("rules/tool_output.toml"),
        dir.join("rules/tool_output.toml"),
    )
    .expect("the example rule file copies");
    let auth_line = if auth {
        "  auth_token_env: VADIS_R72FIX_TOKEN\n"
    } else {
        ""
    };
    let text = format!(
        "server:\n  addr: \"127.0.0.1:{port}\"\n{auth_line}  upstream_attempt_timeout: 60s\n  request_timeout: 10m\n\
session:\n  key_sources: [\"prompt_cache_key\"]\n  ttl: 12h\n\
cache:\n  sticky: true\n  breakeven:\n    enabled: true\n    min_remaining_turns: 3\n    safety_factor: 1.2\n\
trace:\n  dir: ./state/traces\n  rollover: hourly\n\
providers_file: providers.example.yaml\naliases: {{}}\nplugins: []\nfallback: []\n"
    );
    let config = dir.join("config.yaml");
    std::fs::write(&config, text).unwrap();
    config
}

/// Serve `config` until its listening line (stderr), then return the
/// child. The caller owns killing it.
fn serve_until_listening(config: &std::path::Path) -> Child {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    // Spec §5.1: a config that NAMES `auth_token_env` refuses startup
    // when the variable is unset — the auth-on rig must export the same
    // token the stats side will present.
    let token = std::env::var("VADIS_R72FIX_TOKEN").unwrap_or_else(|_| "sk-r72fix-witness".into());
    let mut child = Command::new(vadis_binary())
        .arg("serve")
        .arg("--config")
        .arg(config)
        .env("VADIS_R72FIX_TOKEN", token)
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn vadis serve");
    let stderr = child.stderr.take().expect("piped stderr");
    let up = Arc::new(AtomicBool::new(false));
    let reader_up = Arc::clone(&up);
    let handle = std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if line.contains("vadis listening on") {
                reader_up.store(true, Ordering::SeqCst);
                break;
            }
        }
    });
    // The store opens and migrates before the bind; seconds are orders
    // of magnitude above the observed startup. A dead child's reader
    // ends on EOF, so the loop is bounded either way.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !up.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            break; // it died; the assertion below names it
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    handle.join().expect("the stderr reader finishes");
    assert!(
        up.load(Ordering::SeqCst),
        "serve came up (config: {})",
        config.display()
    );
    child
}

/// One `vadis stats` run through the real binary: (exit, stdout, stderr).
fn stats(config: &std::path::Path, json: bool, token: Option<&str>) -> (i32, String, String) {
    let mut cmd = Command::new(vadis_binary());
    cmd.arg("stats")
        .arg("--config")
        .arg(config)
        .arg("--window")
        .arg("15m");
    if json {
        cmd.arg("--json");
    }
    match token {
        Some(t) => cmd.env("VADIS_R72FIX_TOKEN", t),
        None => cmd.env_remove("VADIS_R72FIX_TOKEN"),
    };
    let out = cmd.output().expect("the binary runs");
    (
        out.status.code().expect("an exit code"),
        String::from_utf8(out.stdout).expect("stdout is UTF-8"),
        String::from_utf8(out.stderr).expect("stderr is UTF-8"),
    )
}

/// The full report must be on stdout — the D1 regression's whole shape:
/// before the fix the panic killed the process (exit 101, stdout 0
/// bytes); after it the fallback's own thread is contained and the
/// report always lands.
fn assert_full_report(exit: i32, stdout: &str, stderr: &str, marker: &str) {
    assert_eq!(exit, 0, "stats exits 0 (stderr: {stderr})");
    assert!(
        stdout.contains("window:")
            && stdout.contains("requests")
            && stdout.contains("overhead p99"),
        "the FULL report is on stdout (got {} bytes)",
        stdout.len()
    );
    let line = stdout
        .lines()
        .find(|l| l.contains("unknown outcome requests"))
        .expect("the figure's line is printed");
    assert!(
        line.contains(marker),
        "the figure's source is marked ({marker}): {line}"
    );
    assert!(
        !stderr.contains("omitted"),
        "no omission note when the fallback served: {stderr}"
    );
}

/// (1) Auth OFF — the shipped default. The fallback still fires (the
/// attempt is never gated on auth being configured) and the endpoint
/// answers unauthenticated.
#[test]
fn fallback_serves_the_figure_with_auth_off() {
    let config = scratch_config("authoff", false, free_port());
    let mut serve = serve_until_listening(&config);
    let (exit, stdout, stderr) = stats(&config, false, None);
    let _ = serve.kill();
    let _ = serve.wait();
    assert_full_report(exit, &stdout, &stderr, "(gateway)");
    let _ = std::fs::remove_dir_all(config.parent().unwrap());
}

/// (2) Auth ON with the token exported — the credential is read from
/// the config's own `auth_token_env` and presented; same figure.
#[test]
fn fallback_serves_the_figure_with_auth_on() {
    let config = scratch_config("authon", true, free_port());
    let mut serve = serve_until_listening(&config);
    let (exit, stdout, stderr) = stats(&config, false, Some("sk-r72fix-witness"));
    let _ = serve.kill();
    let _ = serve.wait();
    assert_full_report(exit, &stdout, &stderr, "(gateway)");
    let _ = std::fs::remove_dir_all(config.parent().unwrap());
}

/// (3) The `--json` shape: the figure at its CONF-56-frozen top-level
/// key, `events.source == "gateway"` beside it (spec §9.2).
#[test]
fn json_names_the_gateway_source() {
    let config = scratch_config("json", false, free_port());
    let mut serve = serve_until_listening(&config);
    let (exit, stdout, stderr) = stats(&config, true, None);
    let _ = serve.kill();
    let _ = serve.wait();
    assert_eq!(exit, 0, "stats --json exits 0 (stderr: {stderr})");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("the report is JSON");
    assert!(
        v.get("unknown_outcome_requests").is_some(),
        "the top-level figure key is present"
    );
    assert_eq!(
        v["events"]["source"], "gateway",
        "events.source names the fallback (stderr: {stderr})"
    );
    let _ = std::fs::remove_dir_all(config.parent().unwrap());
}
