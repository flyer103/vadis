//! CONF-117 (spec §9.4.1's exit-code row, ADR-057 §4 limb (g)): **the
//! surface refuses to invent.** Three refusal arms and one admission:
//!
//! - a `--session` with no records in the trace directory exits **2**
//!   with a message naming the trace dir — never an empty table and
//!   exit 0;
//! - a `--config` that does not load is a **load refusal** (exit 2),
//!   not a silent default;
//! - a missing `--config` (or a bad flag) is a **parse refusal** in the
//!   argument parser itself — the flag is required, so no discovery
//!   order runs behind the operator's back (spec §9.4.1: required,
//!   unlike `stats`);
//! - and the admission: the live parser **serves** `steps` — the
//!   relation `conf_43` polices between the docs and the CLI.

#![forbid(unsafe_code)]

mod common;

use common::{run_steps_session, STEPS_SESSION};

use clap::Parser as _;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_117_refuses_to_invent() {
    let (config_path, _records) = common::run_steps_session("conf117", false).await;
    let config = config_path.to_string_lossy().into_owned();

    // A session with no records: exit 2, never an empty report.
    let rc = vadis_cli::steps::steps(&config, "no-such-session", None, false);
    assert_eq!(rc, 2, "an absent session is a refusal (spec §9.4.1)");

    // A config that does not load: exit 2, the `stats` convention.
    let rc = vadis_cli::steps::steps("/nonexistent/vadis-config.yaml", STEPS_SESSION, None, false);
    assert_eq!(rc, 2, "an unusable config is a load refusal");

    // The served arm still prints and exits 0.
    let rc = vadis_cli::steps::steps(&config, STEPS_SESSION, None, false);
    assert_eq!(rc, 0, "a real session prints a report");

    // --config is required at the PARSER: no discovery order exists
    // behind it (a silent XDG default is the arm spec §9.4.1 refuses).
    assert!(
        vadis_cli::Cli::try_parse_from(["vadis", "steps", "--session", "s"]).is_err(),
        "a missing --config is a parse refusal, not a default"
    );
    // A bad flag is a parse refusal too.
    assert!(vadis_cli::Cli::try_parse_from([
        "vadis",
        "steps",
        "--session",
        "s",
        "--config",
        "c",
        "--nope",
    ])
    .is_err());
    // And the parser admits the well-formed form — the docs↔CLI
    // relation (CONF-43) holds with `steps` served, no deferral marker.
    assert!(
        vadis_cli::Cli::try_parse_from(["vadis", "steps", "--session", "s", "--config", "c",])
            .is_ok()
    );
}
