//! `router serve` / `stats` / `replay` / `trace` (DESIGN §12.1).
//!
//! `serve` is fully config-driven (CONF-25) — the listen address,
//! the plugin set and the roster come from `--config`; no hardcoded default
//! survives in the serving path. The assembly lives in the library target
//! so conformance cases can exercise the same code path.

#![forbid(unsafe_code)]

use clap::Parser;
use router_cli::{Cli, Command};

fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        Command::Serve { config } => tokio::runtime::Runtime::new()
            .expect("tokio runtime")
            .block_on(router_cli::serve(&config)),
        Command::Stats {
            config,
            window,
            json,
        } => router_cli::stats::stats(&config, &window, json),
    };
    std::process::exit(code);
}
