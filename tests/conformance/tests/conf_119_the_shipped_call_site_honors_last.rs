//! CONF-119 (spec §9.4.1's `--last` row): **the shipped call site
//! honors `--last`** — pinned on the built binary's own stdout, not on
//! the builders. CONF-118 pins the shared rule (`shown_rows` /
//! `report_json(&rep, Some(last))`), but the review measured that
//! reverting the one line that passes `last` from the CLI arm to the
//! printer (`steps.rs`: `print_json(&rep, last)` → `print_json(&rep,
//! None)`) leaves CONF-118 **and** every `vadis-cli` unit test green —
//! the builder rule was pinned, the line the defect actually lived on
//! was not. This case is that line's tripwire: it drives the real
//! `vadis` binary of THIS build (CONF-86's precedent — found beside
//! the test executable) over a trace a real `serve` wrote (the shared
//! step-ledger rig, never a hand-written fixture) and reads the wire
//! output the user reads.
//!
//! Limbs, all over the seven-request canned session:
//!
//! - **`--last 3 --json`** — exactly 3 rows, the LAST three steps
//!   (5–7), `summary.steps` stays 7 (§9.4.5), and `showing` states
//!   `{last: 3, of: 7}`. **Red** for a call site that drops `last`;
//! - **no `--last`, `--json`** — all 7 rows and **no `showing`
//!   member at all** (absent, not null: nothing is hidden, so
//!   nothing is stated);
//! - **`--last 0 --json`** — zero rows, `showing.last == 0` (today's
//!   behaviour; the strict-flag question is registered, not this
//!   case's);
//! - **the proper-subset condition** (the amended §9.4.1 sentence):
//!   `showing` is present exactly when the printed rows are a proper
//!   subset of the session's steps — `--last 7` (n == N) prints all
//!   7 rows and states nothing, exactly like no `--last`.

#![forbid(unsafe_code)]

mod common;

use common::{run_steps_session, STEPS_SESSION};

/// The `vadis` binary of this build, beside the test executable
/// (`target/<profile>/deps/conf_119-…` → `target/<profile>/vadis`) —
/// CONF-86's precedent, the one way to assert on the shipped wire
/// output instead of the builders behind it.
fn vadis_binary() -> std::path::PathBuf {
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

/// One `vadis steps --json` run over the rig's config: (exit, stdout).
fn steps_json(
    bin: &std::path::Path,
    config: &std::path::Path,
    last: Option<usize>,
) -> (i32, serde_json::Value) {
    let mut cmd = std::process::Command::new(bin);
    cmd.arg("steps")
        .arg("--config")
        .arg(config)
        .arg("--session")
        .arg(STEPS_SESSION)
        .arg("--json");
    if let Some(n) = last {
        cmd.arg("--last").arg(n.to_string());
    }
    let out = cmd.output().expect("the binary runs");
    let code = out.status.code().expect("an exit code");
    let stdout = String::from_utf8(out.stdout).expect("stdout is UTF-8");
    let doc: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("--json parses: {e}: {stdout:?}"));
    (code, doc)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_119_the_shipped_call_site_honors_last() {
    let (config_path, records) = run_steps_session("conf119", false).await;
    assert_eq!(records.len(), 7, "the canned session is seven requests");
    let bin = vadis_binary();

    // -- --last 3 --json: the shipped surface truncates and states it
    let (code, v) = steps_json(&bin, &config_path, Some(3));
    assert_eq!(code, 0, "a report was printed");
    let rows = v["rows"].as_array().expect("rows is an array");
    assert_eq!(
        rows.len(),
        3,
        "exactly the last three rows, got {}",
        rows.len()
    );
    let shown: Vec<u64> = rows
        .iter()
        .map(|r| r["step"].as_u64().expect("step is an integer"))
        .collect();
    assert_eq!(shown, vec![5, 6, 7], "the LAST three steps");
    assert_eq!(
        v["summary"]["steps"], 7,
        "steps stays the pre---last count (§9.4.5)"
    );
    assert_eq!(v["summary"]["showing"]["last"], 3);
    assert_eq!(v["summary"]["showing"]["of"], 7);

    // -- no --last: every row, and no showing member at all (absent,
    //    not null — nothing is hidden, so nothing is stated).
    let (code, full) = steps_json(&bin, &config_path, None);
    assert_eq!(code, 0);
    assert_eq!(full["rows"].as_array().unwrap().len(), 7);
    assert!(
        full["summary"].get("showing").is_none(),
        "no --last states nothing, got: {}",
        full["summary"]
    );

    // -- --last 0: zero rows, the fact still stated (today's
    //    behaviour; the strict-flag question is registered elsewhere).
    let (code, zero) = steps_json(&bin, &config_path, Some(0));
    assert_eq!(code, 0);
    assert_eq!(zero["rows"].as_array().unwrap().len(), 0);
    assert_eq!(zero["summary"]["showing"]["last"], 0);
    assert_eq!(zero["summary"]["showing"]["of"], 7);

    // -- the proper-subset condition (the amended §9.4.1 sentence):
    //    n == N prints every row and states nothing, exactly like no
    //    --last. showing exists exactly when the printed rows are a
    //    proper subset of the session's steps.
    let (code, whole) = steps_json(&bin, &config_path, Some(7));
    assert_eq!(code, 0);
    assert_eq!(whole["rows"].as_array().unwrap().len(), 7);
    assert!(
        whole["summary"].get("showing").is_none(),
        "n == N hides nothing and states nothing, got: {}",
        whole["summary"]
    );
    assert_eq!(
        whole["rows"], full["rows"],
        "n == N prints the same rows as no --last"
    );
}
