//! Built-in tier-A plugins: cache_guard / transform_rules / cost_ledger /
//! quota_guard / sticky (DESIGN §2). The transform_rules engine (ADR-019
//! §7, DESIGN §12.12 order ④) is implemented; the rest are v0.1 stubs.
//!
//! Since R41-3 this crate also carries the **assembly** (`assembly.rs`):
//! the factory/registry that maps the declared `plugins:` list to plugin
//! instances and drives `router-runtime`'s loader with them (ADR-036
//! D1/D8). The launcher in `router-cli` calls `assemble` once and reads
//! the assembled context's typed slots; no plugin-kind branching lives
//! there.

#![forbid(unsafe_code)]

pub mod assembly;
pub mod transform_rules;

pub use assembly::{assemble, Assembly, TransformChain, TRANSFORM_CHAIN};
pub use transform_rules::{load_path, load_str, FailedRule, LoadReport, TransformRulesEngine};
