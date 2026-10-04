//! `vadis serve` / `stats` / `setup` (DESIGN §12.1; `replay` and `trace`
//! are named in the plan, not yet served).
//!
//! `serve` is fully config-driven (CONF-25) — the listen address, the
//! plugin set and the roster come from the config file; no hardcoded
//! default survives in the serving path. The assembly lives in the
//! library target so conformance cases can exercise the same code path.
//!
//! The config file's location is resolved **once here** (spec §4.12's
//! discovery order, `config_path::resolve_read`) and handed on as an
//! absolute path, so `vadis_cli::serve(&str)` and `stats::stats(&str, …)`
//! keep the signatures their rigs already drive (CONF-23/25/43).

#![forbid(unsafe_code)]

use clap::Parser;
use vadis_cli::{Cli, Command};

fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        Command::Serve { config } => {
            let resolved = match vadis_cli::config_path::resolve_read(config.as_deref()) {
                Ok(r) => r,
                Err(reason) => {
                    eprintln!("vadis: {reason}");
                    std::process::exit(2);
                }
            };
            tokio::runtime::Runtime::new()
                .expect("tokio runtime")
                .block_on(vadis_cli::serve(&resolved.path.to_string_lossy()))
        }
        Command::Stats {
            config,
            window,
            json,
        } => {
            let resolved = match vadis_cli::config_path::resolve_read(config.as_deref()) {
                Ok(r) => r,
                Err(reason) => {
                    eprintln!("vadis: {reason}");
                    std::process::exit(2);
                }
            };
            vadis_cli::stats::stats(&resolved.path.to_string_lossy(), &window, json)
        }
        Command::Setup {
            section,
            config,
            from,
            non_interactive,
            quick,
            print,
            check,
            json,
            dry_run,
            force,
            backup,
        } => vadis_cli::setup::run(vadis_cli::setup::SetupArgs {
            section,
            config,
            from,
            non_interactive,
            quick,
            print,
            json,
            check,
            dry_run,
            force,
            backup,
        }),
    };
    std::process::exit(code);
}
