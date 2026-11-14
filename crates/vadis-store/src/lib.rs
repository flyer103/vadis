//! The SQLite/WAL implementation of `trait Store` (ADR-009, DESIGN §12.10.4).
//!
//! - `events` is the state truth (ADR-010); `sessions` / `cache_ledger` /
//!   `quota_counters` / `provider_cooldown` are projections that this crate
//!   can drop and rebuild from the log in a single scan.
//! - Durability is tiered by event class (ADR-009 item 4): intent/accounting
//!   kinds commit `synchronous=FULL`, one transaction per event, before the
//!   effect they authorize; derived kinds and projection writes are
//!   `synchronous=NORMAL` and batchable.
//! - One connection, one writer. The connection lives behind a `Mutex`
//!   (`rusqlite::Connection` is not `Sync`); a single connection is also what
//!   makes the writer lock workable.
//! - The writer lock is the database's own: `locking_mode=EXCLUSIVE` after
//!   the pragmas, then the migration write acquires and keeps the file locks,
//!   so a second process on the same state directory fails with
//!   `StoreError::Locked` (CONF-23b).

#![forbid(unsafe_code)]

pub mod trace_sink;

pub use trace_sink::{TraceRef, TraceSink};

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OpenFlags};
use vadis_core::store::{
    CooldownRow, EventId, EventKind, LedgerBlock, NewEvent, PlanStateProjRow, Projection,
    ProjectionWrite, Query, QueryRow, RebuildStats, SessionBindingRow, StreakRow, Store,
    StoreError, StoredEvent, EVENT_SCHEMA_VERSION,
};

/// Store DDL version 1: the tables of DESIGN §12.10.4, verbatim in shape.
/// Forward-only: appending a migration means adding a `(version, sql)` row
/// here; a DDL migration never rewrites `events` rows (ADR-009 item 7).
const MIGRATIONS: &[(u32, &str)] = &[
    (
        1,
        r#"
CREATE TABLE schema_version (
    version    INTEGER NOT NULL,
    applied_at TEXT    NOT NULL
);

CREATE TABLE events (
    event_id       INTEGER PRIMARY KEY AUTOINCREMENT,
    ts_us          INTEGER NOT NULL,
    kind           TEXT    NOT NULL,
    request_id     TEXT,
    session        TEXT,
    schema_version INTEGER NOT NULL,
    payload        TEXT    NOT NULL,
    body_hash      TEXT,
    trace_ref      TEXT
);
CREATE INDEX idx_events_request ON events(request_id, event_id);
CREATE INDEX idx_events_kind_ts ON events(kind, ts_us);
CREATE INDEX idx_events_session ON events(session, event_id);

CREATE TABLE sessions (
    session_key   TEXT PRIMARY KEY,
    provider      TEXT    NOT NULL,
    model         TEXT    NOT NULL,
    requests_seen INTEGER NOT NULL DEFAULT 0,
    expires_at_us INTEGER NOT NULL,
    last_event    INTEGER NOT NULL
);
CREATE TABLE cache_ledger (
    session_key TEXT    NOT NULL,
    block_index INTEGER NOT NULL,
    kind        TEXT    NOT NULL,
    tokens      INTEGER NOT NULL,
    hash        TEXT    NOT NULL,
    last_event  INTEGER NOT NULL,
    PRIMARY KEY (session_key, block_index)
);
CREATE TABLE quota_counters (
    provider        TEXT    NOT NULL,
    plan_idx        INTEGER NOT NULL,
    window_start_us INTEGER NOT NULL,
    tokens_used     INTEGER NOT NULL,
    last_event      INTEGER NOT NULL,
    PRIMARY KEY (provider, plan_idx, window_start_us)
);
CREATE TABLE provider_cooldown (
    scope      TEXT    NOT NULL,
    provider   TEXT    NOT NULL,
    model      TEXT    NOT NULL DEFAULT '',
    until_us   INTEGER NOT NULL,
    reason     TEXT    NOT NULL,
    last_event INTEGER NOT NULL,
    PRIMARY KEY (scope, provider, model)
);
"#,
    ),
    (
        // DDL version 2 (ADR-014, DESIGN §12.10.8): the family's account
        // state. Additive — one table, no existing row rewritten.
        2,
        r#"
CREATE TABLE plan_state (
    family     TEXT    PRIMARY KEY,
    account    TEXT    NOT NULL,
    since_us   INTEGER NOT NULL,
    until_us   INTEGER,
    last_event INTEGER NOT NULL
);
"#,
    ),
    (
        // DDL version 3 (ADR-049 §5.2): the state generalizes from the
        // two-valued word to **the active route**. Additive — one
        // nullable column backfilled from the event log's own
        // `to_route` (the same value the incremental path writes), no
        // existing row rewritten (ADR-009 item 7).
        3,
        r#"
ALTER TABLE plan_state ADD COLUMN route TEXT;
"#,
    ),
    (
        // DDL version 4 (ADR-064 §2.3/§2.4.3, DESIGN §12.10.4): the
        // route-scoped silence streak — one new table, one run row per
        // (provider, model) — and the plan state's cause word, one
        // nullable column whose NULL is a DDL-v3 row and reads
        // `primary_exhausted` (the only cause that existed before the
        // ADR; the reader maps it, §6.2 item 2). Additive: no existing
        // row is rewritten (ADR-009 item 7).
        4,
        r#"
CREATE TABLE silence_streak (
    provider   TEXT    NOT NULL,
    model      TEXT    NOT NULL,
    count      INTEGER NOT NULL,
    first_us   INTEGER NOT NULL,
    last_us    INTEGER NOT NULL,
    last_event INTEGER NOT NULL,
    PRIMARY KEY (provider, model)
);
ALTER TABLE plan_state ADD COLUMN cause TEXT;
"#,
    ),
];

/// The SQLite implementation. Construct through [`SqliteStore::open`].
#[derive(Debug)]
pub struct SqliteStore {
    conn: Mutex<Connection>,
}

/// Map a rusqlite error onto the failure-mode variants (DESIGN §12.10.4).
/// During `open`, "database is locked" means another writer holds the state
/// directory — the distinguishable `Locked` reason (CONF-23b); afterwards it
/// means the busy timeout lapsed (`Busy`).
fn map_err(open_phase: bool, e: rusqlite::Error) -> StoreError {
    let locked = matches!(
        &e,
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ffi::ErrorCode::DatabaseBusy,
                ..
            },
            _,
        ) | rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ffi::ErrorCode::DatabaseLocked,
                ..
            },
            _,
        )
    ) || e.to_string().contains("database is locked");
    if locked && open_phase {
        StoreError::Locked
    } else if locked {
        StoreError::Busy
    } else {
        StoreError::Sql(e.to_string())
    }
}

fn now_us() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0)
}

/// The W a config-less `rebuild` folds the silence streak with: the
/// ADR-064 §2.7.1 frozen default (5 m), the only value the store can
/// name without reading config. The serving path never calls `rebuild`
/// (verified at this round's HEAD: only conformance and unit tests do),
/// and a deployment whose families write a different W rebuilds through
/// [`SqliteStore::rebuild_with_silence_window`], which takes it
/// explicitly. ADR-064 §2.3.3's fold needs a window; this constant is
/// the honest default, never a second source of the contract's value.
const REBUILD_DEFAULT_SILENCE_WINDOW_US: i64 = 300_000_000;

/// ADR-064 §2.2.1's counted-fact predicate, in the **event's** spelling
/// (`record_classified`'s shape): exactly two shapes count — a stream
/// that went silent mid-body (`reason: stream_truncated` ∧
/// `stream_end: idle_timeout`) and a head that never arrived
/// (`reason: timeout` ∧ `stage: unknown_outcome`), both demanding
/// `stream == true` (a buffered failure is never a silence fact) and
/// both carrying the `route` member §2.2.2 added. One predicate, one
/// home, read by the rebuild — the live path classifies before it
/// writes, so the two spellings cannot disagree about one attempt.
fn counted_silence_fact(payload: &serde_json::Value) -> Option<(&str, &str)> {
    let stream = payload.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);
    if !stream {
        return None;
    }
    let reason = payload.get("reason").and_then(|v| v.as_str()).unwrap_or("");
    let s1 = reason == "stream_truncated"
        && payload.get("stream_end").and_then(|v| v.as_str()) == Some("idle_timeout");
    let s2 = reason == "timeout"
        && payload.get("stage").and_then(|v| v.as_str()) == Some("unknown_outcome");
    if !s1 && !s2 {
        return None;
    }
    let route = payload.get("route")?;
    let provider = route.get("provider").and_then(|v| v.as_str())?;
    let model = route.get("model").and_then(|v| v.as_str())?;
    // '' is never a legal native model id — a malformed member counts
    // nothing rather than keying a run on the empty route.
    if provider.is_empty() || model.is_empty() {
        return None;
    }
    Some((provider, model))
}

/// unix days → (year, month, day), Howard Hinnant's `civil_from_days`.
/// Only used for `schema_version.applied_at` (an observation column, never a
/// join key), so the hand-rolled calendar math is checked against fixed
/// anchors in the tests below.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (y + i64::from(m <= 2), m, d)
}

fn rfc3339_utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

