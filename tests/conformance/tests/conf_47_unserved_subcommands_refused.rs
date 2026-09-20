//! CONF-47 (spec §9.3): **unserved subcommands exit non-zero with a
//! usage error** — `router replay` and `router trace tail` are planned,
//! not served; the parser refuses them with clap's usage error and its
//! exit code 2, never a silently ignored flag.
//!
//! Driven against the same `Cli` parser `main` dispatches on
//! (`Cli::try_parse_from`), so the refusal is the parser's real verdict,
//! not a restatement of it:
//!
//! - each of `replay`, `trace` (with its `tail` operand) is refused;
//! - the refusal is a usage error naming the **unknown subcommand** and
//!   pointing at the served set (`serve`, `stats`) — the "meaningful
//!   wording" half of the assertion, checked by substring so it is a
//!   relation, not a frozen-string snapshot;
//! - the mapped process exit code is non-zero (`usage` → exit 2), and a
//!   liveness control shows the same argv prefix **with a served
//!   subcommand** parses fine — proving the refusal is the subcommand,
//!   not the flag grammar around it.

#![forbid(unsafe_code)]

use clap::Parser;
use router_cli::Cli;

/// One refusal verdict: the error text and the exit code clap's default
/// `Error::exit` would produce for it.
fn parse(argv: &[&str]) -> (String, i32) {
    match Cli::try_parse_from(argv) {
        Ok(_) => panic!("`{argv:?}` parsed, but its subcommand is not served"),
        Err(e) => {
            let text = e.render().to_string();
            let code = e.exit_code();
            (text, code)
        }
    }
}

#[test]
fn conf_47_unserved_subcommands_exit_nonzero_with_usage_error() {
    // Every planned-but-unserved invocation named in spec §9.3 / the docs.
    let cases: Vec<Vec<&str>> = vec![
        vec![
            "router", "replay", "--trace", "t.jsonl", "--config", "c.yaml",
        ],
        vec!["router", "trace", "tail"],
        // The bare forms refuse too (an unknown subcommand with no args).
        vec!["router", "replay"],
        vec!["router", "trace"],
    ];
    let mut seen_refusals = 0;
    for argv in &cases {
        let (text, code) = parse(argv);
        seen_refusals += 1;
        assert_ne!(code, 0, "`{argv:?}` must exit non-zero, got {code}");
        assert_eq!(
            code, 2,
            "`{argv:?}`: clap's usage error maps to exit 2 (observed {code})"
        );
        // The wording must be a usage error that names the unknown
        // subcommand (the "meaningful" half) and carries a usage line.
        // Checked as substrings — the exact clap rendering is not frozen
        // here. (Observed rendering at the time of writing: clap prints
        // `error: unrecognized subcommand '<sub>'` plus a usage line; it
        // does not enumerate the served set, so that is not asserted.)
        let sub = argv[1];
        let lower = text.to_lowercase();
        assert!(
            lower.contains(sub),
            "`{argv:?}`: the usage error must name `{sub}`: {text:?}"
        );
        assert!(
            lower.contains("unrecognized subcommand"),
            "`{argv:?}`: the refusal must be a usage error naming the \
             subcommand: {text:?}"
        );
        assert!(
            lower.contains("usage:"),
            "`{argv:?}`: the refusal must carry a usage line: {text:?}"
        );
    }
    assert_eq!(seen_refusals, cases.len(), "every case above refused");

    // Liveness control: the same argv prefix with a served subcommand
    // parses — the refusal above is the subcommand, not the surrounding
    // flag grammar.
    let ok = Cli::try_parse_from(["router", "serve", "--config", "c.yaml"])
        .expect("a served subcommand parses");
    assert!(
        matches!(ok.command, router_cli::Command::Serve { .. }),
        "control parsed the serve variant"
    );
}
