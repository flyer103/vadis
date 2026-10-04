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
pub mod config_diff;
pub mod cost;
pub mod error;
pub mod error_class;
pub mod peak;
pub mod plan;
pub mod prefix;
pub mod quota;
pub mod response_cache;
pub mod store;
pub mod trace;
pub mod transform;

pub use body::{RawBody, RawEditError, VADIS_OWNED_TOP_LEVEL_KEYS};
pub use breakeven::{decide_switch, BreakevenParams, StayReason, SwitchCandidate, SwitchVerdict};
pub use config::{
    AccountKind, CapUsdVal, ConfigError, ContextVal, DurationVal, OnPrimaryExhausted,
    PlanPolicyCfg, PriceCfg, RecoveryMode, RouteSpec, VadisConfig, TokensVal, KNOWN_BUILTIN_KINDS,
    KNOWN_SERVICE_SLOTS,
};
pub use config_diff::{changed_keys, ChangeKind, KeyChange};
pub use cost::{
    cost, select_band, CostBreakdown, Currency, Money, Nano, Price, PriceTable, TierTable, Usage,
};
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
pub use response_cache::{
    RecordedResponse, RequestFacts as CacheRequestFacts, ResponseCache, ResponseKey, ResponseStore,
    SourceRef, MAX_ENTRIES as CACHE_MAX_ENTRIES, MAX_STORED_BYTES as CACHE_MAX_STORED_BYTES,
};
pub use store::{
    write_intent_then, EventId, EventKind, LedgerBlock, NewEvent, PlanStateProjRow, Projection,
    ProjectionWrite, Query, QueryRow, RebuildStats, SessionBindingRow, Store, StoreError,
    StoredEvent, EVENT_SCHEMA_VERSION,
};
pub use trace::{
    verified_savings_tokens, CacheRec, CostRec, DecisionRecord, IdentityRec, NullTraceWriter,
    PlanSwitchRec, PrefixBlockRec, PrefixRec, ProtocolRec, QuotaAfter, ReplayedRef, ResultRec,
    StateRec, TraceError, TraceWriter, TransformRecord, TRACE_SCHEMA_VERSION,
};
