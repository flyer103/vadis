# ADR-054 — the live state read: `GET /state/events` and the `vadis stats` fallback

- Status: **accepted** (R72)
- Kind: **a contract + implementation change.** It adds one guarded read-only endpoint that reads
  the process's own store in-process, adds a windowed event query to the `Store` trait, and makes
  `vadis stats` fall back to that endpoint when the local read-only open is refused because a live
  `serve` holds the store.
- Owners: repo owner (authorisation: chat ruling 2026-10-07, "按照 A 落地" — option A: the running
  process answers; the EXCLUSIVE lock stands).
- Supersedes: nothing. ADR-009 item 8 (single writer, `PRAGMA locking_mode = EXCLUSIVE`) is
  **unchanged**; this ADR is the sanctioned follow-up `book/operations.md` parked as *"a dedicated
  `vadis state`-style surface is a separate change, not part of v0.1"*.

## 0. Context — the operator question this answers

`vadis stats --window 15m` against a live gateway is a **designed partial report** (spec §9.2): the
trace-derived figures print unchanged, and exactly one figure — `unknown outcome requests`, the only
figure that lives in the event log — is omitted with a stderr note, because `serve` holds the store
exclusively (ADR-009 item 8's inspection consequence) and a second process's read-only open is
refused. The only way to read the figure today is to stop `serve` first (book/operations.md).

The operator asked for the figure **while the gateway runs**. Two directions were considered; this
ADR is direction A.

## 1. Decision

**Direction A — the running process answers.** The writer itself serves the read:

1. **`GET /state/events`** (new, served): one guarded, read-only endpoint on the same router as
   `/health` and `/metrics`. Query: `window` (required, the §4.1 duration grammar, bounded to
   `24h`). Response: the two figures `stats` derives from the event log — `unknown_outcome_requests`
   and the window bounds — nothing else.
2. **A windowed store query, not a log scan.** The `Store` trait gains
   `Query::EventsSince { kinds, since_us }` — rows in `event_id` order with `ts_us >= since_us`,
   filtered by kind server-side (the `idx_events_kind_ts` index). The endpoint reads **only**
   `upstream.submitted` / `upstream.responded` rows in the window; it never calls `Query::AllEvents`.
   `Query::AllEvents`' bounded-use contract ("the serving path never scans the log") is **kept
   verbatim** — its meaning narrows to *full-log scans*, which this is not.
3. **`vadis stats` fallback.** When the local `SqliteStore::open_read_only` fails, `stats` retries
   through the live gateway: `GET http://<server.addr>/state/events?window=<w>` with the token from
   the config's `server.auth_token_env` (absent/unset ⇒ no header, as today). Success ⇒ the figure
   prints with source `gateway`; failure ⇒ today's omission note, unchanged.
4. **No new env, no config key.** The endpoint's address is the config's own `server.addr`; the
   token is the config's own `server.auth_token_env`. Nothing new to provision.

## 2. Why this shape (the invariants it keeps)

- **ADR-009 item 8 stands.** EXCLUSIVE locking stays; the second-`serve` refusal (CONF-23b) is
  untouched. The reader is the lock holder itself, so the lock's guarantee ("no second writer on one
  file") is not weakened — it is the reason the fallback works.
- **Observation boundary (AGENTS constraint 3).** The serving path's outputs remain the trace JSONL
  and the event log; its inputs remain the client's bytes and its own configuration. The endpoint
  reads the process's own store through the same mutex `/health`'s `plan` section already uses; it
  writes nothing. Nothing off-line enters the serving path, and the serving path reads nothing
  off-line analysis wrote.
- **Single derivation.** The endpoint's figure is `stats`'s own derivation (`EventFigures`): the
  fallback parses the endpoint's response and feeds it to the same printer. One derivation, two
  readers (the §4.16/ADR-041 single-owner rule, applied to the event-log figure).
- **No protocol surface moves.** The three protocol routes, the byte boundary and the cache gate are
  untouched; the endpoint is assembled exactly as `/metrics` is (same guard middleware, same
  assembly point), with its own guard word.
- **Windowed, bounded reads.** The query is time-bounded (≤ 24h) and kind-filtered; it is not
  `AllEvents`, so the log-scan prohibition is not violated in either letter or spirit. The serving
  path gains no unbounded read.

## 3. The contract (spec §4.18 is the full text)

Summary — the properties the ADR owns:

| Property | Value |
|---|---|
| Method/path | `GET /state/events` |
| Query | `window` — required, §4.1 duration grammar, `1ms..=24h` |
| Auth | behind `server.auth_token_env`'s guard, exactly as `/metrics` (protocol word `"state"`) |
| Success | `200` `application/json`: `{"window": {"from": …, "to": …}, "unknown_outcome_requests": N}` |
| Refused | `401` §8 body + one pre-pipeline trace record with `protocol_in: "state"` — §4.7's table |
| Other status | a defect; the status set is `{200, 401}` (404 is impossible: the route is served) |
| Reads | the process's own store (`AppState.store`), `EventsSince` only |
| Writes | nothing on the admitted arm (a refusal writes the guard's one record) |
| In-band figure | the window bounds, so "a report must state the window it covers" holds |

`unknown_outcome_requests` is defined exactly as §6: `upstream.submitted` events in the window with
no `upstream.responded` for the same `request_id`. It is a count and never priced.

## 4. The stats fallback (spec §9.2 amendment)

- Fires **only** on the local read-only open failure (the `Locked`/`Busy` refusal a live `serve`
  produces). Every other failure class (absent store, unreadable, schema too new) keeps today's
  behaviour: omit the figure, print the note.
- The request goes to `http://<server.addr>/state/events?window=<window>`; the client is built
  `.no_proxy()` (the macOS system proxy intercepts localhost and does not honour its own exclusion
  list for `127.0.0.1` — AGENTS "Environment gotchas"). Timeout: 5 s, one attempt.
- With `server.auth_token_env` set, the token is read from that env var and sent as
  `Authorization: Bearer <token>`; absent or empty ⇒ the request is sent with no credential (it will
  be refused if auth is on — the omission note then names the endpoint, so the operator learns the
  fix is to export the variable).
- On `200`: the figure prints with the same line shape, plus `(gateway)` marking the source; the
  stderr note disappears. On any other outcome: today's note, extended to name the gateway attempt.
- The `--json` shape gains `events.source: "store" | "gateway"` (present only when the figure is
  present).

## 5. Alternatives considered

| Alternative | Why rejected |
|---|---|
| **B — relax `locking_mode = EXCLUSIVE`; rely on WAL's multi-reader/single-writer** (any SQLite tool could open the file live; `stats`' local open would succeed) | A reversal of ADR-009 item 8's mechanism: the second-`serve` detection would move to a lock file (weaker on NFS; a stale lockfile after a crash blocks every later boot until removed by hand), and a crash between lockfile and store open leaves the invariant unevidenced. The operator's ask (the figure while serving) is met by A without touching the writer's guarantees. Recorded here so the next reader does not re-litigate it. |
| `/health` grows the figure (no new route) | `/health` is exempt from auth (§4.7) — putting a store read on an unauthenticated route would expose operational counts without a credential. Also overloads the liveness probe with a windowed query. |
| `/metrics` grows a series | §4.16 froze the 21-series set and the exclusion of this figure ("the serving path never scans the log" — `Query::AllEvents`); a per-scrape log scan is what that forbids. Adding a series is a spec §4.16 reversal; the windowed query in a purpose-built endpoint keeps both contracts intact. |
| `vadis state` subcommand | Same endpoint needed under the hood; a second CLI surface duplicates what `stats` already prints. The book's "vadis state-style surface" is satisfied by the endpoint. |
| IPC/socket channel to the writer | A second transport to maintain, a second auth story, and no HTTP precedent in the process. The HTTP endpoint reuses the guard, the router and the address the operator already has. |

## 6. Conformance

`CONF-106` (spec §12.8 register): a live rig (mock upstreams, `vadis_cli::serve`) drives three
requests, one left without a response, then asserts (a) the endpoint answers `200` with the exact
figure against an independent count over the store rows read after exit; (b) the status set is
`{200, 401}` and a refused call writes exactly one pre-pipeline record with
`protocol_in: "state"`; (c) the window bound: an `upstream.submitted` outside the window does not
count; (d) `vadis stats` against the live gateway prints the figure via the fallback (checked
through `stats::report` on a port-matched config) and stops serve first elsewhere.

## 7. Register updates

- `docs/spec.md`: §4.18 (the endpoint's contract), §4.7 (the guard's word list gains `"state"`,
  beside `"metrics"`), §9.2 (the fallback), §6 (nothing — the figure's definition is unchanged).
- `design/DESIGN.md`: §12.24 (the landing), §12.10.8/`Query::AllEvents` note (the windowed query's
  addition, one sentence), CONF-106 in §12.8.
- `book/operations.md`: the inspection consequence paragraph rewritten (stop-first is no longer the
  only way); `book/observability-and-accounting.md`: the omission note's story updated.
- `crates/vadis-core/src/store.rs`: `Query::EventsSince` + doc.
- `crates/vadis-store/src/lib.rs`: the SQL and its unit tests.
- `crates/vadis-proxy/src/state_read.rs` (new): the endpoint's body.
- `crates/vadis-cli/src/lib.rs`: route assembly (`guarded_state_route`), `GuardState` word.
- `crates/vadis-cli/src/stats.rs`: the fallback client.
- `tests/conformance/tests/conf_106_live_state_surface.rs` (new).

## 8. Dated ruling note (2026-10-07, `R72-5` — append-only)

**§3's property table and §6's summary state the status set as `{200, 401}`; the authoritative set
is spec §4.18's `{200, 400, 401}`.** The row omits the `400` **window arm** (an absent,
out-of-bounds or unparsable `window`), which spec §4.18 registers as the endpoint's refusal,
`design/DESIGN.md` §12.24 records, and `CONF-106` limb (b) asserts. By §3's own pointer — spec §4.18
is the full text of the contract, this ADR's table is its summary — the row is **superseded in
precision** by §4.18's set; the summary's `{200, 401}` remains correct about the **guard** half it
was describing. No sentence of §3 or §6 is rewritten (append-only). Found by `R72-4`'s audit
(finding F1); recorded here by `R72-5` rather than by editing the table.
