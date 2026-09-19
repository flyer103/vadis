//! The store seam (DESIGN §12.10.4, ADR-009/ADR-010).
//!
//! `events` is the state truth; `sessions` / `cache_ledger` /
//! `quota_counters` / `provider_cooldown` are projections that may be
//! dropped and rebuilt from the log. This module holds only the **types
//! and the write-ordering rule** — `router-core` stays I/O-free; the
//! SQLite implementation lives in `crates/router-store`.

use serde_json::Value;

/// The ordering anchor and the second half of the trace join key
/// (`request_id` + `event_id`, spec §4.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EventId(pub i64);

/// The event vocabulary's own version, stamped per row (`events.schema_version`)
/// because the log is the truth: old rows are never rewritten and readers
/// upcast (ADR-009 item 7).
pub const EVENT_SCHEMA_VERSION: i64 = 1;

/// The closed event-kind set (ADR-010 item 2, plus `error.classified` from
/// ADR-011 and `restart.marked` from ADR-010 item 4). Enforced in code, not
/// in a SQL `CHECK`, so the vocabulary can grow without a DDL migration
/// (DESIGN §12.10.4); readers tolerate an unknown kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// Inbound body read and hashed, before any decision. FULL.
    RequestReceived,
    /// A route was chosen and the guards passed. NORMAL.
    DecisionMade,
    /// One transform step changed the payload. NORMAL.
    TransformApplied,
    /// A sticky binding was created or moved. FULL.
    SessionBound,
    /// **The intent**: committed before the upstream attempt starts. FULL.
    UpstreamSubmitted,
    /// The response head/body completed (or failed). FULL.
    UpstreamResponded,
    /// An upstream error was classified (ADR-011). NORMAL.
    ErrorClassified,
    /// A route switch, before the next intent. FULL.
    FailoverTriggered,
    /// Usage is known and the five-tier cost computed. FULL.
    CostComputed,
    /// Charged against a plan, before the response is released. FULL.
    QuotaCharged,
    /// A plugin reached ACTIVE (ADR-011's load edge). NORMAL.
    PluginLoaded,
    /// A plugin's effects were fully rolled back. NORMAL.
    PluginUnloaded,
    /// A config was validated and applied. FULL.
    ConfigApplied,
    /// A restart marker for `unknown_outcome` accounting (ADR-010 item 4).
    /// FULL: it must survive the very crash it records.
    RestartMarked,
}

impl EventKind {
    /// The wire form stored in `events.kind`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RequestReceived => "request.received",
            Self::DecisionMade => "decision.made",
            Self::TransformApplied => "transform.applied",
            Self::SessionBound => "session.bound",
            Self::UpstreamSubmitted => "upstream.submitted",
            Self::UpstreamResponded => "upstream.responded",
            Self::ErrorClassified => "error.classified",
            Self::FailoverTriggered => "failover.triggered",
            Self::CostComputed => "cost.computed",
            Self::QuotaCharged => "quota.charged",
            Self::PluginLoaded => "plugin.loaded",
            Self::PluginUnloaded => "plugin.unloaded",
            Self::ConfigApplied => "config.applied",
            Self::RestartMarked => "restart.marked",
        }
    }

    /// Parse a stored kind; `None` for a kind this binary does not know —
    /// rows written by a newer binary stay readable (ADR-009 item 7).
    pub fn from_str_lossy(s: &str) -> Option<Self> {
        Some(match s {
            "request.received" => Self::RequestReceived,
            "decision.made" => Self::DecisionMade,
            "transform.applied" => Self::TransformApplied,
            "session.bound" => Self::SessionBound,
            "upstream.submitted" => Self::UpstreamSubmitted,
            "upstream.responded" => Self::UpstreamResponded,
            "error.classified" => Self::ErrorClassified,
            "failover.triggered" => Self::FailoverTriggered,
            "cost.computed" => Self::CostComputed,
            "quota.charged" => Self::QuotaCharged,
            "plugin.loaded" => Self::PluginLoaded,
            "plugin.unloaded" => Self::PluginUnloaded,
            "config.applied" => Self::ConfigApplied,
            "restart.marked" => Self::RestartMarked,
            _ => return None,
        })
    }

    /// The durability tier (ADR-009 item 4's single question: *may this
    /// fact be recomputed?*). No → FULL, one transaction per event,
    /// committed before the effect it authorizes. Yes → NORMAL, batched.
    pub const fn is_full(self) -> bool {
        matches!(
            self,
            Self::RequestReceived
                | Self::SessionBound
                | Self::UpstreamSubmitted
                | Self::UpstreamResponded
                | Self::FailoverTriggered
                | Self::CostComputed
                | Self::QuotaCharged
                | Self::ConfigApplied
                | Self::RestartMarked
        )
    }
}

