//! 领域模型与纯函数核心（DESIGN §2/§12.1）。
//!
//! 硬约束：不依赖任何 HTTP / 协议 crate；金额全程整数 NanoUsd（ADR-006 裁定口径，
//! 决定路径与 trace 中不出现浮点）。

#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]

pub mod body;
pub mod breakeven;
pub mod cost;
pub mod error;
pub mod peak;
pub mod quota;

pub use body::{RawBody, RawEditError, ROUTER_OWNED_TOP_LEVEL_KEYS};
pub use breakeven::{decide_switch, BreakevenParams, StayReason, SwitchCandidate, SwitchVerdict};
pub use cost::{cost, CostBreakdown, NanoUsd, Price, PriceTable, Usage};
pub use peak::{PeakTable, PeakWindow, Timestamp, Tz, Weekdays};
pub use quota::{charge, OverQuota, QuotaPlan, QuotaState, QuotaVerdict, QuotaWindow};