impl SqliteStore {
    /// Open (creating if absent) and migrate the database at `path`, then
    /// take the writer lock. Any failure is terminal for `serve` (ADR-009
    /// item 8): there is no in-memory fallback.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                StoreError::Unopenable(format!("cannot create {}: {e}", parent.display()))
            })?;
        }
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )
        .map_err(|e| StoreError::Unopenable(format!("{}: {e}", path.display())))?;

        // A short busy timeout: long enough to ride out a checkpoint, short
        // enough that a second `serve` fails promptly with `Locked`.
        let busy_ms = 1_000;
        conn.busy_timeout(std::time::Duration::from_millis(busy_ms))
            .map_err(|e| map_err(true, e))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;\
             PRAGMA foreign_keys = ON;",
        )
        .map_err(|e| map_err(true, e))?;

        // The writer lock (ADR-009 item 8 / DESIGN §12.10.4): EXCLUSIVE
        // locking mode never releases the file locks after the next write,
        // so a second process on this state directory cannot interleave.
        // Honest cost (documented in book/operations.md): while `serve`
        // runs, no other process can read the file either.
        conn.execute_batch("PRAGMA locking_mode = EXCLUSIVE;")
            .map_err(|e| map_err(true, e))?;

        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    /// Open an existing database **read-only** (spec §9.2: `vadis stats`
    /// never writes, never migrates, and does not take the writer role
    /// `serve` holds — a read-only connection is not the exclusive writer,
    /// §4.5). No file is created: an absent, unreadable or too-new database
    /// is an error the caller turns into an omitted figure, never a
    /// mutation.
    pub fn open_read_only(path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| StoreError::Unopenable(format!("{}: {e}", path.display())))?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        // Refuse a schema this binary cannot know (spec §9.2: an unreadable
        // store omits the one log-held figure, it never guesses).
        let supported = MIGRATIONS.last().map(|m| m.0).unwrap_or(0);
        let current = store.schema_version()?;
        if current > supported {
            return Err(StoreError::SchemaTooNew {
                found: current,
                supported,
            });
        }
        Ok(store)
    }

    /// Apply forward-only migrations in order, one transaction each
    /// (the first write also satisfies the EXCLUSIVE-lock acquisition).
    fn migrate(&self) -> Result<(), StoreError> {
        let mut conn = self.conn.lock().expect("store mutex");
        let current: u32 = if conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_version'",
                [],
                |_| Ok(()),
            )
            .is_ok()
        {
            conn.query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|v| v as u32)
            .map_err(|e| map_err(false, e))?
        } else {
            0
        };
        let supported = MIGRATIONS.last().map(|m| m.0).unwrap_or(0);
        if current > supported {
            return Err(StoreError::SchemaTooNew {
                found: current,
                supported,
            });
        }
        for (version, sql) in MIGRATIONS {
            if *version <= current {
                continue;
            }
            let tx = conn.transaction().map_err(|e| map_err(false, e))?;
            tx.execute_batch(sql).map_err(|e| map_err(false, e))?;
            tx.execute(
                "INSERT INTO schema_version (version, applied_at) VALUES (?1, ?2)",
                params![*version as i64, rfc3339_utc(now_us() / 1_000_000)],
            )
            .map_err(|e| map_err(false, e))?;
            tx.commit().map_err(|e| map_err(false, e))?;
        }
        Ok(())
    }

    /// Canonical `col|col` lines per projection row, sorted — the stable
    /// comparison surface for CONF-21 ("incremental == rebuild") and later
    /// for a `vadis state` surface (a separate change, ADR-010).
    pub fn projection_rows(&self, which: Projection) -> Result<Vec<String>, StoreError> {
        let conn = self.conn.lock().expect("store mutex");
        let mut out = Vec::new();
        let mut push = |sql: &str| -> Result<(), StoreError> {
            let mut stmt = conn.prepare(sql).map_err(|e| map_err(false, e))?;
            let mut rows = stmt.query([]).map_err(|e| map_err(false, e))?;
            while let Some(row) = rows.next().map_err(|e| map_err(false, e))? {
                let n = row.as_ref().column_count();
                let cols: Vec<String> = (0..n)
                    .map(|i| {
                        let v = row.get_ref(i).map_err(|e| map_err(false, e))?;
                        Ok(match v {
                            rusqlite::types::ValueRef::Null => "NULL".into(),
                            rusqlite::types::ValueRef::Integer(i) => i.to_string(),
                            rusqlite::types::ValueRef::Text(t) => {
                                String::from_utf8_lossy(t).into_owned()
                            }
                            rusqlite::types::ValueRef::Real(f) => f.to_string(),
                            rusqlite::types::ValueRef::Blob(b) => {
                                format!("blob:{}", b.len())
                            }
                        })
                    })
                    .collect::<Result<_, StoreError>>()?;
                out.push(cols.join("|"));
            }
            Ok(())
        };
        let want = |p: Projection| which == p || which == Projection::All;
        if want(Projection::Sessions) {
            push(
                "SELECT session_key, provider, model, requests_seen, expires_at_us, last_event FROM sessions ORDER BY session_key",
            )?;
        }
        if want(Projection::CacheLedger) {
            push(
                "SELECT session_key, block_index, kind, tokens, hash, last_event \
                  FROM cache_ledger ORDER BY session_key, block_index",
            )?;
        }
        if want(Projection::QuotaCounters) {
            push(
                "SELECT provider, plan_idx, window_start_us, tokens_used, last_event \
                  FROM quota_counters ORDER BY provider, plan_idx, window_start_us",
            )?;
        }
        if want(Projection::ProviderCooldown) {
            push(
                "SELECT scope, provider, model, until_us, reason, last_event \
                  FROM provider_cooldown ORDER BY scope, provider, model",
            )?;
        }
        if want(Projection::PlanState) {
            push(
                "SELECT family, account, route, since_us, until_us, last_event, cause \
                  FROM plan_state ORDER BY family",
            )?;
        }
        if want(Projection::SilenceStreak) {
            push(
                "SELECT provider, model, count, first_us, last_us, last_event \
                  FROM silence_streak ORDER BY provider, model",
            )?;
        }
        out.sort();
        Ok(out)
    }

    /// `Store::rebuild`'s body, with the silence window stated
    /// explicitly (ADR-064 §2.3.3: the streak's fold needs a W, and the
    /// store never reads config — `rebuild` itself folds with the
    /// contract's frozen default, which is wrong for a deployment that
    /// writes a different `silent_escalation.window`; this method is the
    /// faithful form for that case and the §6.2 identity test).
    pub fn rebuild_with_silence_window(
        &self,
        which: Projection,
        silence_window_us: i64,
    ) -> Result<RebuildStats, StoreError> {
        let conn = self.conn.lock().expect("store mutex");
        conn.execute_batch("PRAGMA synchronous = NORMAL;")
            .map_err(|e| map_err(false, e))?;
        let events = self.read_events(&conn)?;
        let want = |p: Projection| which == p || which == Projection::All;
        let mut stats = RebuildStats {
            events_scanned: events.len(),
            ..RebuildStats::default()
        };
        if want(Projection::Sessions) {
            stats.sessions = self.rebuild_sessions(&conn, &events)?;
        }
        if want(Projection::CacheLedger) {
            stats.cache_ledger = self.rebuild_cache_ledger(&conn, &events)?;
        }
        if want(Projection::QuotaCounters) {
            stats.quota_counters = self.rebuild_quota(&conn, &events)?;
        }
        if want(Projection::ProviderCooldown) {
            stats.provider_cooldown = self.rebuild_cooldown(&conn, &events)?;
        }
        if want(Projection::PlanState) {
            stats.plan_state = self.rebuild_plan_state(&conn, &events)?;
        }
        if want(Projection::SilenceStreak) {
            stats.silence_streak =
                self.rebuild_silence_streak(&conn, &events, silence_window_us)?;
        }
        Ok(stats)
    }

    /// Escape hatch for tests and the future `vadis state` surface (a
    /// separate change, ADR-010): the one connection behind the mutex.
    /// Nothing in the serving path uses this.
    pub fn raw_connection(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().expect("store mutex")
    }

    fn read_events(&self, conn: &Connection) -> Result<Vec<StoredEvent>, StoreError> {
        let mut stmt = conn
            .prepare(
                "SELECT event_id, ts_us, kind, request_id, session, schema_version, payload, body_hash, trace_ref FROM events ORDER BY event_id",
            )
            .map_err(|e| map_err(false, e))?;
        let mut rows = stmt.query([]).map_err(|e| map_err(false, e))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(|e| map_err(false, e))? {
            let kind_raw: String = row.get(2).map_err(|e| map_err(false, e))?;
            let payload: String = row.get(6).map_err(|e| map_err(false, e))?;
            out.push(StoredEvent {
                event_id: EventId(row.get(0).map_err(|e| map_err(false, e))?),
                ts_us: row.get(1).map_err(|e| map_err(false, e))?,
                kind: EventKind::from_str_lossy(&kind_raw), // None = unknown, tolerated
                kind_raw,
                request_id: row.get(3).map_err(|e| map_err(false, e))?,
                session: row.get(4).map_err(|e| map_err(false, e))?,
                schema_version: row.get(5).map_err(|e| map_err(false, e))?,
                payload: serde_json::from_str(&payload)
                    .map_err(|e| StoreError::Sql(format!("event payload is not JSON: {e}")))?,
                body_hash: row.get(7).map_err(|e| map_err(false, e))?,
                trace_ref: row.get(8).map_err(|e| map_err(false, e))?,
            });
        }
        Ok(out)
    }

    fn rebuild_sessions(
        &self,
        conn: &Connection,
        events: &[StoredEvent],
    ) -> Result<usize, StoreError> {
        conn.execute("DELETE FROM sessions", [])
            .map_err(|e| map_err(false, e))?;
        // Rule (DESIGN §12.10.5): the binding write increments requests_seen;
        // a sticky hit writes nothing. So requests_seen == the session's
        // session.bound count, and the binding columns come from the latest
        // session.bound row. expires_at_us = that row's ts_us + payload ttl.
        let mut count = 0usize;
        for ev in events {
            if ev.kind_raw != EventKind::SessionBound.as_str() {
                continue;
            }
            let key = match ev.session.as_deref() {
                Some(k) => k,
                None => continue,
            };
            let provider = ev
                .payload
                .get("provider")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let model = ev
                .payload
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let ttl_us = ev
                .payload
                .get("ttl_us")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            conn.execute(
                "INSERT INTO sessions (session_key, provider, model, requests_seen, expires_at_us, last_event) VALUES (?1, ?2, ?3, 1, ?4, ?5) ON CONFLICT(session_key) DO UPDATE SET provider = excluded.provider, model = excluded.model, requests_seen = sessions.requests_seen + 1, expires_at_us = excluded.expires_at_us, last_event = excluded.last_event",
                params![key, provider, model, ev.ts_us + ttl_us, ev.event_id.0],
            )
            .map_err(|e| map_err(false, e))?;
            count += 1;
        }
        Ok(count)
    }

    fn rebuild_cache_ledger(
        &self,
        conn: &Connection,
        events: &[StoredEvent],
    ) -> Result<usize, StoreError> {
        conn.execute("DELETE FROM cache_ledger", [])
            .map_err(|e| map_err(false, e))?;
        // Rule: each upstream.submitted whose payload carries `prefix_blocks`
        // replaces that session's block set (the encoder computed the blocks
        // from exactly those bytes, DESIGN §12.10.6). The latest such row per
        // session wins — replaying in event_id order gives that for free.
        let mut count = 0usize;
        for ev in events {
            if ev.kind_raw != EventKind::UpstreamSubmitted.as_str() {
                continue;
            }
            let key = match ev.session.as_deref() {
                Some(k) => k,
                None => continue,
            };
            let blocks = match ev.payload.get("prefix_blocks").and_then(|v| v.as_array()) {
                Some(b) => b,
                None => continue,
            };
            conn.execute(
                "DELETE FROM cache_ledger WHERE session_key = ?1",
                params![key],
            )
            .map_err(|e| map_err(false, e))?;
            for b in blocks {
                conn.execute(
                    "INSERT INTO cache_ledger (session_key, block_index, kind, tokens, hash, last_event) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        key,
                        b.get("index").and_then(|v| v.as_i64()).unwrap_or(0),
                        b.get("kind").and_then(|v| v.as_str()).unwrap_or(""),
                        b.get("tokens").and_then(|v| v.as_i64()).unwrap_or(0),
                        b.get("hash").and_then(|v| v.as_str()).unwrap_or(""),
                        ev.event_id.0,
                    ],
                )
                .map_err(|e| map_err(false, e))?;
                count += 1;
            }
        }
        Ok(count)
    }

    fn rebuild_quota(
        &self,
        conn: &Connection,
        events: &[StoredEvent],
    ) -> Result<usize, StoreError> {
        conn.execute("DELETE FROM quota_counters", [])
            .map_err(|e| map_err(false, e))?;
        let mut count = 0usize;
        for ev in events {
            if ev.kind_raw != EventKind::QuotaCharged.as_str() {
                continue;
            }
            let provider = ev
                .payload
                .get("provider")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let plan_idx = ev
                .payload
                .get("plan_idx")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let window = ev
                .payload
                .get("window_start_us")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let tokens = ev
                .payload
                .get("tokens")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            conn.execute(
                "INSERT INTO quota_counters (provider, plan_idx, window_start_us, tokens_used, last_event) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(provider, plan_idx, window_start_us) DO UPDATE SET tokens_used = tokens_used + excluded.tokens_used, last_event = excluded.last_event",
                params![provider, plan_idx, window, tokens, ev.event_id.0],
            )
            .map_err(|e| map_err(false, e))?;
            count += 1;
        }
        Ok(count)
    }

    fn rebuild_cooldown(
        &self,
        conn: &Connection,
        events: &[StoredEvent],
    ) -> Result<usize, StoreError> {
        conn.execute("DELETE FROM provider_cooldown", [])
            .map_err(|e| map_err(false, e))?;
        // Rule: an error.classified / failover.triggered payload may carry a
        // `demotion` object (ADR-011 item 8); the latest per (scope,
        // provider, model) wins.
        let mut count = 0usize;
        for ev in events {
            let Some(d) = ev.payload.get("demotion") else {
                continue;
            };
            let scope = d
                .get("scope")
                .and_then(|v| v.as_str())
                .unwrap_or("provider");
            let provider = d.get("provider").and_then(|v| v.as_str()).unwrap_or("");
            let model = d.get("model").and_then(|v| v.as_str()).unwrap_or("");
            let until_us = d.get("until_us").and_then(|v| v.as_i64()).unwrap_or(0);
            let reason = d.get("reason").and_then(|v| v.as_str()).unwrap_or("");
            conn.execute(
                "INSERT INTO provider_cooldown (scope, provider, model, until_us, reason, last_event) VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(scope, provider, model) DO UPDATE SET until_us = excluded.until_us, reason = excluded.reason, last_event = excluded.last_event",
                params![scope, provider, model, until_us, reason, ev.event_id.0],
            )
            .map_err(|e| map_err(false, e))?;
            count += 1;
        }
        Ok(count)
    }

    /// DESIGN §12.10.8's rebuild rule: scan `plan.switched` in `event_id`
    /// order, per family — the last row's `to_route` is the active route,
    /// its `to_account` the derived wire word, its `ts_us` `since_us`, its
    /// `event_id` `last_event`, and `until_us = since_us + cooldown_ms ×
    /// 1000` while the account is `overflow` (the payload carries the
    /// then-current cooldown, so the log alone determines the row; the
    /// serving path recomputes the probe gate from the *current* config
    /// against `since_us`). The route comes from the event's own
    /// `to_route` payload member, so the incremental path and the rebuild
    /// write the identical value from the identical source (CONF-21).
    fn rebuild_plan_state(
        &self,
        conn: &Connection,
        events: &[StoredEvent],
    ) -> Result<usize, StoreError> {
        conn.execute("DELETE FROM plan_state", [])
            .map_err(|e| map_err(false, e))?;
        let mut count = 0usize;
        for ev in events {
            if ev.kind_raw != EventKind::PlanSwitched.as_str() {
                continue;
            }
            let family = ev
                .payload
                .get("family")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let account = ev
                .payload
                .get("to_account")
                .and_then(|v| v.as_str())
                .unwrap_or("primary");
            let route = ev
                .payload
                .get("to_route")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let cooldown_us = ev
                .payload
                .get("cooldown_ms")
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
                .saturating_mul(1_000);
            // ADR-064 §2.4.3 (`plan_state.cause`): the row's own `reason`
            // — the same word the incremental write passed, so an
            // incremental move and a rebuilt one carry the same cause
            // (CONF-21's rule, extended to the new column).
            let cause = ev
                .payload
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("primary_exhausted");
            let until_us = if account == "overflow" && cooldown_us > 0 {
                // Derived display value, not a source of truth: no verdict
                // path reads it — /health and the guard both recompute the
                // deadline from the *loaded* config (R10-F1).
                Some(ev.ts_us.saturating_add(cooldown_us))
            } else {
                None
            };
            conn.execute(
                "INSERT INTO plan_state (family, account, since_us, until_us, last_event, route, cause) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT(family) DO UPDATE SET account = excluded.account, since_us = excluded.since_us, until_us = excluded.until_us, last_event = excluded.last_event, route = excluded.route, cause = excluded.cause",
                params![family, account, ev.ts_us, until_us, ev.event_id.0, route, cause],
            )
            .map_err(|e| map_err(false, e))?;
            count += 1;
        }
        Ok(count)
    }

    /// ADR-064 §2.3.3: `rebuild_silence_streak` — the single-kind fold
    /// over the counted `error.classified` rows. Reads only each row's
    /// `ts_us`, the payload's `route` member and the §2.2.1 predicate's
    /// own keys: no config, no clock, no trace. The window is the
    /// caller's (the fold needs a W; `rebuild` itself has none and the
    /// store never reads config — see [`Self::rebuild_with_silence_window`]).
    /// §2.5.4's delete is mirrored generically: a counted fact that
    /// landed before the LAST `plan.switched` row whose `to_route` is
    /// this route (a return to the plan tier) is dropped, exactly what
    /// the incremental `SilenceStreakClear` removed.
    fn rebuild_silence_streak(
        &self,
        conn: &Connection,
        events: &[StoredEvent],
        window_us: i64,
    ) -> Result<usize, StoreError> {
        conn.execute("DELETE FROM silence_streak", [])
            .map_err(|e| map_err(false, e))?;
        // §2.5.4's mirror: the latest return-to-route move per route.
        let mut returned_at: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();
        for ev in events {
            if ev.kind_raw != EventKind::PlanSwitched.as_str() {
                continue;
            }
            if let Some(to) = ev.payload.get("to_route").and_then(|v| v.as_str()) {
                let e = returned_at.entry(to.to_string()).or_insert(ev.ts_us);
                if ev.ts_us > *e {
                    *e = ev.ts_us;
                }
            }
        }
        // The §2.1 fold, per (provider, model): (count, first_us, last_us,
        // last_event).
        let mut runs: std::collections::HashMap<(String, String), (i64, i64, i64, i64)> =
            std::collections::HashMap::new();
        for ev in events {
            if ev.kind_raw != EventKind::ErrorClassified.as_str() {
                continue;
            }
            let Some((provider, model)) = counted_silence_fact(&ev.payload) else {
                continue;
            };
            let label = format!("{provider}/{model}");
            if let Some(cleared) = returned_at.get(&label) {
                if ev.ts_us <= *cleared {
                    continue;
                }
            }
            let key = (provider.to_string(), model.to_string());
            match runs.get_mut(&key) {
                // §2.1: a fact joins the current run iff it lands within
                // W of the run's `first_us`; otherwise it starts a new
                // run at count 1 and becomes its anchor.
                Some(run) if ev.ts_us.saturating_sub(run.1) <= window_us => {
                    run.0 += 1;
                    run.2 = ev.ts_us;
                    run.3 = ev.event_id.0;
                }
                _ => {
                    runs.insert(key, (1, ev.ts_us, ev.ts_us, ev.event_id.0));
                }
            }
        }
        let mut count = 0usize;
        for ((provider, model), (n, first_us, last_us, last_event)) in runs {
            conn.execute(
                "INSERT INTO silence_streak (provider, model, count, first_us, last_us, last_event) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
                 ON CONFLICT(provider, model) DO UPDATE SET count = excluded.count, \
                 first_us = excluded.first_us, last_us = excluded.last_us, \
                 last_event = excluded.last_event",
                params![provider, model, n, first_us, last_us, last_event],
            )
            .map_err(|e| map_err(false, e))?;
            count += 1;
        }
        Ok(count)
    }
}

