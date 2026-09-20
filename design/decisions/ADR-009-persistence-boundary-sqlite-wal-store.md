# ADR-009 — persistence: one local SQLite/WAL store behind `trait Store`; bodies are never persisted

- Status: accepted
- Date: 2026-09-19
- Related: spec §1 (non-goals) / §4.5 (state) / §6 (observation); DESIGN §8 (state and persistence), §12.1 (crate list), §12.5 (config types); ADR-005 (trace as the only product↔autowork interface); ADR-010 (the event log is the state truth)

## Background

DESIGN §8 previously said "single process, no DB": the cache ledger, the sticky table and the quota
counters lived in memory and were handed over across a restart by a periodic JSON snapshot. Round 2
lands the data plane and the trace, so persistence has to be answered for real, and the snapshot model
turns out to carry four costs that only appear once state is on the critical path:

1. **Atomicity is hand-rolled.** A snapshot writer needs write-temp + fsync + rename, plus a story for a
   partially written file and for a crash between the rename and the in-memory serialization. That is a
   recovery path written from scratch, in a project whose job is routing.
2. **There is no query surface.** "How much of the monthly allowance is left", "which prefix did this
   session last send", "what did the ledger look like at 14:00" all reduce to filters and aggregates over
   the state; with a JSON blob every reader re-implements that aggregation — a second implementation of
   the bookkeeping, which is exactly the skew ADR-005 forbids on the research side, now inside the product.
3. **Read-modify-write needs mutual exclusion.** The quota counter and the sticky binding are
   read-decide-write sequences; a snapshot model either serializes them by hand or loses updates.
4. **It cannot represent the crash window.** "An intent was sent upstream and the response never arrived"
   is a fact that must survive the crash verbatim (ADR-010). A periodic snapshot of *derived counters*
   loses it by construction.

The usual objection — "a database adds latency to the request path" — is measurable, and the measurement
(Evidence below) does not support it.

## Decision

1. **One embedded database, one file, one writer.** Local state is a single SQLite database in **WAL**
   mode, hidden behind `trait Store`. The domain (`router-core`) keeps declaring the traits it already
   declares (`CacheLedger`, `SessionTable`, `QuotaStore`, `TraceSink`, DESIGN §12.2) and stays free of
   I/O; `Store` is the **lower seam** that the state service's implementations are built on. The SQLite
   implementation (`rusqlite`, bundled SQLite) lands in a new workspace member `crates/router-store`.
2. **Tables (v0.1).** `events` is the truth (ADR-010). `sessions` (the sticky table), `cache_ledger` and
   `quota_counters` are **projections** of it, plus `schema_version` for the store's own migrations. Only
   `events` is contractual; a projection's column set is implementation-defined because it is rebuildable.
3. **Request and response bodies are never persisted.** Not in the event log and not in the trace: an
   event carries `body_hash` = the **first 16 hex chars of `sha256(router-visible body bytes)`** (the same
   convention and the same helper as the prefix-block hash of spec §6) plus a pointer (`trace_ref` = trace
   file + line) so that a body can be *checked* whenever the operator captured it out of band. Raw bytes
   live only in captures taken outside the product; the serving path never reads them back, and there is no
   "router keeps your conversations" surface at all.
4. **Durability is tiered, and the tier is a property of the event class:**
   - **intent / accounting events** (`upstream.submitted`, `upstream.responded`, `cost.computed`,
     `quota.charged`, `session.bound`, `failover.triggered`, `config.applied`): `synchronous=FULL`, one
     transaction per event, committed **before** the effect they authorize (ADR-010).
   - **derived projections** (sticky table, cache ledger, quota counters): `synchronous=NORMAL` plus group
     commit, because they can be recomputed from `events`.
   The test for the tier is a single question: *may this fact be recomputed?* No → FULL before the effect.
   Yes → NORMAL and batch it.
