# ADR-010 — the event log is the single source of truth for state; write ahead, then execute

- Status: accepted
- Date: 2026-09-19
- Related: ADR-005 (the trace is the only product↔analysis-loop interface), ADR-009 (storage boundary and tiered durability), ADR-002 (keyed config diff), ADR-004 (v0.1 has no server-side session state); spec §4.5 (state) / §6 (observation) / §8 (degradation); DESIGN §8

## Background

Round 2 makes vadis stateful in the durable sense: a sticky session binding, the cache ledger and the
quota counters must survive a restart, and a crash must not turn "what the gateway did" into a guess under
the operator's feet. The previous design treated state as *derived memory* — the counters were the truth
and any history was implicit — which fails in two ways:

1. **Derived state cannot be audited.** "Why does this plan show 41,000 tokens used?" has no answer beyond
   the current number: a bug that decremented twice is indistinguishable from a real charge.
2. **The crash window has no representation.** Mid-request the process knows it *intended* to call upstream;
   the counters know nothing about it. After a restart the old design could only silently forget (an
   undercount) or blindly retry (a possible double charge upstream).

It is worth stating the context that makes this asymmetry expensive: clients are stateless and resend the
whole conversation (ADR-004), so the gateway is the only place in the system where a request's lifecycle is
recorded at all. If vadis loses an intent, nobody else kept a copy.

## Decision

1. **Every state transition is an event, and `events` is the truth.** Nothing else is authoritative. Every
   queryable view — the sticky table, the cache ledger, the quota counters, any report derived from state —
   is a **projection** that may be dropped and rebuilt from `events` (ADR-009 items 4–5). When a projection
   disagrees with the log, the projection is buggy; it is not a second opinion.
2. **Event vocabulary (the v0.1 slice).** One row per event: `event_id` (AUTOINCREMENT — the ordering
   anchor), `ts_us`, `kind`, `request_id`, `session?`, `schema_version`, `payload` (JSON), `body_hash?`,
   `trace_ref?`.

   | Event | Written at | Durability | Payload essentials |
   |---|---|---|---|
   | `request.received` | after the inbound body is read, before any decision | FULL | protocol in/out, client, session (or null), `body_hash`, `turn_index` |
   | `decision.made` | after selection and the guard chain | NORMAL | provider, model, `selection_source`, plugin chain, `decision_ms` |
   | `transform.applied` | per transform step that changed the payload | NORMAL | plugin, added/saved tokens, `cache_impact`, verdict |
   | `session.bound` | when the sticky binding is created or moved | FULL | session key, provider, model, ttl |
   | `upstream.submitted` | **before** the upstream attempt starts (intent) | **FULL** | route, attempt index, `attempt_id`, `body_hash` of the outbound bytes |
   | `upstream.responded` | when the response head/body completes (or fails) | FULL | status, raw `usage`, latency |
   | `failover.triggered` | on every route switch | FULL | reason, from → to, the re-prefill cost |
   | `cost.computed` | once `usage` is known | FULL | the five-tier cost, `quota_after` |
   | `quota.charged` | with `cost.computed`, before the response is released | FULL | provider, plan, tokens charged, remaining |
   | `plugin.loaded` / `plugin.unloaded` | at load/unload edges | NORMAL | plugin id, kind, tier, effective config digest |
   | `config.applied` | whenever a config is validated and applied | FULL | config digest, changed keys (keyed diff, ADR-002) |

3. **Write ahead, then execute.** The intent event commits *before* the effect it authorizes:
   `upstream.submitted` is committed before the HTTP attempt begins, and `cost.computed` / `quota.charged`
   are committed before the response is released to the client. If the intent write fails, the effect does
   not happen — the request is rejected with nothing sent upstream (ADR-009 item 8). This deliberately
   inverts the usual "log the outcome after it succeeded" habit, because the two errors are not symmetric:
   a missing response is recoverable ambiguity, while an unrecorded upstream call is an unaccountable charge.
4. **Crash-window semantics: `unknown_outcome`.** An `upstream.submitted` without a matching
   `upstream.responded` is a **known unknown**: the gateway cannot tell whether the upstream answered,
   billed, or neither. The rules are:
   - the request is recorded and reported as `unknown_outcome` (the intent row plus the restart marker — the
     state truth keeps the ambiguity instead of resolving it by guessing);
   - **the quota is not charged for it** (and therefore never double-charged): the upstream has very likely
     already billed it, and a conservative local re-charge would silently eat the user's plan, which is
     worse than a locally visible undercount;
   - its cost stays **uncomputed**: there is no `usage` to read, and inventing a number is forbidden by the
     accounting convention (spec §7);
   - reconciliation is left to the operator, against the provider's own bill and the measured captures.
     `vadis stats` reports the number of `unknown_outcome` requests in the window, so the ambiguity is
     visible rather than absorbed into a total.