/// One event to append. `ts_us` is supplied by the store, not the caller:
/// it is observation, never a join key (DESIGN §12.10.4).
#[derive(Debug, Clone)]
pub struct NewEvent<'a> {
    pub kind: EventKind,
    /// `None` only for store-level events (`restart.marked`).
    pub request_id: Option<&'a str>,
    pub session: Option<&'a str>,
    /// First 16 hex of sha256(router-visible bytes); never the body.
    pub body_hash: Option<&'a str>,
    /// `"<trace file>:<line>"`, only where DESIGN §12.10.5 note R2 allows it.
    pub trace_ref: Option<&'a str>,
    /// The event's essentials as JSON.
    pub payload: Value,
}

impl<'a> NewEvent<'a> {
    /// The minimal store-level event (no request anchor).
    pub fn store_level(kind: EventKind, payload: Value) -> Self {
        Self {
            kind,
            request_id: None,
            session: None,
            body_hash: None,
            trace_ref: None,
            payload,
        }
    }
}

/// A stored event row, in `event_id` order.
#[derive(Debug, Clone)]
pub struct StoredEvent {
    pub event_id: EventId,
    pub ts_us: i64,
    /// The wire form of `kind`, always present.
    pub kind_raw: String,
    /// `None` when the row's kind is unknown to this binary — tolerated,
    /// never an error (rows written by a newer binary stay readable,
    /// ADR-009 item 7).
    pub kind: Option<EventKind>,
    pub request_id: Option<String>,
    pub session: Option<String>,
    pub schema_version: i64,
    pub payload: Value,
    pub body_hash: Option<String>,
    pub trace_ref: Option<String>,
}

/// Which projections a `rebuild` or `query` addresses. `All` exists for the
/// correctness oracle (CONF-21) and startup repair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Projection {
    Sessions,
    CacheLedger,
    QuotaCounters,
    ProviderCooldown,
    All,
}

/// One incremental projection write (the NORMAL tier: batchable, rebuildable).
/// `last_event` is the FULL event row the write rides on — the store reads
/// that row's own `ts_us` where a timestamp is needed, so the incremental
/// path and the rebuild path compute identical values (CONF-21) and no write
/// depends on a second clock read (AGENTS constraint 2).
#[derive(Debug, Clone)]
pub enum ProjectionWrite<'a> {
    /// Create or move a sticky binding and count one more request for the
    /// session (`requests_seen` is incremented by the binding write; a
    /// sticky hit writes nothing — DESIGN §12.10.5).
    SessionBound {
        session_key: &'a str,
        provider: &'a str,
        model: &'a str,
        /// TTL in microseconds; `expires_at_us` = the anchor row's `ts_us` + this.
        ttl_us: i64,
        last_event: EventId,
    },
    /// Replace the session's prefix block set (spec §6: the last block set
    /// seen is what the *next* request's continuity is measured against).
    CacheLedgerPut {
        session_key: &'a str,
        /// Block index → (kind, tokens, hash). An empty list records
        /// "no blocks" and clears the session's rows.
        blocks: &'a [(u32, &'a str, u64, &'a str)],
        last_event: EventId,
    },
    /// Add charged tokens to a plan window.
    QuotaCharged {
        provider: &'a str,
        plan_idx: u32,
        /// Window start in microseconds; the caller computes it from the
        /// plan's `window`/`reset_day` rule (pure function of the event's
        /// own ts, router-core's quota module).
        window_start_us: i64,
        tokens: i64,
        last_event: EventId,
    },
    /// Upsert a provider/route cooldown (ADR-011 demotion).
    ProviderCooldown {
        scope: &'a str,
        provider: &'a str,
        /// Empty = provider-wide.
        model: &'a str,
        until_us: i64,
        reason: &'a str,
        last_event: EventId,
    },
}