5. **A projection is never the truth.** A projection that is stale, lost or corrupt is rebuilt from
   `events` by a single scan; the rebuild is a startup/maintenance path, never a request path. Any
   projection that cannot be rebuilt is by definition not a projection.
6. **The store is a startup prerequisite, and the path is fixed in v0.1.** The database defaults to
   `<directory containing the config file>/state/router.db` (`state/` is already gitignored). If it cannot
   be opened or migrated, `serve` exits non-zero with the reason instead of running with a silent
   in-memory fallback; there is no "state off" switch in v0.1. A `state:` config section (so the file can
   live elsewhere, exactly as `trace.dir` does) is an **additive future key** that would not break
   existing configs (spec §4.1 sets that precedent).
7. **Migrations are forward-only, and three version axes stay distinct:**

   | Axis | Where | Meaning |
   |---|---|---|
   | store DDL version | `schema_version` table | the schema of the file (migrations) |
   | event payload version | `events.schema_version`, per row | the truth's own version: old rows are **never rewritten**, readers upcast |
   | trace record version | `DecisionRecord.schema_version` (spec §6, DESIGN §12.6) | the analysis record's version (ADR-005) |

   The event payload is versioned **from day one** because the event log is the state truth: a row written
   by v0.1 must still be readable by v0.N. A DDL migration never rewrites event rows.
8. **Failure modes are explicit (fail by design):**

   | Failure | Behaviour |
   |---|---|
   | store cannot be opened or migrated at startup | `serve` exits non-zero with the reason (never a silent in-memory fallback) |
   | an intent/accounting (FULL) write fails mid-request | the request is **rejected before anything reaches the upstream** (`500 internal`, `details.stage = "intent"`): nothing was billed, the client's retry is safe |
   | a projection (NORMAL) write fails | the request is unaffected; the projection is marked stale and rebuilt from `events` |
   | a trace write fails | unchanged (spec §8): does not block the request, `errors[].kind = trace_write_failed` |
   | the process dies mid-request | the crash window is a recorded fact, not a guess (ADR-010: `unknown_outcome`) |
   | a second `serve` on the same state directory | rejected at startup (writer lock), rather than two writers on one file |
9. **The hosted form is the same trait.** Multi-user / multi-node is an explicit v0.1 non-goal (spec §1);
   when it is wanted, it is a **second `Store` implementation** (PostgreSQL) plus its own migration set,
   not a change to the domain or to any plugin.

## Evidence (measured)

Method: one SQLite file in WAL mode on the internal APFS volume, rows of ≈ 410 B (a 372 B event payload
plus the fixed columns), 3,000 inserts per run, latency measured **per statement** through Python's
`sqlite3` binding. The binding adds Python-level call overhead per statement, so these numbers are an
**upper bound** relative to `rusqlite` in the real write path. Host: Apple M4 Pro, macOS 27.0, APFS (SSD),
page size 4096, 2026-09-19.

| Write pattern | p50 | p99 | max |
|---|---|---|---|
| per-row commit, `synchronous=FULL` (intent / accounting class) | 0.051 ms | 0.100 ms | 0.594 ms |
| group of 16 rows per commit, `synchronous=FULL` | 0.079 ms / commit | 0.132 ms / commit | 0.981 ms |
| per-row commit, `synchronous=NORMAL` (projection class) | 0.009 ms | 0.026 ms | 3.907 ms |

What the measurement decides:

- A request writes a handful of FULL events. At p99 ≈ 0.1 ms per commit the durable-write cost is roughly
  0.3–0.5 ms per request — below the resolution of the latency gate (router's own overhead budget) and two
  to four orders of magnitude below an upstream round trip. **Latency is not the obstacle; complexity is.**
- Group commit at `NORMAL` amortizes to ≈ 0.008 ms/row (p99), which is why the projections may be batched
  and why the durability tier is chosen per event class rather than globally.
- The `max` column is the OS flush, not SQLite's bookkeeping (one NORMAL commit hit 3.9 ms, a
  checkpoint/fsync). It is invisible at p99 but it argues against stacking many small FULL commits into one
  request's path.
