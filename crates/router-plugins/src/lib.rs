//! Built-in tier-A plugins: cache_guard / transform_rules / cost_ledger /
//! quota_guard / sticky (DESIGN §2). The transform_rules engine (ADR-019
//! §7, DESIGN §12.12 order ④) is implemented; the rest are v0.1 stubs.

#![forbid(unsafe_code)]

pub mod transform_rules;

pub use transform_rules::{load_path, load_str, FailedRule, LoadReport, TransformRulesEngine};
