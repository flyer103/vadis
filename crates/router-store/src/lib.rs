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

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use router_core::store::{
    CooldownRow, EventId, EventKind, NewEvent, Projection, ProjectionWrite, Query, QueryRow,
    RebuildStats, SessionBindingRow, Store, StoreError, StoredEvent, EVENT_SCHEMA_VERSION,
};
use rusqlite::{params, Connection, OpenFlags};

/// Store DDL version 1: the tables of DESIGN §12.10.4, verbatim in shape.
/// Forward-only: appending a migration means adding a `(version, sql)` row
/// here; a DDL migration never rewrites `events` rows (ADR-009 item 7).
const MIGRATIONS: &[(u32, &str)] = &[(
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
)];

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
    /// for a `router state` surface (a separate change, ADR-010).
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
        out.sort();
        Ok(out)
    }

    /// Escape hatch for tests and the future `router state` surface (a
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
                        "SELECT session_key, provider, model, requests_seen, expires_at_us\
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
                        "SELECT tokens_used FROM quota_counters\
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
                        "SELECT scope, provider, model, until_us, reason FROM provider_cooldown\
                         WHERE provider = ?1 AND (model = '' OR model = ?2) AND until_us > ?3\
                         ORDER BY until_us DESC LIMIT 1"
                    }
                    None => {
                        "SELECT scope, provider, model, until_us, reason FROM provider_cooldown\
                         WHERE provider = ?1 AND model = '' AND until_us > ?2\
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
            Query::AllEvents => QueryRow::Events(self.read_events(&conn)?),
        })
    }

    fn rebuild(&self, which: Projection) -> Result<RebuildStats, StoreError> {
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
        Ok(stats)
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
    use router_core::store::write_intent_then;
    use serde_json::json;

    fn tempdir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "router-store-{}-{}-{tag}",
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
        let db = dir.join("state/router.db");
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
        let a = SqliteStore::open(&inc_dir.join("state/router.db")).unwrap();
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
        let b = SqliteStore::open(&reb_dir.join("state/router.db")).unwrap();
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
        let db = dir.join("state/router.db");
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
        let db = dir.join("state/router.db");
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
        // Anchors computed independently of this code (unix epoch arithmetic).
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(951_868_799), "2000-02-29T23:59:59Z"); // leap day
        assert_eq!(rfc3339_utc(1_767_225_600), "2026-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(1_789_821_296), "2026-09-19T12:34:56Z");
        assert_eq!(rfc3339_utc(4_107_542_400), "2100-03-01T00:00:00Z"); // 2100 is not a leap year
    }

    #[test]
    fn unopenable_when_parent_is_a_file() {
        let dir = tempdir("unopen");
        let blocker = dir.join("state");
        std::fs::write(&blocker, b"not a directory").unwrap();
        let err = SqliteStore::open(&dir.join("state/router.db")).unwrap_err();
        assert!(matches!(err, StoreError::Unopenable(_)), "got {err:?}");
    }
}
