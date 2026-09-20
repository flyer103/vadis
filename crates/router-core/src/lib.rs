//! Domain model and pure-function core (DESIGN §2/§12.1).
//!
//! Hard constraints: no dependency on any HTTP/protocol crate; money is
//! integer Nano end to end (the ADR-006 ruling — no floats appear on the
//! decision path or in traces).

#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]

pub mod body;
pub mod breakeven;
pub mod config;
pub mod cost;
pub mod error;
pub mod error_class;
pub mod peak;
pub mod plan;
pub mod prefix;
pub mod quota;
pub mod store;
pub mod trace;
pub mod transform;

pub use body::{RawBody, RawEditError, ROUTER_OWNED_TOP_LEVEL_KEYS};
pub use breakeven::{decide_switch, BreakevenParams, StayReason, SwitchCandidate, SwitchVerdict};
pub use config::{
    AccountKind, CapUsdVal, ConfigError, ContextVal, DurationVal, OnPrimaryExhausted,
    PlanPolicyCfg, PriceCfg, RecoveryMode, RouteSpec, RouterConfig, TokensVal, KNOWN_BUILTIN_KINDS,
    KNOWN_SERVICE_SLOTS,
};
pub use cost::{cost, CostBreakdown, Currency, Money, Nano, Price, PriceTable, Usage};
pub use error::ErrorCode;
pub use error_class::{
    classify_upstream_error, Classification, Demotion, ErrorClass, ErrorEvidence, TransportCause,
    DEFAULT_COOLDOWN,
};
pub use peak::{PeakTable, PeakWindow, Timestamp, Tz, Weekdays};
pub use plan::{
    route_in_family, PlanAccount, PlanFirstRule, PlanMove, PlanRequest, PlanStateRow,
    ProbeBlockedBy, REASON_PRIMARY_EXHAUSTED, REASON_PRIMARY_RECOVERED,
};
pub use prefix::{
    attribute_tokens, body_sha16, extract_prefix_blocks, prefix_continuity, BlockKind, PrefixBlock,
};
pub use quota::{
    charge, window_start_for, OverQuota, QuotaPlan, QuotaState, QuotaVerdict, QuotaWindow,
};
pub use store::{
    write_intent_then, EventId, EventKind, LedgerBlock, NewEvent, PlanStateProjRow, Projection,
    ProjectionWrite, Query, QueryRow, RebuildStats, SessionBindingRow, Store, StoreError,
    StoredEvent, EVENT_SCHEMA_VERSION,
};
pub use trace::{
    verified_savings_tokens, CostRec, DecisionRecord, IdentityRec, NullTraceWriter, PlanSwitchRec,
    PrefixBlockRec, PrefixRec, ProtocolRec, QuotaAfter, ResultRec, StateRec, TraceError,
    TraceWriter, TransformRecord, TRACE_SCHEMA_VERSION,
};