/// The serving path's reads (DESIGN §12.10.4 `query`).
#[derive(Debug, Clone)]
pub enum Query<'a> {
    /// The sticky binding for a session, if present and unexpired.
    SessionBinding { session_key: &'a str },
    /// `requests_seen` for a session (0 = no session yet); `turn_index` is
    /// this value + 1 (DESIGN §12.10.5).
    SessionRequestsSeen { session_key: &'a str },
    /// A plan window's charged tokens (0 when the window has no row).
    QuotaUsed {
        provider: &'a str,
        plan_idx: u32,
        window_start_us: i64,
    },
    /// The active cooldown for a provider (provider-wide if `model` is None,
    /// else the union of the provider-wide and route-scoped rows).
    Cooldown {
        provider: &'a str,
        model: Option<&'a str>,
    },
    /// The session's current prefix-block set (the `cache_ledger`
    /// projection, block order) — what the *next* request's continuity
    /// is measured against (spec §6, §12.10.6).
    CacheLedgerBlocks { session_key: &'a str },
    /// The full event log in `event_id` order (bounded use: conformance and
    /// rebuild; the serving path never scans the log).
    AllEvents,
}

#[derive(Debug, Clone)]
pub struct SessionBindingRow {
    pub session_key: String,
    pub provider: String,
    pub model: String,
    pub requests_seen: i64,
    pub expires_at_us: i64,
}

#[derive(Debug, Clone)]
pub struct CooldownRow {
    pub scope: String,
    pub provider: String,
    pub model: String,
    pub until_us: i64,
    pub reason: String,
}

/// One `cache_ledger` row, in block order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerBlock {
    pub index: u32,
    pub kind: String,
    pub tokens: u64,
    pub hash: String,
}

/// The answer to a `Query`.
#[derive(Debug, Clone)]
pub enum QueryRow {
    SessionBinding(Option<SessionBindingRow>),
    Count(i64),
    Cooldown(Option<CooldownRow>),
    CacheLedger(Vec<LedgerBlock>),
    Events(Vec<StoredEvent>),
}

/// Row counts a `rebuild` touched, per projection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RebuildStats {
    pub sessions: usize,
    pub cache_ledger: usize,
    pub quota_counters: usize,
    pub provider_cooldown: usize,
    pub events_scanned: usize,
}

