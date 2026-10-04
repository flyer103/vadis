//! R60-2's end-to-end witness, binary half (spec §4.11's rule-file lane;
//! ADR-046): a fresh `vadis setup` landing, served by the **real**
//! binary of this build, starts with a startup log **free of the named
//! absence** — no `transform_rules: rule file … No such file or
//! directory` note — while the same landing with its rule file deleted
//! (the RED control, the pre-fix world's shape) **does** carry the note.
//!
//! The rule-actually-applies half is deliberately NOT re-measured here:
//! it is already evidenced by `CONF-63`/`CONF-60` over the same
//! unchanged rule-file bytes (ADR-046; the round's card says so). The
//! in-process assembly half of the witness lives in `setup::tests`
//! (`the_landed_config_mounts_the_transform_engine`).
//!
//! Drives the real `vadis` binary found beside the test executable
//! (the same `cargo test --workspace` run builds it — CONF-86's lookup
//! trick), on a port probed free at the moment of use, and kills every
//! child it starts. std only: no new dependency (spec §4.11's boundary
//! table — "add a crate dependency" is a refusal row).

#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

/// The `vadis` binary of this build, beside the test executable
/// (`target/<profile>/deps/<this-test>-…` → `target/<profile>/vadis`).
fn router_binary() -> std::path::PathBuf {
    let exe = std::env::current_exe().expect("the test executable's path");
    exe.parent()
        .and_then(|deps| deps.parent())
        .map(|profile| profile.join(format!("vadis{}", std::env::consts::EXE_SUFFIX)))
        .expect("the test executable lives under target/<profile>/deps")
}

/// One scratch landing directory plus its config path.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vadis-r60-2-serve-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A TCP port that is free right now (bound to :0, read, released).
/// The child binds between our release and its own bind, so the run
/// retries on a fresh port if the race is lost (up to `attempts`).
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind 127.0.0.1:0 for a free port")
        .local_addr()
        .expect("the local address")
        .port()
}

/// Run the real binary's `setup` non-interactively into `dir`.
fn run_setup(config: &std::path::Path) {
    let out = Command::new(router_binary())
        .arg("setup")
        .arg("--non-interactive")
        .arg("--config")
        .arg(config)
        .output()
        .expect("spawn vadis setup");
    assert!(
        out.status.success(),
        "vadis setup failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Serve `config` until its startup log settles, then kill it and
/// return every line the process printed (stderr has the notes; the
/// listening line rides stderr too — `vadis listening on …`).
fn serve_and_collect(config: &std::path::Path) -> Vec<String> {
    let mut child = Command::new(router_binary())
        .arg("serve")
        .arg("--config")
        .arg(config)
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn vadis serve");
    let lines = drain_until_listening(&mut child);
    // The child is ours: kill it before asserting anything, so no
    // assertion failure can leak a listener.
    let _ = child.kill();
    let _ = child.wait();
    lines
}

/// Read the child's stderr lines (and stdout's, appended after) until
/// the listening line appears or the budget expires — whichever comes
/// first. A child that dies early returns what it printed, so the
/// assertion that follows names the real reason.
fn drain_until_listening(child: &mut Child) -> Vec<String> {
    let stderr = child.stderr.take().expect("piped stderr");
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let reader_seen = Arc::clone(&seen);
    let handle = std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            let mut v = reader_seen.lock().unwrap();
            let listening = line.contains("vadis listening on");
            v.push(line);
            drop(v);
            if listening {
                break;
            }
        }
    });
    // The startup budget: the store opens and migrates before the
    // listener binds; a few seconds is orders of magnitude above the
    // observed startup on this hardware.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if seen
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("vadis listening on"))
        {
            break;
        }
        if std::time::Instant::now() > deadline {
            break;
        }
        // A dead child will never listen: stop reading.
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    handle.join().expect("the stderr reader finishes");
    let mut v = seen.lock().unwrap().clone();
    // A note can land after the listening line only if it was printed
    // before it in program order; give the pipe a last drain beat so
    // ordering is not racy for the lines we assert on.
    std::thread::sleep(std::time::Duration::from_millis(100));
    v.extend(seen.lock().unwrap().iter().cloned());
    v.dedup();
    v
}

/// The landed tree's `server.addr` hand-edited to a port that is free
/// now — the one mutation the witness needs, on the config's own addr
/// line, exactly the field `vadis setup server` would have answered.
fn retarget_addr(config: &std::path::Path, port: u16) {
    let text = std::fs::read_to_string(config).unwrap();
    let line = text
        .lines()
        .find(|l| l.trim_start().starts_with("addr:"))
        .expect("the landed root carries server.addr");
    let new_line = line.replace("127.0.0.1:8790", &format!("127.0.0.1:{port}"));
    assert_ne!(&new_line, line, "the addr line carries the template's port");
    std::fs::write(config, text.replace(line, &new_line)).unwrap();
}

/// The round's real acceptance (limb 5b): a fresh landing served by the
/// real binary starts free of the named absence, and the same landing
/// with the rule file deleted (the RED control) carries it.
#[test]
fn serve_over_a_fresh_landing_has_no_rule_file_note() {
    let bin = router_binary();
    assert!(
        bin.is_file(),
        "the vadis binary of this build: {}",
        bin.display()
    );

    // GREEN arm: the untouched fresh landing.
    let dir = scratch("green");
    let config = dir.join("config.yaml");
    run_setup(&config);
    assert!(
        dir.join("rules/tool_output.toml").is_file(),
        "the fresh landing carries the rule file beside the config"
    );
    retarget_addr(&config, free_port());
    let green = serve_and_collect(&config);
    assert!(
        green.iter().any(|l| l.contains("vadis listening on")),
        "the green arm served (lines: {green:?})"
    );
    assert!(
        !green
            .iter()
            .any(|l| l.contains("transform_rules: rule file")),
        "the fresh landing's startup log carries no rule-file note (lines: {green:?})"
    );

    // RED control: the same landing with its rule file deleted — the
    // pre-fix world's exact shape (ADR-046's Background). The note IS
    // present here.
    let dir2 = scratch("red");
    let config2 = dir2.join("config.yaml");
    run_setup(&config2);
    std::fs::remove_file(dir2.join("rules/tool_output.toml")).unwrap();
    retarget_addr(&config2, free_port());
    let red = serve_and_collect(&config2);
    assert!(
        red.iter().any(|l| l.contains("vadis listening on")),
        "the red control served too (lines: {red:?})"
    );
    assert!(
        red.iter()
            .any(|l| l.contains("transform_rules: rule file")
                && l.contains("No such file or directory")),
        "the RED control carries the named absence (lines: {red:?})"
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dir2);
}