impl Store for SqliteStore {
    fn append(&self, ev: NewEvent<'_>) -> Result<EventId, StoreError> {
        let mut conn = self.conn.lock().expect("store mutex");
        // The tier is a property of the event class (ADR-009 item 4). FULL
        // kinds commit in their own transaction — never stacked, the intent
        // row must be durable before its effect (DESIGN §12.10.4).
        let tier = if ev.kind.is_full() { "FULL" } else { "NORMAL" };
        conn.execute_batch(&format!("PRAGMA synchronous = {tier};"))
            .map_err(|e| map_err(false, e))?;
        let tx = conn.transaction().map_err(|e| map_err(false, e))?;
        tx.execute(
            "INSERT INTO events (ts_us, kind, request_id, session, schema_version, payload, body_hash, trace_ref) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                now_us(),
                ev.kind.as_str(),
                ev.request_id,
                ev.session,
                EVENT_SCHEMA_VERSION,
                ev.payload.to_string(),
                ev.body_hash,
                ev.trace_ref,
            ],
        )
        .map_err(|e| map_err(false, e))?;
        let id = tx.last_insert_rowid();
        tx.commit().map_err(|e| map_err(false, e))?;
        Ok(EventId(id))
    }

    fn project(&self, w: ProjectionWrite<'_>) -> Result<(), StoreError> {
        let mut conn = self.conn.lock().expect("store mutex");
        // NORMAL + group commit (ADR-009 item 4): a lost projection write is
        // repaired by `rebuild`, so it never fsyncs per row.
        conn.execute_batch("PRAGMA synchronous = NORMAL;")
            .map_err(|e| map_err(false, e))?;
        let tx = conn.transaction().map_err(|e| map_err(false, e))?;
        match w {
            ProjectionWrite::SessionBound {
                session_key,
                provider,
                model,
                ttl_us,
                last_event,
            } => {
                // expires_at_us derives from the anchor event's own ts_us
                // (read by last_event), not a second clock read — the
                // incremental and rebuild paths then compute identical
                // values (CONF-21; AGENTS constraint 2).
                let anchor_ts: i64 = tx
                    .query_row(
                        "SELECT ts_us FROM events WHERE event_id = ?1",
                        params![last_event.0],
                        |r| r.get(0),
                    )
                    .map_err(|e| map_err(false, e))?;
                tx.execute(
                    "INSERT INTO sessions (session_key, provider, model, requests_seen, expires_at_us, last_event) VALUES (?1, ?2, ?3, 1, ?4, ?5) ON CONFLICT(session_key) DO UPDATE SET provider = excluded.provider, model = excluded.model, requests_seen = sessions.requests_seen + 1, expires_at_us = excluded.expires_at_us, last_event = excluded.last_event",
                    params![session_key, provider, model, anchor_ts + ttl_us, last_event.0],
                )
                .map_err(|e| map_err(false, e))?;
            }
            ProjectionWrite::CacheLedgerPut {
                session_key,
                blocks,
                last_event,
            } => {
                tx.execute(
                    "DELETE FROM cache_ledger WHERE session_key = ?1",
                    params![session_key],
                )
                .map_err(|e| map_err(false, e))?;
                for (index, kind, tokens, hash) in blocks {
                    tx.execute(
                        "INSERT INTO cache_ledger (session_key, block_index, kind, tokens, hash, last_event) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![session_key, *index as i64, *kind, *tokens as i64, *hash, last_event.0],
                    )
                    .map_err(|e| map_err(false, e))?;
                }
            }
            ProjectionWrite::QuotaCharged {
                provider,
                plan_idx,
                window_start_us,
                tokens,
                last_event,
            } => {
                tx.execute(
                    "INSERT INTO quota_counters (provider, plan_idx, window_start_us, tokens_used, last_event) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(provider, plan_idx, window_start_us) DO UPDATE SET tokens_used = tokens_used + excluded.tokens_used, last_event = excluded.last_event",
                    params![provider, plan_idx as i64, window_start_us, tokens, last_event.0],
                )
                .map_err(|e| map_err(false, e))?;
            }
            ProjectionWrite::ProviderCooldown {
                scope,
                provider,
                model,
                until_us,
                reason,
                last_event,
            } => {
                tx.execute(
                    "INSERT INTO provider_cooldown (scope, provider, model, until_us, reason, last_event) VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(scope, provider, model) DO UPDATE SET until_us = excluded.until_us, reason = excluded.reason, last_event = excluded.last_event",
                    params![scope, provider, model, until_us, reason, last_event.0],
                )
                .map_err(|e| map_err(false, e))?;
            }
            ProjectionWrite::PlanSwitched {
                family,
                account,
                to_route,
                cooldown_us,
                cause,
                last_event,
            } => {
                // since_us anchors on the event row's own ts_us (never a
                // second clock read), so the incremental path and the
                // rebuild compute identical values (CONF-21). The route
                // is the caller's active route — the same `to_route` the
                // rebuild reads from the event payload.
                let since_us: i64 = tx
                    .query_row(
                        "SELECT ts_us FROM events WHERE event_id = ?1",
                        params![last_event.0],
                        |r| r.get(0),
                    )
                    .map_err(|e| map_err(false, e))?;
                let until_us = if account == "overflow" && cooldown_us > 0 {
                    Some(since_us.saturating_add(cooldown_us))
                } else {
                    None
                };
                tx.execute(
                    "INSERT INTO plan_state (family, account, since_us, until_us, last_event, route, cause) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT(family) DO UPDATE SET account = excluded.account, since_us = excluded.since_us, until_us = excluded.until_us, last_event = excluded.last_event, route = excluded.route, cause = excluded.cause",
                    params![family, account, since_us, until_us, last_event.0, to_route, cause],
                )
                .map_err(|e| map_err(false, e))?;
            }
            ProjectionWrite::SilenceStreak {
                provider,
                model,
                window_us,
                last_event,
            } => {
                // ADR-064 §2.3.1's ONE upsert, store-owned so the fold
                // anchors on the classification row's own `ts_us`
                // (CONF-21's idiom): a fact within `window_us` of the
                // run's `first_us` joins it; a later fact starts a fresh
                // run at count 1. The incremental path and the §2.3.3
                // rebuild therefore compute identical rows from the
                // identical source — the caller never folds.
                let anchor_ts: i64 = tx
                    .query_row(
                        "SELECT ts_us FROM events WHERE event_id = ?1",
                        params![last_event.0],
                        |r| r.get(0),
                    )
                    .map_err(|e| map_err(false, e))?;
                let prev: Option<(i64, i64)> = tx
                    .query_row(
                        "SELECT count, first_us FROM silence_streak WHERE provider = ?1 AND model = ?2",
                        params![provider, model],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .map(Some)
                    .or_else(|e| match e {
                        rusqlite::Error::QueryReturnedNoRows => Ok(None),
                        other => Err(other),
                    })
                    .map_err(|e| map_err(false, e))?;
                let (count, first_us) = match prev {
                    Some((c, first_us)) if anchor_ts.saturating_sub(first_us) <= window_us => {
                        (c + 1, first_us)
                    }
                    _ => (1, anchor_ts),
                };
                tx.execute(
                    "INSERT INTO silence_streak (provider, model, count, first_us, last_us, last_event) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
                     ON CONFLICT(provider, model) DO UPDATE SET count = excluded.count, \
                     first_us = excluded.first_us, last_us = excluded.last_us, \
                     last_event = excluded.last_event",
                    params![provider, model, count, first_us, anchor_ts, last_event.0],
                )
                .map_err(|e| map_err(false, e))?;
            }
            ProjectionWrite::SilenceStreakClear { provider, model } => {
                // ADR-064 §2.5.4: the return trip's delete — a family
                // that comes back to its plan tier starts from zero.
                tx.execute(
                    "DELETE FROM silence_streak WHERE provider = ?1 AND model = ?2",
                    params![provider, model],
                )
                .map_err(|e| map_err(false, e))?;
            }
        }
        tx.commit().map_err(|e| map_err(false, e))?;
        Ok(())
    }

    fn query(&self, q: Query<'_>) -> Result<QueryRow, StoreError> {
        let conn = self.conn.lock().expect("store mutex");
        Ok(match q {
            Query::SessionBinding { session_key } => {
                let row = conn
                    .query_row(
                        "SELECT session_key, provider, model, requests_seen, expires_at_us \
                         FROM sessions WHERE session_key = ?1 AND expires_at_us > ?2",
                        params![session_key, now_us()],
                        |r| {
                            Ok(SessionBindingRow {
                                session_key: r.get(0)?,
                                provider: r.get(1)?,
                                model: r.get(2)?,
                                requests_seen: r.get(3)?,
                                expires_at_us: r.get(4)?,
                            })
                        },
                    )
                    .map_err(|e| match e {
                        rusqlite::Error::QueryReturnedNoRows => {
                            rusqlite::Error::QueryReturnedNoRows
                        }
                        other => other,
                    });
                QueryRow::SessionBinding(match row {
                    Ok(b) => Some(b),
                    Err(rusqlite::Error::QueryReturnedNoRows) => None,
                    Err(e) => return Err(map_err(false, e)),
                })
            }
            Query::SessionBindingsFor { provider, model } => {
                let mut stmt = conn
                    .prepare(
                        "SELECT session_key, provider, model, requests_seen, expires_at_us \
                         FROM sessions WHERE provider = ?1 AND model = ?2 AND expires_at_us > ?3",
                    )
                    .map_err(|e| map_err(false, e))?;
                let rows = stmt
                    .query_map(params![provider, model, now_us()], |r| {
                        Ok(SessionBindingRow {
                            session_key: r.get(0)?,
                            provider: r.get(1)?,
                            model: r.get(2)?,
                            requests_seen: r.get(3)?,
                            expires_at_us: r.get(4)?,
                        })
                    })
                    .map_err(|e| map_err(false, e))?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| map_err(false, e))?;
                QueryRow::SessionBindings(rows)
            }
            Query::SessionRequestsSeen { session_key } => {
                let n: i64 = conn
                    .query_row(
                        "SELECT requests_seen FROM sessions WHERE session_key = ?1",
                        params![session_key],
                        |r| r.get(0),
                    )
                    .map_err(|e| map_err(false, e))?;
                QueryRow::Count(n)
            }
            Query::QuotaUsed {
                provider,
                plan_idx,
                window_start_us,
            } => {
                let n: i64 = conn
                    .query_row(
                        "SELECT tokens_used FROM quota_counters \
                         WHERE provider = ?1 AND plan_idx = ?2 AND window_start_us = ?3",
                        params![provider, plan_idx as i64, window_start_us],
                        |r| r.get(0),
                    )
                    .map_err(|e| map_err(false, e))?;
                QueryRow::Count(n)
            }
            Query::Cooldown { provider, model } => {
                // A route-scoped cooldown is shadowed by a provider-wide one
                // that lasts longer; report whichever blocks the route now.
                let sql = match model {
                    Some(_) => {
                        "SELECT scope, provider, model, until_us, reason FROM provider_cooldown \
                         WHERE provider = ?1 AND (model = '' OR model = ?2) AND until_us > ?3 \
                         ORDER BY until_us DESC LIMIT 1"
                    }
                    None => {
                        "SELECT scope, provider, model, until_us, reason FROM provider_cooldown \
                         WHERE provider = ?1 AND model = '' AND until_us > ?2 \
                         ORDER BY until_us DESC LIMIT 1"
                    }
                };
                let row = match model {
                    Some(m) => conn.query_row(sql, params![provider, m, now_us()], |r| {
                        Ok(CooldownRow {
                            scope: r.get(0)?,
                            provider: r.get(1)?,
                            model: r.get(2)?,
                            until_us: r.get(3)?,
                            reason: r.get(4)?,
                        })
                    }),
                    None => conn.query_row(sql, params![provider, now_us()], |r| {
                        Ok(CooldownRow {
                            scope: r.get(0)?,
                            provider: r.get(1)?,
                            model: r.get(2)?,
                            until_us: r.get(3)?,
                            reason: r.get(4)?,
                        })
                    }),
                };
                QueryRow::Cooldown(match row {
                    Ok(c) => Some(c),
                    Err(rusqlite::Error::QueryReturnedNoRows) => None,
                    Err(e) => return Err(map_err(false, e)),
                })
            }
            Query::CacheLedgerBlocks { session_key } => {
                let mut stmt = conn
                    .prepare(
                        "SELECT block_index, kind, tokens, hash FROM cache_ledger \
                         WHERE session_key = ?1 ORDER BY block_index",
                    )
                    .map_err(|e| map_err(false, e))?;
                let blocks = stmt
                    .query_map(params![session_key], |r| {
                        Ok(LedgerBlock {
                            index: r.get::<_, i64>(0)? as u32,
                            kind: r.get(1)?,
                            tokens: (r.get::<_, i64>(2)?).max(0) as u64,
                            hash: r.get(3)?,
                        })
                    })
                    .map_err(|e| map_err(false, e))?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| map_err(false, e))?;
                QueryRow::CacheLedger(blocks)
            }
            Query::PlanState { family } => {
                let row = conn.query_row(
                    "SELECT family, account, route, since_us, until_us, last_event, cause FROM plan_state \
                     WHERE family = ?1",
                    params![family],
                    |r| {
                        Ok(PlanStateProjRow {
                            family: r.get(0)?,
                            account: r.get(1)?,
                            route: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                            since_us: r.get(3)?,
                            until_us: r.get(4)?,
                            last_event: r.get(5)?,
                            cause: r.get(6)?,
                        })
                    },
                );
                QueryRow::PlanState(match row {
                    Ok(p) => Some(p),
                    Err(rusqlite::Error::QueryReturnedNoRows) => None,
                    Err(e) => return Err(map_err(false, e)),
                })
            }
            Query::SilenceStreak { provider, model } => {
                let row = conn.query_row(
                    "SELECT provider, model, count, first_us, last_us, last_event FROM silence_streak \
                     WHERE provider = ?1 AND model = ?2",
                    params![provider, model],
                    |r| {
                        Ok(StreakRow {
                            provider: r.get(0)?,
                            model: r.get(1)?,
                            count: r.get(2)?,
                            first_us: r.get(3)?,
                            last_us: r.get(4)?,
                            last_event: r.get(5)?,
                        })
                    },
                );
                QueryRow::SilenceStreak(match row {
                    Ok(s) => Some(s),
                    Err(rusqlite::Error::QueryReturnedNoRows) => None,
                    Err(e) => return Err(map_err(false, e)),
                })
            }
            Query::OverflowSpend {
                family_route,
                month_start_us,
            } => {
                // DESIGN §12.10.8: the cap is compared against measured
                // usage priced by the config table — a SUM over the
                // family's overflow-route `cost.computed` rows in the
                // UTC month. No separate counter exists to disagree with
                // the log.
                let nano: Result<i64, _> = conn.query_row(
                    "SELECT COALESCE(SUM(CAST(json_extract(payload, '$.total_nano') AS INTEGER)), 0) \
                     FROM events WHERE kind = 'cost.computed' \
                     AND json_extract(payload, '$.route') = ?1 \
                     AND ts_us >= ?2",
                    params![family_route, month_start_us],
                    |r| r.get(0),
                );
                let nano = nano.map_err(|e| map_err(false, e))?;
                QueryRow::Count(nano)
            }
            Query::AllEvents => QueryRow::Events(self.read_events(&conn)?),
            Query::EventsSince { kinds, since_us } => {
                // Spec §4.18 (ADR-054): the serving path's bounded read —
                // kind-filtered inside SQLite (the idx_events_kind_ts index
                // serves it), window-bounded by the caller. Never a scan of
                // the whole log.
                let placeholders = kinds.iter().map(|_| "?").collect::<Vec<_>>().join(",");
                let sql = format!(
                    "SELECT event_id, ts_us, kind, request_id, session, schema_version, payload, \
                     body_hash, trace_ref FROM events WHERE kind IN ({placeholders}) \
                     AND ts_us >= ? ORDER BY event_id"
                );
                let mut stmt = conn.prepare(&sql).map_err(|e| map_err(false, e))?;
                let mut params: Vec<&dyn rusqlite::ToSql> =
                    kinds.iter().map(|k| k as &dyn rusqlite::ToSql).collect();
                params.push(&since_us);
                let mut rows = stmt
                    .query(params.as_slice())
                    .map_err(|e| map_err(false, e))?;
                let mut out = Vec::new();
                while let Some(row) = rows.next().map_err(|e| map_err(false, e))? {
                    let kind_raw: String = row.get(2).map_err(|e| map_err(false, e))?;
                    let payload: String = row.get(6).map_err(|e| map_err(false, e))?;
                    out.push(StoredEvent {
                        event_id: EventId(row.get(0).map_err(|e| map_err(false, e))?),
                        ts_us: row.get(1).map_err(|e| map_err(false, e))?,
                        kind: EventKind::from_str_lossy(&kind_raw), // None = unknown, tolerated
                        kind_raw,
                        request_id: row.get(3).map_err(|e| map_err(false, e))?,
                        session: row.get(4).map_err(|e| map_err(false, e))?,
                        schema_version: row.get(5).map_err(|e| map_err(false, e))?,
                        payload: serde_json::from_str(&payload).map_err(|e| {
                            StoreError::Sql(format!("event payload is not JSON: {e}"))
                        })?,
                        body_hash: row.get(7).map_err(|e| map_err(false, e))?,
                        trace_ref: row.get(8).map_err(|e| map_err(false, e))?,
                    });
                }
                QueryRow::Events(out)
            }
        })
    }

    fn rebuild(&self, which: Projection) -> Result<RebuildStats, StoreError> {
        self.rebuild_with_silence_window(which, REBUILD_DEFAULT_SILENCE_WINDOW_US)
    }

    fn schema_version(&self) -> Result<u32, StoreError> {
        let conn = self.conn.lock().expect("store mutex");
        conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|v| v as u32)
        .map_err(|e| map_err(false, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use vadis_core::store::write_intent_then;

    fn tempdir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "vadis-store-{}-{}-{tag}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn intent_for(route: &str) -> NewEvent<'static> {
        NewEvent {
            kind: EventKind::UpstreamSubmitted,
            request_id: Some("req-1"),
            session: Some("sess-1"),
            body_hash: Some("0123456789abcdef"),
            trace_ref: None,
            payload: json!({"route": route, "attempt_index": 0, "attempt_id": "att-1"}),
        }
    }

    // (1) empty-database migration: fresh file, then reopen — idempotent.
    #[test]
    fn empty_db_migrates_and_reopens() {
        let dir = tempdir("migrate");
        let db = dir.join("state/vadis.db");
        {
            let s = SqliteStore::open(&db).unwrap();
            assert_eq!(s.schema_version().unwrap(), MIGRATIONS.last().unwrap().0);
        }
        let s = SqliteStore::open(&db).unwrap();
        assert_eq!(
            s.schema_version().unwrap(),
            MIGRATIONS.last().unwrap().0,
            "reopening applies nothing further"
        );
        // All five tables exist.
        let conn = s.conn.lock().unwrap();
        for t in [
            "schema_version",
            "events",
            "sessions",
            "cache_ledger",
            "quota_counters",
            "provider_cooldown",
        ] {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = ?1",
                    params![t],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "table {t} must exist");
        }
    }

    // (2) projection == rebuild, row for row (ADR-009 item 5 / CONF-21's
    //     in-crate twin). Both stores run the **same** event sequence; the
    //     incremental one layers `project` calls on top, the rebuild one
    //     recomputes from `events` alone. Exercises every projection.
    #[test]
    fn projection_equals_rebuild() {
        // The intent payload carries prefix_blocks in both runs — the
        // encoder computed them from exactly the outbound bytes (§12.10.6),
        // so the log alone fully determines the cache ledger.
        let intent = || {
            let mut ev = intent_for("zai/glm-5.3");
            ev.payload = json!({
                "route": "zai/glm-5.3", "attempt_index": 0, "attempt_id": "att-1",
                "prefix_blocks": [
                    {"index": 0, "kind": "system",  "tokens": 120, "hash": "1111111111111111"},
                    {"index": 1, "kind": "message", "tokens": 800, "hash": "2222222222222222"},
                    {"index": 2, "kind": "tool",    "tokens": 300, "hash": "3333333333333333"}
                ]
            });
            ev
        };
        let scenario = |s: &SqliteStore| {
            s.append(NewEvent {
                kind: EventKind::RequestReceived,
                request_id: Some("req-1"),
                session: Some("sess-1"),
                body_hash: Some("aaaaaaaaaaaaaaaa"),
                trace_ref: None,
                payload: json!({"protocol_in": "chat", "client": "codex", "turn_index": 1}),
            })
            .unwrap();
            let e2 = s
                .append(NewEvent {
                    kind: EventKind::SessionBound,
                    request_id: Some("req-1"),
                    session: Some("sess-1"),
                    body_hash: None,
                    trace_ref: None,
                    payload: json!({"session_key": "sess-1", "provider": "zai", "model": "glm-5.3", "ttl_us": 43_200_000_000_i64}),
                })
                .unwrap();
            let e3 = s.append(intent()).unwrap();
            s.append(NewEvent {
                kind: EventKind::UpstreamResponded,
                request_id: Some("req-1"),
                session: Some("sess-1"),
                body_hash: None,
                trace_ref: None,
                payload: json!({"status": 200, "latency_ms": 812}),
            })
            .unwrap();
            let e5 = s
                .append(NewEvent {
                    kind: EventKind::QuotaCharged,
                    request_id: Some("req-1"),
                    session: Some("sess-1"),
                    body_hash: None,
                    trace_ref: Some("2026-09-19T12.jsonl:42"),
                    payload: json!({"provider": "zai", "plan_idx": 0, "window_start_us": 1_767_225_600_000_000_i64, "tokens": 14520, "remaining": 985_480}),
                })
                .unwrap();
            let e6 = s
                .append(NewEvent {
                    kind: EventKind::ErrorClassified,
                    request_id: Some("req-2"),
                    session: None,
                    body_hash: None,
                    trace_ref: None,
                    payload: json!({"status": 429, "reason": "rate_limit", "action": "fallback provider",
                                    "demotion": {"scope": "provider", "provider": "moonshot", "model": "", "until_us": 1_789_830_000_000_000_i64, "reason": "rate_limit"}}),
                })
                .unwrap();
            (e2, e3, e5, e6)
        };
        let blocks: Vec<(u32, &str, u64, &str)> = vec![
            (0, "system", 120, "1111111111111111"),
            (1, "message", 800, "2222222222222222"),
            (2, "tool", 300, "3333333333333333"),
        ];

        let inc_dir = tempdir("inc");
        let a = SqliteStore::open(&inc_dir.join("state/vadis.db")).unwrap();
        let (e2, e3, e5, e6) = scenario(&a);
        a.project(ProjectionWrite::SessionBound {
            session_key: "sess-1",
            provider: "zai",
            model: "glm-5.3",
            ttl_us: 43_200_000_000,
            last_event: e2,
        })
        .unwrap();
        a.project(ProjectionWrite::CacheLedgerPut {
            session_key: "sess-1",
            blocks: &blocks,
            last_event: e3,
        })
        .unwrap();
        a.project(ProjectionWrite::QuotaCharged {
            provider: "zai",
            plan_idx: 0,
            window_start_us: 1_767_225_600_000_000,
            tokens: 14_520,
            last_event: e5,
        })
        .unwrap();
        a.project(ProjectionWrite::ProviderCooldown {
            scope: "provider",
            provider: "moonshot",
            model: "",
            until_us: 1_789_830_000_000_000,
            reason: "rate_limit",
            last_event: e6,
        })
        .unwrap();

        let reb_dir = tempdir("reb");
        let b = SqliteStore::open(&reb_dir.join("state/vadis.db")).unwrap();
        let _ = scenario(&b);
        let stats = b.rebuild(Projection::All).unwrap();
        assert_eq!(stats.events_scanned, 6);

        for p in [
            Projection::Sessions,
            Projection::CacheLedger,
            Projection::QuotaCounters,
            Projection::ProviderCooldown,
        ] {
            let strip_expires = |rows: Vec<String>| {
                rows.into_iter()
                    .map(|r| {
                        // sessions rows: key|provider|model|requests_seen|expires_at_us|last_event.
                        // expires_at_us anchors on each store's own event
                        // timestamps, so it is not comparable across two
                        // stores — only within one (asserted below).
                        let mut f: Vec<&str> = r.split('|').collect();
                        if f.len() == 6 {
                            f.remove(4);
                        }
                        f.join("|")
                    })
                    .collect::<Vec<_>>()
            };
            let ra = strip_expires(a.projection_rows(p).unwrap());
            let rb = strip_expires(b.projection_rows(p).unwrap());
            assert_eq!(ra, rb, "projection {p:?}: incremental != rebuild");
        }

        // Rebuilding the incremental store's own projections must be a no-op
        // (the strongest form of the oracle).
        let before = a.projection_rows(Projection::All).unwrap();
        a.rebuild(Projection::All).unwrap();
        assert_eq!(a.projection_rows(Projection::All).unwrap(), before);
    }

    // (3) intent write failure ⇒ no effect, error returned (ADR-009 item 8;
    //     the SQLite-level twin of CONF-22, via a failing wrapper).
    #[test]
    fn intent_failure_runs_no_effect() {
        struct FailingStore;
        impl Store for FailingStore {
            fn append(&self, _: NewEvent<'_>) -> Result<EventId, StoreError> {
                Err(StoreError::Busy)
            }
            fn project(&self, _: ProjectionWrite<'_>) -> Result<(), StoreError> {
                panic!("projection must not be reached");
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
        let mut effect_ran = false;
        let err = write_intent_then(&FailingStore, intent_for("zai/glm-5.3"), |_| {
            effect_ran = true;
        })
        .unwrap_err();
        assert!(!effect_ran);
        assert_eq!(err, StoreError::Busy);
    }

    // (4) the writer lock: a second open on the same file is refused with
    //     the distinguishable `Locked` reason (ADR-009 item 8).
    #[test]
    fn second_writer_is_locked() {
        let dir = tempdir("lock");
        let db = dir.join("state/vadis.db");
        let _first = SqliteStore::open(&db).unwrap();
        let second = SqliteStore::open(&db);
        match second {
            Err(StoreError::Locked) => {}
            other => panic!("expected StoreError::Locked, got {other:?}"),
        }
    }

    // (5) events are ordered by event_id, and FULL intents are durable
    //     across a close/reopen.
    #[test]
    fn events_ordered_and_durable() {
        let dir = tempdir("order");
        let db = dir.join("state/vadis.db");
        {
            let s = SqliteStore::open(&db).unwrap();
            let ids: Vec<i64> = [
                (EventKind::RequestReceived, "req-1"),
                (EventKind::DecisionMade, "req-1"),
                (EventKind::UpstreamSubmitted, "req-1"),
                (EventKind::UpstreamResponded, "req-1"),
            ]
            .iter()
            .map(|(k, r)| {
                s.append(NewEvent {
                    kind: *k,
                    request_id: Some(r),
                    session: Some("sess-1"),
                    body_hash: None,
                    trace_ref: None,
                    payload: json!({}),
                })
                .unwrap()
                .0
            })
            .collect();
            assert!(ids.windows(2).all(|w| w[0] < w[1]), "strictly increasing");
        }
        let s = SqliteStore::open(&db).unwrap();
        let QueryRow::Events(evs) = s.query(Query::AllEvents).unwrap() else {
            panic!("expected events");
        };
        let kinds: Vec<&str> = evs.iter().map(|e| e.kind_raw.as_str()).collect();
        assert_eq!(
            kinds,
            vec![
                "request.received",
                "decision.made",
                "upstream.submitted",
                "upstream.responded"
            ]
        );
        // Every row carries the payload version from day one (ADR-009 item 7).
        assert!(evs.iter().all(|e| e.schema_version == EVENT_SCHEMA_VERSION));
    }

    #[test]
    fn rfc3339_matches_fixed_anchors() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(951_868_799), "2000-02-29T23:59:59Z"); // leap day
        assert_eq!(rfc3339_utc(1_767_225_600), "2026-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(1_789_821_296), "2026-09-19T12:34:56Z");
        assert_eq!(rfc3339_utc(4_107_542_400), "2100-03-01T00:00:00Z"); // 2100 is not a leap year
    }

    // (6) plan_state: incremental == rebuild (CONF-21's rule for the DDL
    //     v2 projection, DESIGN §12.10.8), plus the query surface and
    //     the additive migration from a v1 database.
    #[test]
    fn plan_state_projection_and_rebuild() {
        let dir = tempdir("plan-state");
        let db = dir.join("state/vadis.db");
        let s = SqliteStore::open(&db).unwrap();
        assert_eq!(s.schema_version().unwrap(), 4, "DDL version 4");

        let switched = || NewEvent {
            kind: EventKind::PlanSwitched,
            request_id: Some("req-1"),
            session: Some("sess-1"),
            body_hash: None,
            trace_ref: None,
            payload: json!({
                "family": "glm-5.3",
                "from_account": "primary",
                "to_account": "overflow",
                "from_route": "zai-plan/glm-5.3",
                "to_route": "zai/glm-5.3",
                "reason": "primary_exhausted",
                "probe": false,
                "cooldown_ms": 900_000i64,
            }),
        };
        let ev = s.append(switched()).unwrap();
        s.project(ProjectionWrite::PlanSwitched {
            family: "glm-5.3",
            account: "overflow",
            to_route: "zai/glm-5.3",
            cooldown_us: 900_000 * 1_000,
            cause: "primary_exhausted",
            last_event: ev,
        })
        .unwrap();

        let QueryRow::PlanState(row) = s.query(Query::PlanState { family: "glm-5.3" }).unwrap()
        else {
            panic!("expected a plan_state row");
        };
        let inc = row.expect("the family switched");
        assert_eq!(inc.account, "overflow");
        // ADR-049 §5.2: the row carries the ACTIVE ROUTE, and the
        // incremental write and the rebuild source it identically (the
        // event's own `to_route`).
        assert_eq!(inc.route, "zai/glm-5.3");
        assert_eq!(inc.last_event, ev.0);
        // since_us is the event row's own ts_us, read back directly.
        let ts_us: i64 = s
            .raw_connection()
            .query_row(
                "SELECT ts_us FROM events WHERE event_id = ?1",
                params![ev.0],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            inc.since_us, ts_us,
            "since_us anchors on the event's own ts_us"
        );
        assert_eq!(
            inc.until_us,
            Some(ts_us + 900_000 * 1_000),
            "until_us = since_us + the written cooldown"
        );

        // A family that never switched has no row (⇒ primary).
        assert!(matches!(
            s.query(Query::PlanState { family: "other" }).unwrap(),
            QueryRow::PlanState(None)
        ));

        // Rebuild from the log alone converges row for row.
        s.rebuild(Projection::PlanState).unwrap();
        let QueryRow::PlanState(after) = s.query(Query::PlanState { family: "glm-5.3" }).unwrap()
        else {
            panic!("expected a plan_state row");
        };
        assert_eq!(after.expect("still switched"), inc);
    }

    // (6b) ADR-064 §6.2 item 1: `silence_streak` incremental == rebuild —
    //      the same hand-written event sequence folded twice, through the
    //      writer (one `ProjectionWrite::SilenceStreak` per counted fact,
    //      the §2.3.1 shape the two proxy writers call) and through the
    //      §2.3.3 fold, must produce byte-equal rows; and rebuilding the
    //      incremental store's own rows is a no-op. Includes §2.5.4's
    //      mirror: facts before the last return-to-route `plan.switched`
    //      row are cleared in both paths.
    #[test]
    fn silence_streak_rebuild_equals_incremental() {
        let dir_a = tempdir("silence-inc");
        let a = SqliteStore::open(&dir_a.join("state/vadis.db")).unwrap();
        let dir_b = tempdir("silence-reb");
        let b = SqliteStore::open(&dir_b.join("state/vadis.db")).unwrap();
        assert_eq!(a.schema_version().unwrap(), 4, "DDL version 4");

        // W = 60 s in force for every fact and for the rebuild.
        const W: i64 = 60_000_000;
        let base = 1_800_000_000_000_000i64; // an arbitrary µs anchor

        // One counted fact as an event, with its ts_us pinned so the
        // fold's arithmetic is exact (append stamps now; the test sets
        // the row's own ts immediately after, BEFORE the incremental
        // write that anchors on it).
        let fact = |store: &SqliteStore, payload: serde_json::Value, ts: i64| EventId({
            let id = store
                .append(NewEvent {
                    kind: EventKind::ErrorClassified,
                    request_id: Some("req-1"),
                    session: Some("sess-1"),
                    body_hash: None,
                    trace_ref: None,
                    payload,
                })
                .unwrap()
                .0;
            store
                .raw_connection()
                .execute("UPDATE events SET ts_us = ?1 WHERE event_id = ?2", params![ts, id])
                .unwrap();
            id
        });
        let plan_switched = |store: &SqliteStore, ts: i64| EventId({
            let id = store
                .append(NewEvent {
                    kind: EventKind::PlanSwitched,
                    request_id: Some("req-1"),
                    session: Some("sess-1"),
                    body_hash: None,
                    trace_ref: None,
                    payload: json!({
                        "family": "m1",
                        "from_account": "overflow",
                        "to_account": "primary",
                        "from_route": "p-api/m1",
                        "to_route": "p-plan/m1",
                        "reason": "primary_recovered",
                        "probe": true,
                        "cooldown_ms": 0i64,
                    }),
                })
                .unwrap()
                .0;
            store
                .raw_connection()
                .execute("UPDATE events SET ts_us = ?1 WHERE event_id = ?2", params![ts, id])
                .unwrap();
            id
        });
        let s1 = |provider: &str| {
            json!({
                "reason": "stream_truncated",
                "action": "abort",
                "matched": false,
                "stream": true,
                "stream_end": "idle_timeout",
                "route": {"provider": provider, "model": "m1"},
            })
        };
        let s2 = json!({
            "reason": "timeout",
            "action": "abort",
            "matched": false,
            "stream": true,
            "stage": "unknown_outcome",
            "route": {"provider": "p-plan", "model": "m1"},
        });
        let not_counted = json!({
            "reason": "stream_truncated",
            "action": "abort",
            "matched": false,
            "stream": true,
            "stream_end": "upstream_ended_early",
            "route": {"provider": "p-plan", "model": "m1"},
        });

        // The sequence (both stores append identically):
        //   t0        S1 on p-plan/m1     — run 1 starts
        //   t0+10s    S2 on p-plan/m1     — same run, count 2
        //   t0+20s    not-counted cause   — no fact
        //   t0+30s    S1 on p-api/m1      — another route's own run
        //   t0+200s   S1 on p-plan/m1     — outside W: a fresh run of 1
        //   t0+210s   plan.switched back to p-plan/m1 (§2.5.4's clear)
        //   t0+220s   S1 on p-plan/m1     — post-return: count 1
        let seq: Vec<(serde_json::Value, i64)> = vec![
            (s1("p-plan"), base),
            (s2.clone(), base + 10_000_000),
            (not_counted, base + 20_000_000),
            (s1("p-api"), base + 30_000_000),
            (s1("p-plan"), base + 200_000_000),
        ];
        for (i, (payload, ts)) in seq.iter().enumerate() {
            let ea = fact(&a, payload.clone(), *ts);
            let _eb = fact(&b, payload.clone(), *ts);
            // The incremental path: one upsert per counted fact, the
            // route read off the payload the same way the writer does.
            let route = payload["route"]["provider"].as_str().unwrap();
            let model = payload["route"]["model"].as_str().unwrap();
            a.project(ProjectionWrite::SilenceStreak {
                provider: route,
                model,
                window_us: W,
                last_event: ea,
            })
            .unwrap();
            if i == 1 {
                // The escalation's run of 2, observed at the instant it
                // would fire: t0 and t0+10s joined one run (the t0+200s
                // fact below has not reset it yet).
                let QueryRow::SilenceStreak(pre) =
                    a.query(Query::SilenceStreak { provider: "p-plan", model: "m1" }).unwrap()
                else {
                    panic!("expected a streak row");
                };
                assert_eq!(
                    pre.as_ref().unwrap().count,
                    2,
                    "t0 and t0+10s join one run"
                );
            }
        }

        // §2.5.4: the return trip clears the plan route's rows (the
        // incremental clear is the writer's — the delete the rebuild
        // must mirror).
        let _ret_a = plan_switched(&a, base + 210_000_000);
        let _ret_b = plan_switched(&b, base + 210_000_000);
        a.project(ProjectionWrite::SilenceStreakClear {
            provider: "p-plan",
            model: "m1",
        })
        .unwrap();

        // One post-return fact: a fresh run of 1.
        let post = s1("p-plan");
        let ea = fact(&a, post.clone(), base + 220_000_000);
        let _eb = fact(&b, post, base + 220_000_000);
        a.project(ProjectionWrite::SilenceStreak {
            provider: "p-plan",
            model: "m1",
            window_us: W,
            last_event: ea,
        })
        .unwrap();

        // The rebuild folds the same log with the same W.
        b.rebuild_with_silence_window(Projection::All, W).unwrap();
        let rows_a = a.projection_rows(Projection::SilenceStreak).unwrap();
        let rows_b = b.projection_rows(Projection::SilenceStreak).unwrap();
        assert_eq!(rows_a, rows_b, "incremental != rebuild for silence_streak");
        // And the rows say what the sequence means: p-plan/m1 is a
        // post-return run of 1, p-api/m1 a run of 1.
        assert_eq!(rows_a.len(), 2, "two routes, two rows: {rows_a:?}");
        let QueryRow::SilenceStreak(plan_row) =
            a.query(Query::SilenceStreak { provider: "p-plan", model: "m1" }).unwrap()
        else {
            panic!("expected a streak row");
        };
        let plan_row = plan_row.expect("the post-return fact re-created it");
        assert_eq!(plan_row.count, 1);
        assert_eq!(plan_row.first_us, base + 220_000_000);

        // The strongest form: rebuilding the incremental store's own
        // rows is a no-op.
        let before = a.projection_rows(Projection::SilenceStreak).unwrap();
        a.rebuild_with_silence_window(Projection::SilenceStreak, W)
            .unwrap();
        assert_eq!(a.projection_rows(Projection::SilenceStreak).unwrap(), before);
    }

    // (6c) ADR-064 §6.2 item 2: `plan_state.cause` — an incremental move
    //      and a rebuilt one carry the same cause word, and a DDL-v3 row
    //      (cause NULL after the additive migration) reads back `None`,
    //      which the guard maps to `primary_exhausted` (the only cause
    //      that existed before the ADR; the mapping itself is witnessed
    //      by CONF-121 (b)/(e) end to end).
    #[test]
    fn plan_state_cause_identity_and_ddl_v3_null_reading() {
        let dir_a = tempdir("cause-inc");
        let a = SqliteStore::open(&dir_a.join("state/vadis.db")).unwrap();
        let dir_b = tempdir("cause-reb");
        let b = SqliteStore::open(&dir_b.join("state/vadis.db")).unwrap();

        let switched = || NewEvent {
            kind: EventKind::PlanSwitched,
            request_id: Some("req-1"),
            session: Some("sess-1"),
            body_hash: None,
            trace_ref: None,
            payload: json!({
                "family": "m1",
                "from_account": "primary",
                "to_account": "overflow",
                "from_route": "p-plan/m1",
                "to_route": "p-api/m1",
                "reason": "primary_silent",
                "probe": false,
                "silence": {"count": 2, "threshold": 2, "window_us": 300_000_000i64},
                "cooldown_ms": 0i64,
            }),
        };
        let ev = a.append(switched()).unwrap();
        a.project(ProjectionWrite::PlanSwitched {
            family: "m1",
            account: "overflow",
            to_route: "p-api/m1",
            cooldown_us: 0,
            cause: "primary_silent",
            last_event: ev,
        })
        .unwrap();
        let _ = b.append(switched()).unwrap();
        b.rebuild(Projection::PlanState).unwrap();

        for (label, s) in [("incremental", &a), ("rebuild", &b)] {
            let QueryRow::PlanState(row) = s.query(Query::PlanState { family: "m1" }).unwrap()
            else {
                panic!("expected a plan_state row");
            };
            assert_eq!(
                row.expect("the family switched").cause.as_deref(),
                Some("primary_silent"),
                "{label}: the cause word is the row's own reason"
            );
        }

        // A DDL-v3 row migrated forward: the ALTER added `cause` as
        // NULL and nothing backfills it — the read is `None`, and the
        // consumer (the guard) maps it to `primary_exhausted`.
        a.raw_connection()
            .execute("UPDATE plan_state SET cause = NULL", [])
            .unwrap();
        let QueryRow::PlanState(row) = a.query(Query::PlanState { family: "m1" }).unwrap() else {
            panic!("expected a plan_state row");
        };
        assert_eq!(
            row.expect("the family switched").cause,
            None,
            "a DDL-v3 row reads NULL, mapped by the reader — never here"
        );
    }

    // (7) a v1 database migrates forward (v2, then v3) without touching
    //     events.
    #[test]
    fn v1_db_migrates_to_v2() {
        let dir = tempdir("v1-to-v2");
        let db = dir.join("state/vadis.db");
        {
            let s = SqliteStore::open(&db).unwrap();
            s.append(NewEvent {
                kind: EventKind::RequestReceived,
                request_id: Some("req-old"),
                session: None,
                body_hash: None,
                trace_ref: None,
                payload: json!({"protocol_in": "chat"}),
            })
            .unwrap();
        }
        let s = SqliteStore::open(&db).unwrap();
        assert_eq!(s.schema_version().unwrap(), 4, "v1 migrates to the current DDL");
        let QueryRow::Events(evs) = s.query(Query::AllEvents).unwrap() else {
            panic!("expected events");
        };
        assert_eq!(evs.len(), 1, "no row rewritten or lost");
    }

    // ADR-049 §5.2: a pre-tier (v2) database migrates forward, and the
    // generalized projection is rebuildable from its own log — the last
    // `plan.switched` row's `to_route` becomes the active route, so an
    // old log (whose rows already carry `to_route`) backfills the new
    // column without any row rewrite.
    #[test]
    fn v2_plan_state_backfills_route_from_the_log_on_rebuild() {
        let dir = tempdir("v2-route");
        let db = dir.join("state/vadis.db");
        let s = SqliteStore::open(&db).unwrap();
        let ev = s
            .append(NewEvent {
                kind: EventKind::PlanSwitched,
                request_id: Some("req-old"),
                session: None,
                body_hash: None,
                trace_ref: None,
                payload: json!({
                    "family": "glm-5.3",
                    "from_account": "primary",
                    "to_account": "overflow",
                    "from_route": "zai-plan/glm-5.3",
                    "to_route": "zai/glm-5.3",
                    "reason": "primary_exhausted",
                    "probe": false,
                    "cooldown_ms": 0i64,
                }),
            })
            .unwrap();
        s.project(ProjectionWrite::PlanSwitched {
            family: "glm-5.3",
            account: "overflow",
            to_route: "zai/glm-5.3",
            cooldown_us: 0,
            cause: "primary_exhausted",
            last_event: ev,
        })
        .unwrap();
        // The rebuild's word is law: the row comes back identical.
        s.rebuild(Projection::PlanState).unwrap();
        let QueryRow::PlanState(row) = s.query(Query::PlanState { family: "glm-5.3" }).unwrap()
        else {
            panic!("expected a plan_state row");
        };
        let row = row.expect("still switched");
        assert_eq!(row.route, "zai/glm-5.3", "the active route survives");
        assert_eq!(row.account, "overflow");
    }

    #[test]
    fn unopenable_when_parent_is_a_file() {
        let dir = tempdir("unopen");
        let blocker = dir.join("state");
        std::fs::write(&blocker, b"not a directory").unwrap();
        let err = SqliteStore::open(&dir.join("state/vadis.db")).unwrap_err();
        assert!(matches!(err, StoreError::Unopenable(_)), "got {err:?}");
    }

    // (7) class guard: every Query variant's SQL actually executes on a
    //     migrated database. This bug class has now struck three times
    //     (the cooldown query's lost space, then the sessions/quota_counters
    //     reads): a backslash continuation whose next line starts with
    //     whitespace silently loses that whitespace, the SQL becomes a
    //     syntax error, and the serving path swallows it via .ok(). A
    //     variant that returns Err here is unreadable in production,
    //     whatever the unit tests say.
    #[test]
    fn every_query_variant_executes_without_sql_error() {
        let dir = tempdir("query-exec");
        let db = dir.join("state/vadis.db");
        let s = SqliteStore::open(&db).unwrap();

        // Seed one row per projection so the reads have data to find.
        let ev = s
            .append(NewEvent {
                kind: EventKind::SessionBound,
                request_id: Some("req-1"),
                session: Some("sess-guard"),
                body_hash: None,
                trace_ref: None,
                payload: json!({}),
            })
            .unwrap();
        s.project(ProjectionWrite::SessionBound {
            session_key: "sess-guard",
            provider: "prov-a",
            model: "glm-5.3",
            ttl_us: 3_600_000_000,
            last_event: ev,
        })
        .unwrap();
        let ev = s
            .append(NewEvent {
                kind: EventKind::QuotaCharged,
                request_id: Some("req-1"),
                session: Some("sess-guard"),
                body_hash: None,
                trace_ref: None,
                payload: json!({}),
            })
            .unwrap();
        s.project(ProjectionWrite::QuotaCharged {
            provider: "prov-a",
            plan_idx: 0,
            window_start_us: 0,
            tokens: 105,
            last_event: ev,
        })
        .unwrap();
        let ev = s
            .append(NewEvent {
                kind: EventKind::FailoverTriggered,
                request_id: Some("req-1"),
                session: Some("sess-guard"),
                body_hash: None,
                trace_ref: None,
                payload: json!({}),
            })
            .unwrap();
        s.project(ProjectionWrite::ProviderCooldown {
            scope: "provider",
            provider: "prov-a",
            model: "",
            until_us: now_us() + 60_000_000,
            reason: "quota_exhausted",
            last_event: ev,
        })
        .unwrap();
        let ev = s
            .append(NewEvent {
                kind: EventKind::PlanSwitched,
                request_id: Some("req-1"),
                session: Some("sess-guard"),
                body_hash: None,
                trace_ref: None,
                payload: json!({
                    "family": "glm-5.3",
                    "from_account": "primary",
                    "to_account": "overflow",
                    "from_route": "prov-a/glm-5.3",
                    "to_route": "prov-b/glm-5.3",
                    "reason": "primary_exhausted",
                    "probe": false,
                    "cooldown_ms": 0i64,
                }),
            })
            .unwrap();
        s.project(ProjectionWrite::PlanSwitched {
            family: "glm-5.3",
            account: "overflow",
            to_route: "prov-b/glm-5.3",
            cooldown_us: 0,
            cause: "primary_exhausted",
            last_event: ev,
        })
        .unwrap();
        let ev = s
            .append(NewEvent {
                kind: EventKind::CostComputed,
                request_id: Some("req-1"),
                session: Some("sess-guard"),
                body_hash: None,
                trace_ref: None,
                payload: json!({"route": "prov-b/glm-5.3", "total_nano": 200_000i64}),
            })
            .unwrap();
        // cost.computed is its own projection sink (the events row is the
        // read model for OverflowSpend); nothing further to project.

        // Execute every Query variant. query_row maps "no row" to
        // QueryRow::Count(0) semantics upstream, but a SQL *syntax* error
        // surfaces as StoreError::Sql — exactly what this test exists to
        // catch. The content of each row is other tests' business.
        let cases: Vec<(&str, Result<QueryRow, StoreError>)> = vec![
            (
                "SessionBinding",
                s.query(Query::SessionBinding {
                    session_key: "sess-guard",
                }),
            ),
            (
                "SessionBindingsFor",
                s.query(Query::SessionBindingsFor {
                    provider: "prov-a",
                    model: "glm-5.3",
                }),
            ),
            (
                "SessionRequestsSeen",
                s.query(Query::SessionRequestsSeen {
                    session_key: "sess-guard",
                }),
            ),
            (
                "QuotaUsed",
                s.query(Query::QuotaUsed {
                    provider: "prov-a",
                    plan_idx: 0,
                    window_start_us: 0,
                }),
            ),
            (
                "Cooldown",
                s.query(Query::Cooldown {
                    provider: "prov-a",
                    model: Some("glm-5.3"),
                }),
            ),
            (
                "Cooldown(provider-wide)",
                s.query(Query::Cooldown {
                    provider: "prov-a",
                    model: None,
                }),
            ),
            (
                "CacheLedgerBlocks",
                s.query(Query::CacheLedgerBlocks {
                    session_key: "sess-guard",
                }),
            ),
            ("PlanState", s.query(Query::PlanState { family: "glm-5.3" })),
            (
                "OverflowSpend",
                s.query(Query::OverflowSpend {
                    family_route: "prov-b/glm-5.3",
                    month_start_us: 0,
                }),
            ),
            ("AllEvents", s.query(Query::AllEvents)),
        ];
        for (name, res) in cases {
            let Ok(row) = res else {
                panic!("Query::{name} returned Err — its SQL does not execute");
            };
            // Cooldown reads for a provider with a live row must find it;
            // this pins the read the SQL-space fix made reachable.
            if name == "Cooldown" {
                assert!(
                    matches!(row, QueryRow::Cooldown(Some(_))),
                    "{name} lost the row"
                );
            }
        }

        // The two projection-read fixes, pinned by content so a regression cannot hide
        // behind "no Err": the seeded rows are actually visible.
        let QueryRow::SessionBinding(Some(bind)) = s
            .query(Query::SessionBinding {
                session_key: "sess-guard",
            })
            .unwrap()
        else {
            panic!("SessionBinding must find the seeded row");
        };
        assert_eq!(bind.provider, "prov-a");
        let QueryRow::Count(used) = s
            .query(Query::QuotaUsed {
                provider: "prov-a",
                plan_idx: 0,
                window_start_us: 0,
            })
            .unwrap()
        else {
            panic!("QuotaUsed returns Count");
        };
        assert_eq!(used, 105, "QuotaUsed must read the charged tokens, not 0");
        let QueryRow::Count(spend) = s
            .query(Query::OverflowSpend {
                family_route: "prov-b/glm-5.3",
                month_start_us: 0,
            })
            .unwrap()
        else {
            panic!("OverflowSpend returns Count");
        };
        assert_eq!(spend, 200_000, "OverflowSpend sums the seeded cost row");
    }
}
