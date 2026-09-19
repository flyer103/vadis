//! Domain model and pure-function core (DESIGN §2/§12.1).
//!
//! Hard constraints: no dependency on any HTTP/protocol crate; money is
//! integer NanoUsd end to end (the ADR-006 ruling — no floats appear on the
//! decision path or in traces).

#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]

pub mod body;
pub mod breakeven;
pub mod config;
pub mod cost;
pub mod error;
pub mod peak;
pub mod quota;

pub use body::{RawBody, RawEditError, ROUTER_OWNED_TOP_LEVEL_KEYS};
pub use breakeven::{decide_switch, BreakevenParams, StayReason, SwitchCandidate, SwitchVerdict};
pub use config::{
    ConfigError, ContextVal, DurationVal, PriceCfg, RouteSpec, RouterConfig, TokensVal,
    KNOWN_BUILTIN_KINDS, KNOWN_SERVICE_SLOTS,
};
pub use cost::{cost, CostBreakdown, NanoUsd, Price, PriceTable, Usage};
pub use peak::{PeakTable, PeakWindow, Timestamp, Tz, Weekdays};
pub use quota::{charge, OverQuota, QuotaPlan, QuotaState, QuotaVerdict, QuotaWindow};