/// The failure modes (DESIGN §12.10.4). The behaviours behind each variant
/// are ADR-009 item 8's; the variant is what a caller matches on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// Another process holds this state directory (the writer lock).
    Locked,
    /// Missing directory, permissions, corruption.
    Unopenable(String),
    /// The file's DDL version exceeds this binary's maximum (forward-only).
    SchemaTooNew {
        found: u32,
        supported: u32,
    },
    /// SQLITE_BUSY past the busy timeout.
    Busy,
    Sql(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Locked => write!(
                f,
                "state store is locked by another router process (writer lock)"
            ),
            Self::Unopenable(r) => write!(f, "state store cannot be opened: {r}"),
            Self::SchemaTooNew { found, supported } => write!(
                f,
                "state store schema version {found} is newer than this binary supports ({supported}); \
                 upgrade router to read it"
            ),
            Self::Busy => write!(f, "state store is busy past the timeout"),
            Self::Sql(e) => write!(f, "state store sql error: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// The persistence seam under the state service's traits (ADR-009 item 1).
/// One writer; reads take the same mutex — microseconds at this scale
/// (DESIGN §12.10.4).
pub trait Store: Send + Sync {
    /// Append one event, durably: intent/accounting kinds commit
    /// `synchronous=FULL` in their own transaction; derived kinds commit
    /// `NORMAL`. Returns the row's `event_id` (the ordering anchor).
    fn append(&self, ev: NewEvent<'_>) -> Result<EventId, StoreError>;

    /// One incremental projection write (the NORMAL tier: batchable,
    /// rebuildable; a lost write regresses statistics, never correctness).
    fn project(&self, w: ProjectionWrite<'_>) -> Result<(), StoreError>;

    /// The serving path's reads.
    fn query(&self, q: Query<'_>) -> Result<QueryRow, StoreError>;

    /// Drop and rebuild one projection (or all) from `events` by a single
    /// scan. The state layer's correctness oracle: the incremental path
    /// must converge with it row for row (CONF-21).
    fn rebuild(&self, which: Projection) -> Result<RebuildStats, StoreError>;

    /// The store's DDL version.
    fn schema_version(&self) -> Result<u32, StoreError>;
}

/// **Write ahead, then execute** (ADR-010 item 3, ADR-009 item 8).
///
/// Commits the intent event first; only if that commit succeeded is `effect`
/// run. If the intent write fails, `effect` is **never called** — the caller
/// must reject the request before anything reaches the upstream, with
/// `500 internal` / `details.stage = "intent"` (CONF-22). The two errors are
/// deliberately not symmetric: an unrecorded upstream call is an
/// unaccountable charge, a missing effect is a safe client retry.
pub fn write_intent_then<T, F>(
    store: &dyn Store,
    intent: NewEvent<'_>,
    effect: F,
) -> Result<(EventId, T), StoreError>
where
    F: FnOnce(EventId) -> T,
{
    let event_id = store.append(intent)?;
    Ok((event_id, effect(event_id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    /// Minimal in-memory double: records the call order so the
    /// write-ordering rule is testable without SQLite (router-core has no
    /// I/O; the real implementation's behaviour is asserted in
    /// `router-store`'s own tests and CONF-20/21/22).
    struct MemStore {
        events: Mutex<Vec<(EventKind, Option<String>)>>,
        next_id: Mutex<i64>,
        fail_append: std::sync::atomic::AtomicBool,
    }

    impl MemStore {
        fn new() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
                next_id: Mutex::new(0),
                fail_append: std::sync::atomic::AtomicBool::new(false),
            }
        }
    }

    impl Store for MemStore {
        fn append(&self, ev: NewEvent<'_>) -> Result<EventId, StoreError> {
            if self.fail_append.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(StoreError::Sql("injected failure".into()));
            }
            let id = {
                let mut n = self.next_id.lock().unwrap();
                *n += 1;
                *n
            };
            self.events
                .lock()
                .unwrap()
                .push((ev.kind, ev.request_id.map(|s| s.to_string())));
            Ok(EventId(id))
        }
        fn project(&self, _: ProjectionWrite<'_>) -> Result<(), StoreError> {
            Ok(())
        }
        fn query(&self, _: Query<'_>) -> Result<QueryRow, StoreError> {
            Ok(QueryRow::Count(0))
        }
        fn rebuild(&self, _: Projection) -> Result<RebuildStats, StoreError> {
            Ok(RebuildStats::default())
        }
        fn schema_version(&self) -> Result<u32, StoreError> {
            Ok(1)
        }
    }

    fn intent() -> NewEvent<'static> {
        NewEvent {
            kind: EventKind::UpstreamSubmitted,
            request_id: Some("req-1"),
            session: Some("sess-1"),
            body_hash: Some("0123456789abcdef"),
            trace_ref: None,
            payload: json!({"route": "zai/glm-5.3", "attempt_index": 0}),
        }
    }

    #[test]
    fn intent_commit_precedes_effect() {
        let store = MemStore::new();
        let mut order: Vec<&'static str> = Vec::new();
        let (id, ()) = write_intent_then(&store, intent(), |_| order.push("effect")).unwrap();
        assert_eq!(id, EventId(1));
        assert_eq!(order, vec!["effect"]);
        assert_eq!(store.events.lock().unwrap().len(), 1);
    }

    #[test]
    fn intent_write_failure_executes_nothing() {
        let store = MemStore::new();
        store
            .fail_append
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let mut effect_ran = false;
        let err = write_intent_then(&store, intent(), |_| effect_ran = true).unwrap_err();
        assert!(!effect_ran, "the effect must not run after a failed intent");
        assert!(matches!(err, StoreError::Sql(_)));
        assert!(store.events.lock().unwrap().is_empty(), "no side effects");
    }

    #[test]
    fn event_kind_roundtrip_and_tiers() {
        let all = [
            EventKind::RequestReceived,
            EventKind::DecisionMade,
            EventKind::TransformApplied,
            EventKind::SessionBound,
            EventKind::UpstreamSubmitted,
            EventKind::UpstreamResponded,
            EventKind::ErrorClassified,
            EventKind::FailoverTriggered,
            EventKind::CostComputed,
            EventKind::QuotaCharged,
            EventKind::PluginLoaded,
            EventKind::PluginUnloaded,
            EventKind::ConfigApplied,
            EventKind::RestartMarked,
        ];
        for k in all {
            assert_eq!(EventKind::from_str_lossy(k.as_str()), Some(k));
        }
        // ADR-009 item 4's tier list, exactly.
        let full: Vec<&str> = all
            .iter()
            .filter(|k| k.is_full())
            .map(|k| k.as_str())
            .collect();
        assert_eq!(
            full,
            vec![
                "request.received",
                "session.bound",
                "upstream.submitted",
                "upstream.responded",
                "failover.triggered",
                "cost.computed",
                "quota.charged",
                "config.applied",
                "restart.marked",
            ]
        );
        // Unknown kinds stay readable-but-unknown (ADR-009 item 7).
        assert_eq!(EventKind::from_str_lossy("future.thing"), None);
    }
}