5. **Honest boundary (unverified, and it may not be fixable by cleverness).** Whether an OpenAI-compatible
   upstream honours an idempotency key is **not verified** in v0.1, and no client-supplied request id is
   guaranteed to be deduplicating. If the upstream does not deduplicate, then "was this request billed?" is
   **undecidable at the protocol layer**: no local bookkeeping can answer it, and a client retry — or a
   gateway retry after a crash — can legitimately be billed twice. This is an inherent cost of the design,
   and the mitigation available in v0.1 is *visibility* (item 4) plus the operator's provider-side bill. If
   a provider later exposes an idempotency key or a usage-export API, the crash window narrows to exactly
   what that API supports, and this ADR gets superseded rather than patched.
6. **Explicit exclusion: request and response bodies never enter the event log** (ADR-009 item 3). The log
   carries `body_hash` plus `trace_ref`, so a payload is checkable and joinable when it was captured, and
   honestly unavailable when it was not.
7. **Boundary with ADR-005: the event log is not a second observation channel.** It is serving-time
   internal state. The analysis loop never reads the store; the product → analysis-loop channel remains **trace JSONL
   only**. Anything the loop must observe has to appear in the trace — which is why the trace's identity
   group gains `event_id` (spec §6, DESIGN §12.6), making the join key `request_id` + `event_id` explicit.
   Research that wants state over time reads the trace, never the database.

## Alternatives considered

| Alternative | Why rejected |
|---|---|
| the counters are the truth, with a periodic snapshot for hand-over (the previous DESIGN §8) | a counter change has no audit trail, and the crash window is lost by construction |
| log **after** the effect succeeded (the ordinary habit) | the crash window disappears (there is no intent row to find), and an unrecorded upstream call is an unaccountable charge |
| a separate audit log beside authoritative counters | two writers over one fact: the audit log and the counter can disagree, and then nothing says which is right |
| retry the upstream call after a crash to resolve the unknown | it converts a possible double bill into a probable one; retries belong to the client, which at least knows its own idempotency story |
| derive state from the trace instead of keeping a store | the trace is an **output** (written at request end, retained per operator whim, and the analysis channel of ADR-005); the serving path reading its own output would invert the pipeline and cross the observation boundary of AGENTS constraint 3. It also cannot be write-ahead, which is the whole point |

## Rationale

- This is not event sourcing for its own sake. It is the cheapest way to get four things at once:
  auditability of every counter change, rebuildable derived views, an explicit crash window, and one write
  path that the sticky table and the quota counter are both projections of.
- Write-ahead is what makes the crash window *representable*. Without the intent row, "the process died
  mid-request" and "the process was idle" look identical in the data.
- "Do not re-charge" is chosen for a stated reason rather than by taste: in a subscription-plan world the
  two errors are not symmetric — a silent double charge eats the plan, while the undercount is visible in
  the `unknown_outcome` report and reconciles against the provider's bill.
- Keeping the log internal keeps ADR-005's claim *structural* ("research does not enter the serving path,
  and there is no second interface") instead of turning it into a discipline requirement.

## Consequences

- The write path is now **ordered**, so every state-changing call site is reviewed for one question: does
  the intent row precede the effect? That invariant is testable on a fixed trace ("the last event before an
  upstream call is `upstream.submitted`") and is the natural next conformance case; DESIGN §12.8's ID space
  is additive, and this ADR deliberately does not allocate an ID for it.
- Every projection needs a rebuild path, and the cheapest correctness oracle in the whole state layer is a
  test that "incremental projection == rebuild from `events`".
- The trace gains `event_id` (spec §6 identity / DESIGN §12.6): a `DecisionRecord` now names its anchor in
  the log, so the analysis record and the state truth join on `request_id` + `event_id` rather than on
  timestamps.
- Money keeps a single authority: cost answers come from the trace (spec §6/§7, ADR-005). `cost.computed`
  and `quota.charged` rows are *state* events — what was charged to a plan — and `vadis replay` remains the
  authority for money. The two must not be presented as interchangeable numbers.
- A vocabulary change must keep old rows readable: `events.schema_version` is per row, rows are never
  rewritten, and readers upcast (ADR-009 item 7).
- Operator tooling for the projections and the `unknown_outcome` report (a `vadis state`-style surface)
  becomes necessary, but it is a **separate change** with its own doc update; this ADR does not add it to
  the CLI surface.

## Publication note (2026-10-03, R62-2)

The `R<n>` labels and finding ids in this document name iterations of the project's own
private analysis loop — a loop that is not part of this repository, so no label here is
resolvable by a reader of it; they are kept as the provenance of the decision. This
publication pass removed only the dead-pointer class: every reference into that loop's
working tree (its file paths and round-record names, its state record, charter, execution
model and replay contract, its scripts and module names, and the kanban card ids), each
replaced by the neutral phrase the sentence needs. Nothing else moved — no figure,
threshold, `§`/`ADR`/`CONF` id, code sample or contract sentence.