- An earlier exploratory run at the same row size on the same host produced per-row FULL p50 0.037 / p99
  0.100 / max 0.607 ms and a 16-row group commit p99 of 0.077 ms; the re-measurement above lands within one
  Python-call of it, so the conclusion does not depend on the harness.

**Re-measurement is owed, and it is part of the data-plane latency gate.** These numbers come from a
Python `sqlite3` harness over 410 B rows. The write path that actually ships (`rusqlite` inside
`crates/router-store`, real event payloads, a database that grows all day) must be re-measured as
part of that gate, including one case with a much longer payload and one against a larger database
file. This ADR's decision does not depend on the outcome — a factor of ten still leaves the write
path far below an upstream round trip — but the gate's latency budget does, and the budget must
name measured numbers rather than these.

No number above is inferred; all of them come from these two runs. Reproducing them needs nothing but a WAL
database, 410-byte `events` rows and the per-statement timing loop described here.

## Alternatives considered

| Alternative | Why rejected |
|---|---|
| memory + periodic JSON snapshot (the previous DESIGN §8) | hand-rolled atomicity, no query surface, read-modify-write without exclusion, and it cannot represent the crash window |
| the event log as append-only JSONL files (no database) | the state side needs queries and read-modify-write, not a stream; a file index plus compaction is a database with fewer guarantees. The analysis side keeps its JSONL (the trace, ADR-005) — the two media have different jobs |
| an embedded KV store (`sled`, `redb`) | the consumers are filters and aggregates (quota, sessions, reports); a KV store pushes index design and recovery work back onto this project |
| DuckDB | an analytics engine: a good story for the reports, unproven as a serving-time write-ahead log |
| PostgreSQL now | multi-user/multi-node is an explicit v0.1 non-goal (spec §1); the trait keeps the seam for later without paying for it now |
| hand-rolled FFI to the `sqlite` C library | `rusqlite` is the maintained binding; hand-rolled FFI is avoidable risk and avoidable `unsafe` |

## Rationale

- SQLite is the boring choice: atomic commit, one file, one writer, WAL readers and versioned migrations
  are solved problems, exercised on every phone on earth. The gateway's job is routing, not storage
  engineering.
- Hiding it behind `trait Store` keeps `router-core` I/O-free (testable with an in-memory implementation)
  and keeps the hosted path an added file rather than a refactor.
- "Bodies are never persisted" keeps the footprint at one row per event (hundreds of bytes) and keeps the
  privacy surface at zero: router stores no conversation content, only hashes.
- Tiered durability is derived from one question (item 4), so adding a future event is a small,
  local decision rather than a global re-tuning.

## Consequences

- DESIGN §8 stops describing "memory + JSON snapshot"; the design of record is event log + projections +
  tiered durability (this ADR and ADR-010).
- `router-core` gains `trait Store`; `crates/router-store` joins the crate list with `rusqlite` in its
  dependency allowlist (DESIGN §12.1). The build gains a bundled C library (libsqlite3) — accepted
  knowingly, and the reason belongs in the commit message that adds the dependency.
- The state file becomes operator data alongside the trace directory: it must be backed up (quota counters
  and the sticky table cannot be reconstructed from a log that was not backed up) and treated as
  sensitive-in-aggregate. Backup is an ops duty, not a v0.1 feature.
- Startup gains a real failure mode with an operator action (`state/router.db` unreadable → fix permissions
  or move the file aside). The trace path keeps working independently of the store.
- Money stays on the trace path: `router replay` / `router stats` compute cost from the trace (ADR-005,
  spec §7), while quota remaining, sticky bindings and the cache ledger come from the store's projections.
  The two records are joined by `request_id` + `event_id` (spec §4.5).
- Because the DB is a new artifact in the workspace, the docs-first rule applies to it: this ADR and spec
  §4.5 are the contract, and the implementation follows them rather than defining them.
