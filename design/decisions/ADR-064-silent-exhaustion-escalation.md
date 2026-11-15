# ADR-064 — silent exhaustion escalates: a route-scoped `stream_silence` streak moves a plan family (K = 2 within W = 5 m), and nothing outside a plan family moves at all

- Status: **accepted, contract only** (`R76-0`, 2026-11-14). The contract below is frozen; the code that
  makes it real lands in a **separate** change (`R76-1`), and §6 says exactly what that change owes. Until it
  lands, `plan_policy[].silent_escalation` is a key the loader **refuses**
  (`deny_unknown_fields`), no `plan.switched` row carries `reason: "primary_silent"`, `/health`'s `plan`
  block has no `escalation` member, and `CONF-121`/`CONF-122` are parked `#[ignore]`d.
- Kind: **a contract change with no product byte.** It amends two documents (`docs/spec.md`,
  `design/DESIGN.md`), writes a dated note on each of `ADR-011`, `ADR-014` and `ADR-056`, allocates two
  conformance IDs, and writes their two case files **parked**. Nothing under `crates/`, `tests/` outside
  those two files, `.github/`, `config.example.yaml`, `providers.example.yaml`, `book/` or `README.md` is
  touched by this card.
- Authority: the owner's ruling of **2026-11-14** — 「好」, answering the analysis that recommended this
  mechanism (§1.1 quotes it and names the analysis it authorises: the numbered summary in the round's session
  record, not a chat transcript). The evidence is a **two-week trace audit of the owner's live deployment**
  (2026-10-04 → 2026-11-14, **15,040** `coding-fast` requests); every figure quoted below is **read out of
  that audit**, never estimated, and §2.9 says what the round may not claim about them.
- Amends: `docs/spec.md` **§4** (the schema sketch gains the key and its three commented members), **§4.6**
  (one key row, three refusal rows, and rule 3's mover sentence — the
  billing mover keeps its half and §4.21 becomes the only other mover), **§4.11** (one paragraph: the key is
  not askable), **§6** (`plan_switch.reason`'s vocabulary and its producer table), **§9.1** (`/health`'s
  `plan` block gains `escalation`), and the new **§4.21**; `design/DESIGN.md` **§12.6** (one
  bullet: the record's `plan_switch` arm), **§12.8** (heading, two rows, the allocation paragraph), **§12.9**
  (the round's register line), **§12.10.4** (store DDL version **4**: one table, one column, one rebuild),
  **§12.10.8** (the escalation's landing); dated notes on **ADR-011**, **ADR-014**, **ADR-056**.
- Does **not** amend: `§4.20` (ADR-056's section, including §4.20.5's K = 1 row trigger — §2.2.3 says why),
  `§4.16`/`§12.21` (`/metrics` gains no series, §2.8), ADR-011's class table or `demotes_provider()`,
  ADR-014's probe predicate, ADR-056's advisory-row trigger or TTL, ADR-063's ordering key.
- Cases: **`CONF-121`** (the escalation: the K-th fact within W moves the family, the reason word, the
  record, the state, the non-retry of the K-th request, the probe interplay and the return-trip reset) and
  **`CONF-122`** (the over-trigger controls: one fact never escalates; facts outside W never accumulate; a
  non-counted cause never counts; a metered route's facts never reach a plan route's counter; `block` never
  escalates; nothing account-level happens outside a plan family; a reload does not zero the streak). Both
  files land with this card, parked `#[ignore = "CONF-12N: depends on R76-1"]` (the `CONF-20…25` rule).
- Related: `AGENTS.md` constraints **1** (no relayed byte moves — §2.4.6), **2** (determinism: the streak is
  a fold over recorded facts, never a clock the content does not have — §2.3), **3** (the observation
  boundary: the counter is a store projection, never a trace read — §2.3.4), **4** (no unverified savings —
  §2.9), **8** (docs before code — §6.3 names the `book/` sentence the *implementing* round owes), **9** (the
  measurement is not part of the search space — §2.9 and §5); **ADR-005** (the trace is the interface),
  **ADR-009** (one store, rebuildable projections), **ADR-010** (the event log is state truth),
  **ADR-011** (the taxonomy, `demotes_provider()`, item 6's not-billed rule), **ADR-014** (plan-first routing,
  the probe, the session-boundary rule), **ADR-024** (a skip narrates `failover_from`, never
  `failover.triggered`), **ADR-049** (the plan tier and the metered ranking), **ADR-056** (§4.20: the silence
  row, its advisory rule, its §5.4 separation), **ADR-058** (`measured` is not `verified`), **ADR-060** (no
  retry after the head; `stream_end`'s four causes), **ADR-063** (one order for the walk).

---

## 1. Why

### 1.1 The owner's sentence, and the measurement behind it

The owner's framing, recorded verbatim (2026-11-14):

> **连续 2 次 `stream_silence`（head 超时或 stream 截断）→ 升格 spill**

*Two consecutive `stream_silence` facts — a head timeout or a stream truncation — escalate to a spill.*

That is one sentence, and this ADR's whole job is to make it a contract with no unstated choices left in it:
**what a fact is, how two are consecutive, what "escalate" does, whose account moves, who may not move, and
what must not happen afterwards.** The analysis it authorises is the round's numbered summary (the mechanism
recommended in it), not the transcript of the conversation that produced it.

The finding the round exists for is measured, and it is a **vocabulary** defect rather than a routing one:

| # | Fact | Evidence (trace-only, the audited fortnight) |
|---|---|---|
| **F1** | The plan account's exhaustion **never surfaces as `403 quota_exhausted`** — the only class ADR-011/ADR-014 let move an account. | **0** `403` quota-exhausted rows on the plan route in 15,040 requests. |
| **F2** | What the upstream does instead, shape 1: the head arrives and the body dies mid-stream. | **113** `stream_truncated` aborts; **97** of them on 10-06→10-08 (≈**32.3**/day, ≈**1.35**/hour). |
| **F3** | Shape 2: the head never arrives at all. | **4** `timeout`-class `502`s on 11-01/11-07, each with `upstream_ms` pinned at exactly **120,002–120,003 ms** = `server.upstream_attempt_timeout`; each an abort. |
| **F4** | The walk itself is healthy — the trigger vocabulary is incomplete. | The **5** clean `429`s the fortnight saw (metered flash route) all failed over correctly; 10-27's `connect_failure` spilled plan→metered correctly. |
| **F5** | So the family stayed on an exhausted plan route while `on_primary_exhausted: spill` — the mechanism the operator configured — never fired. | Every F2/F3 request ended as a `2xx`-with-`errors[]` truncation or a `502`, with the family's `plan_state` still `primary`; ADR-056's own reading of the same shape: those truncations sat at `overhead_ms` median **120,010 ms** against an `upstream_ms` median of **5,163 ms** (106 records, the earlier window) — **≈3.5 h** of pure waiting, of which only 43 of 83 retries met a clean `200`. |

At the fortnight's rate a truncation is **0.75 %** of requests, but the cost is not the rate: each one is a
client that waited two minutes and then retried, and the retry went back to the same silent route because
nothing had moved the family. The instrument that would have moved it exists (`on_primary_exhausted: spill`)
and reads one signal (a `403`); the upstream does not send that signal.

### 1.2 Three facts about today's code that make the owner's sentence unimplementable as written

The sentence says *"two consecutive `stream_silence`"*. Read against the shipped tree, three things are true
and each blocks a naive reading:

1. **A `stream_silence` row is written for one cause out of four.** `record_classified`
   (`crates/vadis-proxy/src/stream_forward.rs:2329-2368`) writes the route-scoped advisory row **iff**
   `trunc.stream_end == "idle_timeout"`. ADR-060's other three causes — `upstream_ended_early`,
   `request_timeout`, `client_closed` — write the classification row and **no** cooldown row, deliberately
   (spec §4.20.5: a graceful early close is not a stall, our own deadline is not the route's fault, and the
   client leaving is not the route's fault at all). So "every row the relay's truncation arm writes" is **not**
   the set of facts the measurement needs.
2. **A head that never arrives writes no row of either kind.** The streaming head arm
   (`stream_forward.rs:1901-1930`, `OpenHead::UnknownOutcome`) returns a `502` with
   `details {stage: "unknown_outcome", error_class: "timeout", stream: true, retry_refused: "billed"}` and
   writes **no `error.classified` event** — `record_failure_trace` → `finish_failure` produces the trace line
   only (`crates/vadis-proxy/src/forward.rs:881`), and the only two `EventKind::ErrorClassified` writers are
   the buffered classification path (`forward.rs:2857`) and the truncation arm (`stream_forward.rs:2357`).
   **The log cannot answer "how many head timeouts hit this route".** That is F3's 4 rows, and it is the
   *first* half of the owner's sentence.
3. **Rows cannot be counted, even for the cause that writes them.** `provider_cooldown` is one row per
   `(scope, provider, model)` — an `ON CONFLICT … DO UPDATE` upsert (`crates/vadis-store/src/lib.rs:568`).
   Two stalls on one route inside the row's 60 s TTL therefore leave **one** row, not two. The card's own
   recommendation (60 s) makes this sharper, and W = 5 m (§2.1) makes it structural: the first row has
   certainly expired when the second fact lands. **A row count is not a fact count.**

Fact 1 says which predicate is wrong; fact 2 says the log is missing a writer; fact 3 says the *artefact* is
wrong. The ruling therefore defines the fact, the counter and the move separately.

### 1.3 Why a streak is admissible here and was refused in ADR-056

ADR-056 §2.5.1 ruled **K = 1** for the *cooldown row* and gave the reason:

> *"A 'consecutive K' rule needs a streak counter, and a counter is **state the event log cannot rebuild**:
> `rebuild_cooldown` reconstructs the projection from the `demotion` objects alone (CONF-21 asserts
> projection == rebuild row by row), so a streak held anywhere but in an event payload would make the two
> paths disagree by construction."*

That reasoning is **sound and unchanged**, and it is not the reasoning this ADR relies on — because the
objection is to state the log cannot rebuild, and this streak **is** rebuildable. Every counted fact is a
classification row with a `ts_us`; the route is on the row (§2.2.2 makes it explicit for the one writer that
lacked it); the fold in §2.3.3 reconstructs the streak exactly, from `events` alone, with no config and no
clock. So the difference between ADR-056's refusal and this ADR's acceptance is not a change of mind about
streaks: **it is the counter's source**. The cooldown row had no source, so it could not have one; the
escalation counts rows that already exist (plus the one the log was missing), so it can.

Two further differences, stated so the two K's are not read as rivals:

| | ADR-056's row (spec §4.20.5) | ADR-064's escalation (§4.21) |
|---|---|---|
| K | **1** — one stall | **2** — two facts within W |
| Scope | **route** (`provider_cooldown.scope = "route"`) | **route**, read against a family (§2.4) |
| What it does | moves the *selection* — an advisory position (ADR-063 term 1) | moves the **account** of a plan family that declared `spill` |
| TTL / window | 60 s constant, on the row | 5 m, on the **run** (§2.1) |
| Artefact | `provider_cooldown` (one upsert row) | `silence_streak` (DDL v4, §2.3) |
| Rebuild | from each row's own `demotion` object | from the fold of counted classification rows |
| Why that K | one stall is not an account fact; anti-flap is bought by the short TTL | one row is not an account fact **and** one row is not evidence of a *pattern*; two in five minutes is |

### 1.4 Why this is a contract, and why now

The sentence has four kinds of choice in it — a threshold, a window, an artefact and a move — and each would
be implemented differently by two implementers reading only the sentence. The register's own splits are the
answer: the contract is reviewed alone (`R76-0`), the code lands against a frozen contract (`R76-1`), and the
cases are parked in between so the suite is never red.

---

## 2. The ruling

### 2.1 The primitive: a route-scoped **silence streak**, K = 2, W = 5 m

| Property | Ruling |
|---|---|
| Key | **`(provider, model)`** — the route, in the wire's own terms: the provider entry's name and the route's **native model id** (never an alias, never `""`), exactly the pair ADR-056 §2.5's row is keyed by |
| Threshold | **K = 2** — the owner's number, and the contract's |
| Window | **W = 5 m (300 s)** — this ADR's, justified below; reloadable, §2.7 |
| Run definition | a **run** starts at the first counted fact and lasts W: a fact joins the current run iff `fact.ts_us − run.first_us <= W`; a fact that arrives later starts a new run at count 1 and becomes its `first_us` |
| Escalate when | the run's count reaches **K** on a fact that lands on a family's **active plan route** (§2.4.1) |
| Read predicate | a run is **live** iff `now − first_us <= W`; an expired run contributes **0** |
| What it is not | not a cooldown, not a demotion, not a health verdict, not an error class, not a row the walk reads for selection |

**§2.1.1 Why W = 5 m and not the card's suggested 60 s.** The card left the value to me and asked for a
justification, so here is the arithmetic, and it is the reason the recommendation is not taken. The audit's
own retry figure (ADR-056 §1.1 F1, the earlier 13-day window) is that **83 of 106** truncations were retried
**within five minutes**. A second fact is, in the dominant case, *the client's own retry* — and a retry that
arrives at 90 s or 4 m is a second fact about the same route. **W = 60 s would not reach the very incidents
this round exists for.** The independence model below (an `inferred` comparison, never a forecast) makes the
same point numerically, from the audited counts alone:

| W | expected runs with ≥2 facts, the 3-day exhaustion window (10-06→10-08, 97 facts ≈ 1.35/h) | expected runs with ≥2 facts, the whole 42-day window (113 facts spread uniformly) |
|---|---|---|
| 60 s | **≈1.1** | ≈0.11 |
| **5 m** | **≈5.1** | **≈0.52** |
| 15 m | ≈13.1 | ≈1.55 |

The middle row is the ruling: on the exhaustion window it fires about five times — enough to move a family
that is genuinely exhausted — while a *healthy* fortnight of evenly-spread truncations produces about **half**
an escalation, i.e. roughly one false spill per six weeks, each costing one cooldown of metered spend plus the
re-prefill a move always costs (ADR-014 item 5). 15 m triples the false-positive expectation for a quarter more
detection; 60 s detects almost nothing. Both extremes are rejected in §7.

**Two structural facts sit under those numbers, and they bracket W from both sides.** (i) A counted S1 fact
*is* the silence: its `ts_us` is stamped when the gap bound finally trips, so a second fact produced by the
client's own retry arrives one silence-to-verdict interval after the first — at the shipped
`server.stream_idle_timeout` of 30 s, a W below ~30 s could never hold two facts at all, i.e. the mechanism
would be dead by construction. (ii) A useful W must reach the *retry latency*, and the audit's own figure is
that 83 of 106 truncations were retried within five minutes — so the useful band is roughly
"longer than one silence, shorter than an incident", and 5 m sits inside it while 60 s covers only the
immediate-retry half of the population.

**§2.1.2 Why the run is anchored, not sliding.** A sliding window (count the facts in `(now − W, now]`) needs
every fact's timestamp or a per-read recomputation, and on a route that stalls at a steady drip it can be held
open indefinitely — one fact per 0.9 W keeps the count at ≥2 forever and the family spills in a loop. An
anchored run is **self-limiting**: it can be entered at most once per W, it is a fold over two integers, and
its rebuild is exact. The cost, stated: three facts in 6 m (at 0 m, 3 m, 6 m) count 2 in the first run and 1 in
the second at W = 5 m, so a slow triple escalates once and not twice — the conservative direction for a
mechanism whose false positive spends money.

**§2.1.3 Why K = 2 and not K = 1.** K = 1 is already spent: it is ADR-056's row, and §2.5.3 of that ADR is
explicit that one stall is a *selection* fact and not an account fact — a single hiccup may not take the free
account away. The owner's sentence says the same thing ("连续 2 次"). A config value below 2 is therefore a
**load refusal**, not a knob that silently recreates K = 1 (§2.7.2).

### 2.2 Which facts count — the exact predicate

**§2.2.1 The two counted facts.** One predicate, applied to the classification of an attempt, and it admits
exactly two shapes — the two the owner's sentence names. It is stated in **both** spellings, because the record
nests the class word under `details.error_class` while the classification event carries it as `reason`
(`record_classified`'s existing shape), and the fold in §2.3.3 reads the event's:

| # | Counted fact | The record's spelling (`errors[].details`) | The event's spelling (`error.classified` payload) | Where it comes from today |
|---|---|---|---|---|
| **S1** | **a stream that went silent mid-body** | `error_class == "stream_truncated"` ∧ `stream_end == "idle_timeout"` | `reason == "stream_truncated"` ∧ `stream_end == "idle_timeout"` ∧ `stream == true` | the relay's truncation arm; this is also the fact that writes ADR-056's advisory row |
| **S2** | **a head that never arrived** | `stream == true` ∧ `stage == "unknown_outcome"` ∧ `error_class == "timeout"` | `stream == true` ∧ `stage == "unknown_outcome"` ∧ `reason == "timeout"` | the streaming head arm (the outbound request was fully written; the head missed `server.upstream_attempt_timeout` or the whole-request bound) |

One fact, two spellings, one writer each: an arm writes the record and the row from a single classification, so
the two spellings cannot disagree about the same attempt — and §2.2.2 makes both spellings exist for S2.
Both are facts about the **upstream being silent** — one after the head, one before it. F2 and F3 of §1.1 are
exactly these two populations, and the contract's whole value is that both feed one counter.

**§2.2.2 Every counted fact must be writable, and S2's writer is owed (§1.2 fact 2).** The streak is rebuilt
by folding classification rows, so a counted fact must exist as one. S1 already does. S2 does not: the
streaming head arm writes a trace line and nothing else. `R76-1` therefore **gives it the row ADR-011 item 8
already requires of every classification** — `error.classified` with `reason: "timeout"`, `action: "abort"`,
`matched: false`, `stream: true`, `stage: "unknown_outcome"` — and, on **both** writers, a `route`
member `{provider, model}` (the same pair ADR-056 §2.5's `demotion` object carries; one value, two readers).
That member is what makes the fold in §2.3.3 possible without a join to `upstream.submitted`, and it is
**additive on an events payload**, so no `EVENT_SCHEMA_VERSION` move and no existing reader moves. Registered
as `R76-0-F1`: before this round, the log could not answer *"how many head timeouts hit route R"*.

*Why a `route` member and not a read of S1's existing `demotion.provider` / `demotion.model`:* because the
**fold must be one predicate over one member**, and only S1 carries a `demotion` object (S2 writes no
demotion — it moves nothing on its own). The pair is therefore stated once per counted fact, in the same two
fields, on both causes; the `demotion` object keeps its own six members untouched for `rebuild_cooldown`
(ADR-056 §6.2). That is the sanctioned duplication this ledger already has between a payload's object and the
projection written from it — one value, two readers — not two names for one fact drifting apart.

**§2.2.3 What does not count, and why each exclusion is a ruling rather than an omission.**

| Not counted | Why |
|---|---|
| `stream_end == "upstream_ended_early"` | ADR-056 §2.5's own reason, unchanged: a graceful early close is the shape a provider's *answering* route takes, and punishing it would make the breaker fire on a route that is serving. It also writes no advisory row today, and §4.20.5's trigger is untouched (§2.5.3) |
| `stream_end == "request_timeout"` | our own outer bound, not the route's behaviour — ADR-056 §2.5 says so and this ADR keeps it |
| `stream_end == "client_closed"` | ADR-060 D1: the client left; the route was healthy. Counting it would let a flaky client spill a family |
| `connect_failure` | a fast refusal is a different fact (the TLS/TCP path never opened), already failover-eligible, and `details.stream` is absent on that arm, so the S2 predicate cannot match it either |
| any **buffered** (non-streaming) failure | the escalation is a *stream silence* instrument; the predicate demands `details.stream == true`, and a buffered timeout — even the mid-body stall CONF-105 rigs — is out of it. A buffered deployment therefore never escalates, which is correct: it also never writes the advisory row (spec §4.20.5), and the two instruments keep one domain |
| a fact on the **metered overflow** route | it is counted on **that route's** streak, and no family's plan tier can contain that route by construction (spec §4.6: a `plan_policy`'s `primary` provider must be `account: coding_plan`, its `overflow`'s `account: api`, and ADR-049's tier is the family tag's `coding_plan` routes only). No special case is coded, and none is needed: the keys cannot collide. The plan route's counter is therefore untouched by overflow traffic — the card's requirement, satisfied structurally |
| a `404`, `401`, `429`, `5xx` or any other classified failure | not silence. The escalation's evidence is one kind of fact only |

**§2.2.4 A fact that lands after the escalation does not restart K, and does not re-move.** The run keeps
counting (it is a fold over facts, and facts are facts), but the move is gated on the family's **state** (§2.4.1):
once the family is off its plan route, no fact on that route can escalate it again — including the facts a
recovery probe writes. `R76-1` may not add a "reset the run on escalation" write: the run's own W and the
return-trip delete (§2.5.4) are what bound it, and a reset on escalation would make the *next* single stall a
K-th fact again (§2.5.4's loop).

### 2.3 The counter's home

**§2.3.1 One table, one owner, one write per counted fact.** Store **DDL version 4**, additive (ADR-009 item 7:
no existing row is rewritten):

```sql
-- ADR-064; an additive migration through `const MIGRATIONS` (DDL v3 -> v4).
CREATE TABLE silence_streak (
    provider   TEXT    NOT NULL,      -- the route's provider entry name
    model      TEXT    NOT NULL,      -- the route's NATIVE model id (never an alias, never '')
    count      INTEGER NOT NULL,      -- facts in the current run; >= 1
    first_us   INTEGER NOT NULL,      -- the run's anchor: the first counted fact's ts_us
    last_us    INTEGER NOT NULL,      -- the latest counted fact's ts_us
    last_event INTEGER NOT NULL,      -- the classification row that wrote this state
    PRIMARY KEY (provider, model)
);
```

The **owner is the fact writer** — the arm that classifies the silence — and there are exactly two, sharing
one function (`record_silence_fact`): the streaming relay's truncation arm (S1) and the streaming head arm
(S2). Neither invents a second notion of "silent"; both hand the same `(provider, model, ts_us, event_id)`
to one upsert, so the incremental path and the rebuild path cannot disagree (CONF-21's idiom, applied to a new
projection).

**§2.3.2 Why its own artefact and not a `provider_cooldown` row count.** §1.2 fact 3: the cooldown row is an
upsert keyed by the route, so two facts inside one TTL leave one row — and at W = 5 m the first row has
expired before the second fact lands in most pairs. Counting rows would therefore be *wrong*, not merely
fragile. A separate table also keeps the two instruments' lifecycles apart: the row is 60 s and advisory
(selection), the streak is 5 m and account-moving (routing intent).

**§2.3.3 The rebuild, and why ADR-056's objection does not reach it.** `rebuild_silence_streak` scans
`error.classified` rows in `event_id` order and keeps, per `(provider, model)`, the fold of §2.1: a row that
satisfies §2.2.1 and arrives within W of the run's `first_us` increments; otherwise it starts a run. It reads
**only** `ts_us`, the payload's `route` member and the predicate's own keys — no config, no clock, no trace.
A projection with a rebuild path is ADR-009 item 5's requirement, and this one has one, which is exactly the
property ADR-056 §2.5.1 found missing in the counter it refused.

**§2.3.4 Why not in memory, and why not the trace.** In memory: a restart would zero it, and the escalation's
own subject — a family that has been stalling for a day — outlives processes; the store is where the rest of
the routing intent already lives (`plan_state`, `provider_cooldown`). The trace: **forbidden**, and not for
taste — AGENTS constraint 3 makes the trace the *output* channel; a serving-path read of the analysis channel
would invert it. The event log (via its projection) is the only legitimate source, and it is the one used.

### 2.4 The move

**§2.4.1 When, and on whom.** The escalation is evaluated **once, at the instant a counted fact is written**,
by the same writer. It fires iff **all** of:

1. the fact's run count has reached **K** (§2.1);
2. the fact's route is the **active route** of a family declared under `plan_policy`/`plan_policies`
   (`plan_state.route`, or the policy's `primary` when the family has no row — spec §9.1's own reading);
3. that active route is **in the plan tier** — the family is not already spilled (if it is, the answer is
   "nothing to move", §2.2.4);
4. the policy's `on_primary_exhausted` is **`spill`** (§2.4.4 for `block`);
5. `silent_escalation.enabled` is true (§2.7).

**§2.4.2 What it does: the same move the `403` path performs, through the same writer.** Not a new mechanism,
not a second router: `R76-1` calls the existing account-move writer (`record_plan_switch`, DESIGN §12.10.8)
with **one new argument, the cause word**, and every existing consequence follows unchanged — one
`plan.switched` row at durability **FULL** (ADR-014 item 8: it changes what every subsequent request in the
family is billed), one `session.bound` re-point per live binding of the abandoned route (DESIGN §12.10.5 note
R7 — so the log says *these sessions moved*, not only *the family moved*), and the `plan_state` projection
written from that same row. The destination is whatever that writer already resolves — under a singleton plan
tier (every config written before ADR-049, and the shipped examples) it is the policy's `overflow` route, i.e.
exactly the owner's *升格 spill*; under a tier of several plans it is the next plan member, and the family stays
in-plan. **This ADR adds a cause word, not a second definition of "where".**

**§2.4.3 The reason word: `primary_silent` — the fifth `plan_switch.reason`, and the only cause-shaped one.**
The existing four encode *where the family went* (`primary_exhausted` left the tier, `plan_exhausted` stayed
in-plan, `primary_recovered` came back, `primary_cooling_down` was displaced without a state move). Reusing
`primary_exhausted` for silence would be a **false statement**: no upstream said "exhausted", and silence's
entire problem is that no such word arrived. So the vocabulary gains one **cause** word, and a reader learns
where the family went from `from`/`to` and from the state it lands in:

- `plan.switched.reason = "primary_silent"`, `probe: false`, and — because the evidence must ride the row that
  was decided on it — one additive member **`silence: {count, threshold, window_us}`** (the run's count at the
  decision, the K in force, the W in force). One row, the transition, and the evidence that caused it.
- `result.plan_switch.reason` on the requests **displaced by that state** reads **the state's cause word**
  (§2.8.2), so the trace and the event log agree about *why the family is spending money* instead of the trace
  flattening both causes to `primary_exhausted`.
- The producer table (spec §6) gains the row: *produced when a family's active plan route reached K counted
  silence facts within W and the policy is `spill`*.

**§2.4.4 `spill` only. `block` does not escalate, and that is a ruling.** Under `block` the operator has
declared *fail rather than spend*; there is nothing to escalate **to**, and moving the state would convert a
silent plan into a `429 quota_exceeded` refusal whose message would have to claim an exhaustion nobody
declared (§8's vocabulary has no word for "the plan went quiet"). So a `block` family keeps today's behaviour
exactly — the attempt fails the way it failed — and the gap is registered (`R76-0-F2`): an operator who wants
silence to *refuse* rather than *stall* has no instrument in this round.

**§2.4.5 The K-th request is not retried.** The request whose attempt produced the K-th fact is the request
that **failed**: its body was already partially on the wire (S1) or its bytes were fully written and no head
came (S2). ADR-011 item 6 row 3 and ADR-060 §8's head rule forbid a retry in both shapes — the upstream may
have billed it, and a relayed byte cannot be un-relayed. So the escalation moves the family for **requests
that begin after it**, and the K-th request ends exactly as its predecessors did. This is the one place the
silence path differs from the `403` path, which retries the same request on the overflow route (ADR-014 item
5) because a `403` is a *refusal* with no bytes and no bill. Stated here so an implementer does not "complete"
the mechanism by adding a retry.

**§2.4.6 Nothing about the relayed bytes moves.** The escalation is decided *after* the body is over; the
per-chunk write-through, the ordering, the "never fabricate a terminal" rule and the record's own shape are
untouched (AGENTS constraint 1). A request that is displaced by the state is an ordinary request on the
overflow route — same bytes, one attempt, one record.

### 2.5 The probe, the two clocks, and the loop that must not happen

**§2.5.1 Probe facts count.** A probe is admitted at a session boundary (ADR-014 item 3, unchanged) and
attempts the family's plan route; if the plan is still silent, the probe itself writes a counted fact and
advances the run. That is correct and wanted: a probe that stalls is evidence. It cannot loop, because §2.2.4
gates the move on the state (already spilled ⇒ nothing to move) and because a probe is admitted at most once
per cooldown **and** once per session **and** not before the plan's declared window boundary (ADR-014 item 10).

**§2.5.2 The two clocks, in one sentence.** **W (5 m) is the escalation's own window over silence facts and
the `cooldown` (15 m default) is ADR-014's interval between the move and the first admitted probe: they are
different clocks, they never read each other, and neither can open or close the other's gate** — a cooldown
expiry admits a probe whatever the streak says, and an escalation neither shortens nor extends the probe
deadline (it *moves* it only in the sense that a new `plan.switched` row makes `since` later, which is
ADR-014's own arithmetic, not a new rule).

**§2.5.3 ADR-056's advisory row is untouched, and the asymmetry is deliberate.** A counted S1 fact writes the
`stream_silence` advisory row exactly as it does today, with the same 60 s TTL, the same `reason` word and the
same advisory (never-binding) rule; a counted S2 fact writes **no** advisory row, because §4.20.5's trigger is
`stream_end == "idle_timeout"` and this ADR does not widen it. The two instruments are therefore fed by
overlapping but different populations, on purpose: the row steers one minute of *selection* and its trigger is
frozen by a case (`CONF-110`), while the streak moves an *account* and its evidence is deliberately wider,
because a head that never arrives is evidence about the route's availability even though it is not a stream
that went quiet after its head. A future reader must not "fix" the asymmetry by widening §4.20.5: doing so
would move `CONF-110`'s own assertion and buy nothing the streak does not already do.

**§2.5.4 The return trip resets the run — one delete, and it is a correctness rule.** A `plan.switched` row
that moves a family **back** to `primary` (`reason: "primary_recovered"`, whoever produces it: a probe success
or the plan's own window boundary) deletes the family's plan-route `silence_streak` rows. Without that delete
a family whose `cooldown` is short (the test rigs run `0s` deliberately) could return to its plan **inside** the
run that spilled it, and the very next single stall would be a K-th fact — re-spilling a family that had just
proved itself. With it, a returned family starts from zero, which is also what `/health` reports (§2.8.3). This
is not a "reset on success": a *successful ordinary request* on the plan route does **not** touch the streak —
the run is time-bounded, a mid-run success does not un-happen two silences, and a store write on the success
path would tax the data plane for a fact the run's own W already bounds.

### 2.6 Reload, restart, in-flight

| Interplay | Ruling |
|---|---|
| **restart** | nothing is lost: the streak is in the store, and even a lost projection is rebuilt from `events` (§2.3.3). A restart does **not** reset a run — a family that stalled twice before a redeploy is still two facts after it |
| **reload** | a reload **must not zero the streak** (it is state, not config — the store row is untouched by `config.applied`), **must not retro-escalate** (a lowered `threshold` is not evidence about the plan, so a config change can never trigger a move; escalation happens only when a *fact* lands), and **must not re-judge history**: new K/W apply to facts that arrive after the reload, evaluated then. `ADR-056`'s in-flight precedent (a request keeps the bounds it entered with) is not extended to this window, and that is deliberate: those bounds *cut* a request, whereas K/W only decide a *future* move — the request whose fact is being evaluated has already ended |
| **in-flight request** | the escalation applies to requests that **start after** it. A request already past its guard is not recalled (§2.4.5), and a request that arrives later reads the new state through the guard's ordinary rule 4 (state `overflow` ⇒ `Downgrade`, spec §4.6) — the same path a `403` spill uses |
| **`recover: none`** | no probe; the family returns at the plan's declared window boundary when one exists (ADR-014 item 10), and that return is a `primary_recovered` move, so §2.5.4's delete applies. With no window either, the family stays on the metered route until an operator acts — unchanged |
| **an escalation while the overflow cap is reached** | the cap's refusal (`cost_cap_exceeded`, §4.6) is evaluated by the guard on the next request and wins, exactly as it does after a `403` spill: this ADR neither reads nor writes the cap |

### 2.7 The config key

**§2.7.1 Spelling and defaults.** One reloadable key on the policy object, in **both** spellings
(`plan_policy` and `plan_policies[i]`, one type):

| Key | Type | Default | Semantics |
|---|---|---|---|
| `silent_escalation` | object | `{enabled: true, threshold: 2, window: 5m}` | §4.21's escalation knobs: whether the rule runs, how many counted facts, over how long |
| `silent_escalation.enabled` | bool | **`true`** | `false` ⇒ the rule never moves the family; facts still write the advisory row and (S2) still record their classification row — the state is switched off, not the observation |
| `silent_escalation.threshold` | integer | **`2`** | K. **`>= 2`**; a value below 2 is a load refusal naming `plan_policy.silent_escalation.threshold` (§2.1.3) |
| `silent_escalation.window` | duration | **`5m`** | W, the config duration grammar (`300ms`…`15m`, spec §9.2's own; no `d` unit). Range **`1s` … `1h`**; outside it the load is refused naming the key |

*Why `enabled` exists at all:* an operator whose plan truncations are benign must be able to keep the old
behaviour, and the alternative (a meaningless threshold) is the "threshold as a flag" anti-pattern this
project refuses (`overflow_monthly_cap_usd`'s absent-means-off is the precedent, but a threshold's
absent-means-off would collide with its own default).

*Why the range ends where it does:* a window shorter than one fact's own silence-to-verdict interval can never
hold two facts (§2.1.1 (i)), so a sub-second value is a config that silently disables the mechanism; 1 s is the
floor because it is the scale the rigs state it at (`CONF-122` (b) drives `window: 1s`), and an executable case
is the fixed point (§4.20.1's precedent). The ceiling is 1 h because the duration grammar has no `d` unit, so
an hour is the largest window a file can state without composing units — and two facts an hour apart are
different incidents by any reading, which is not a pattern a cooldown-scale response should answer. Both ends
are refusals rather than silent clamps, the `§4.20.1` precedent.

**§2.7.2 Why the default is on, and why that is not a new economic choice.** The escalation only ever runs for
a family whose policy says `on_primary_exhausted: spill` — and `spill` is itself the default and the
operator's own declaration that *spending beats stalling*. Default-on therefore **makes an existing
declaration reachable**; it adds no new willingness to spend, which is the test ADR-012's human gate exists
for. The operator who wants the old behaviour writes `enabled: false`, and the operator who wants no spending
at all is already using `block` (§2.4.4).

**§2.7.3 It is not in the wizard's askable set, and the row set does not move.** `silent_escalation` is a
nested mapping inside a policy entry, and spec §4.11's locator rule already refuses to set a block value
(`plan_policies`'s own header is `NotSettable`, §4.11's "two paths not reachable by any row"); the eight
per-family rows of §4.11's `routing` section therefore stay exactly eight, and this ADR adds one **sentence**
to §4.11 saying so. The witness is the section table's own unit test, which builds the row set **over the
shipped root** (`setup/sections.rs`'s test module): the key lands in the examples only as a comment, so no row
appears — and if a row did appear, that test fails.

**§2.7.4 The examples carry a comment and no key; `R76-1` writes it.** `config.example.yaml`'s
`plan_policies` block gains a **comment-only** two-line note naming the key and its defaults (the GAP-Q15
rule: an example carrying a key the parser does not know is an unservable file, so the comment and the parser
land in the same change — `R76-1`'s). **This card changes no byte of the pair** (acceptance item 3b).

### 2.8 Observability

**§2.8.1 The event.** `plan.switched` with `reason: "primary_silent"`, `probe: false`, and the additive
`silence: {count, threshold, window_us}` evidence member (§2.4.3). Durability is ADR-014 item 8's **FULL** for
the unchanged reason (it changes what every subsequent request in the family is billed).

**§2.8.2 The trace.** `result.plan_switch.reason` gains `primary_silent`, and the **state-driven** arm of that
field reads the state's cause word instead of the literal `primary_exhausted` (spec §6's producer table). No
new trace field, no `schema_version` move: `plan_switch` is an optional object whose `reason` is a string, and
the value set was already open to extension (ADR-049 added `plan_exhausted` to it the same way). Invariant,
checkable: **on a record displaced by the family's state, `result.plan_switch.reason` equals the `reason` of
the `plan.switched` row that produced that state.**

**§2.8.3 `/health`'s `plan` block gains one read-only member** (spec §9.1):

```json
"escalation": { "count": 1, "threshold": 2, "window_ms": 300000 }
```

| Key | Type | Semantics |
|---|---|---|
| `count` | integer | the **live** run's count on the family's **currently active route**: `0` when the run has expired (`now − first_us > W`), when no run exists, and when the active route is not in the plan tier (a spilled family escalates nothing). Read from the `silence_streak` projection |
| `threshold` | integer | K from the **currently loaded config** — the value the next fact would be judged against, not the value in force when the run started (a reload moves a judgement, not a history) |
| `window_ms` | integer ms | W from the currently loaded config, in milliseconds — the spelling `details.window_ms` (§4.20.3) already uses for a duration on this wire, so the two silence instruments share one unit word |

`count` is a **count** (`§7`'s label set): it is not a cost, not a saving, and it prices nothing. `escalation`
is absent when `plan.configured` is false (§9.1's own rule: no policy, no family, no invented member).

**§2.8.4 `/metrics` does not move.** No series is added: the escalation's effect is already visible in
`vadis_plan_switches` (a switch is counted whether a `403` or silence caused it) and its evidence is a count
of records, not a money figure. `CONF-87`'s frozen series set therefore stays byte-identical, and
`DESIGN §12.21` is untouched. Registered (`R76-0-F4`) as the deliberate absence: an operator cannot chart the
escalation rate, and a future round that wants a series owes the §12.21 amendment and the `CONF-87` edit.

**§2.8.5 No new error class.** ADR-011's vocabulary stays closed: `stream_truncated`'s `action` remains
`abort` and **no class gains `fails_over`**; the escalation reuses the walk's existing move and produces no
`error.type`, no HTTP status and no §8 row. `demotes_provider()` is untouched — this moves a *family's routing
intent*, never a provider's availability.

### 2.9 What the round may not claim

1. **No figure here is a saving.** The escalation spends money by design (on purpose, for a shorter time); the
   replayed impact (§6.4) is `inferred` — a model over recorded facts — and is never added to any measured
   money line (ADR-058).
2. **The escalation is plan-family-scoped only.** No non-plan route escalates to anything account-level; no
   provider-wide state moves; no walk refusal is introduced.
3. **Nothing on the measurement side moves.** No gate definition, no threshold of an existing gate, no corpus,
   no L1-envelope value, no `CONF` assertion outside the two new parked files (AGENTS 9 / ADR-012). §5 claims
   **no** amendment to any existing case.
4. **The mechanism is not a substitute for the `403`.** A plan that says `403 quota_exhausted` still moves the
   account through ADR-014 item 5, and `403` remains the only class that moves it *on a billing fact*.

---

## 3. Blast radius, in both directions

**Gained.** A family whose plan went quiet stops serving from it after the second fact in five minutes instead
of never: the clients that follow are served by the metered route in roughly the time it takes to fail twice
rather than after a day of stalls. A measured window's 113 truncations and 4 head timeouts become *observable
as an account fact* for the first time, and on the audited counts the mechanism is expected to fire about
five times inside the three-day exhaustion window while a healthy fortnight is expected to produce about half
an event (§2.1.1). The log also gains the row it was missing for a head that never arrives (§2.2.2), so
"how many timeouts hit route R" becomes answerable from `events`.

**Paid.** A false positive costs one cooldown of metered spend plus a re-prefill for the sessions it re-points
(ADR-014 item 5's priced switch) — the reason K = 2, W = 5 m and not something wider. The mechanism is on by
default for `spill` families (§2.7.2), which is a behaviour change for a config nobody edits; the book owes the
sentence that says so (§6.3). One new store table and one new column, plus one projection to rebuild (§2.3).

**Unchanged, deliberately.** No relayed byte; no `result.status`; no `error.type`, HTTP status or §8 shape; no
`DecisionRecord.schema_version`; no money, `usage` or ratio; no `error.class` or `demotes_provider()`;
ADR-056's row, TTL and advisory rule; ADR-014's probe predicate and cooldown; ADR-063's ordering key; `/metrics`'s
frozen series set; `/state/events`'s figures.

---

## 4. The cases, and what each limb observes

Each case runs through the real `serve` assembly over loopback mocks (the existing testkit), reads the trace
JSONL, the event log and `/health` as a stranger would, and asserts **values**, never shapes. Both files are
written to compile against the **contract's** surfaces only — the trace, `Query::AllEvents`, `Query::PlanState`
and `/health`'s JSON — because the store's `silence_streak` has no query until `R76-1` adds one, and a parked
case that does not compile is a red contract. **They therefore assert the escalation's observable behaviour,
never the counter's internals**; the projection's own identity is the store's unit-test obligation (§6.2).

### 4.1 `CONF-121` — the escalation: two facts inside five minutes move the family

Rig: one family, `p-plan/m1` (`account: coding_plan`) → `p-api/m1` (`account: api`), policy
`on_primary_exhausted: spill`, `recover: probe`, `cooldown: 0s` (the testkit's convention: it removes "the
cooldown has not elapsed" as an alternative explanation), `silent_escalation: {threshold: 2, window: 5m}`, and
`server { upstream_attempt_timeout: 1s, stream_idle_timeout: 400ms, request_timeout: 8s }` so no limb rests on
machine speed.

| Limb | Rig | Observation point | Asserted value | What turns it red |
|---|---|---|---|---|
| **(a) two silence facts spill the family** | `p-plan` queues a `silence()` answer (head + one event, then the socket held open) **twice**; `p-api` queues a clean stream. Two streaming requests on the family, back to back | after both: `Query::PlanState`; `Query::AllEvents`; `p-plan.requests()`; `p-api.requests()`; the two records | `plan_state.account == "overflow"`; exactly **one** `plan.switched` row with `reason: "primary_silent"`, `probe: false`, `from_route: "p-plan/m1"`, `to_route: "p-api/m1"`, and `silence: {count: 2, threshold: 2, window_us: 300000_000}`; `p-plan` saw exactly **2** requests, `p-api` **0** (nothing was retried inside either request); record 1 and record 2 both carry `errors[0].details.error_class == "stream_truncated"` and `plan_switch: null` | the base tree (no key: the config is **refused** at load); an implementation that spills on the first fact (one `p-plan` request, and a `plan.switched` after it — the over-trigger §4.2 (a) forbids); one that never spills; one that retries the truncated request onto `p-api` inside the same request (`p-api` sees 1 before the third request) |
| **(b) the third request is displaced, and says why** | immediately after (a): one more streaming request on the family; `p-api` queues a clean stream | the third record; `p-api.requests()`; `/health` | served `200` from `p-api` (its mock sees the request); `result.plan_switch` present with `reason: "primary_silent"`, `from: "p-plan/m1"`, `to: "p-api/m1"`, `probe: false`; `result.failover_from` **null** (nothing failed in this request — the state moved it); `decision.provider == "p-api"` | an implementation that labels the displacement `primary_exhausted` (the word says an upstream declared exhaustion and none did); one that sets `failover_from`; one that leaves the family on `p-plan` |
| **(c) the head that never arrived counts too** | a raw listener that accepts, reads the request and never answers (CONF-105's `slow_head_listener` idiom), `upstream_attempt_timeout: 400ms`; two such requests on the family, then a third | the event log; `plan_state`; the third record | the log carries **two** `error.classified` rows with `reason: "timeout"`, `stage: "unknown_outcome"`, `stream: true` and a `route` member of `{provider: "p-plan", model: "m1"}`; `plan_state.account == "overflow"` with one `plan.switched` row `reason: "primary_silent"` whose `silence.count == 2`; the third request is served by `p-api` | the base tree, which writes **no** classification row for a head timeout at all (finding `R76-0-F1`), so a fold over the log sees zero facts; an implementation that counts only `stream_truncated` (the head half of the owner's sentence is dropped); one that counts a *buffered* timeout into the same route (limb (d) of `CONF-122` is the control) |
| **(d) the return trip resets the run** | continuing (b): `p-plan` queues a clean stream; the next request is a session's **first** request (`turn_index == 1`, `prompt_cache_key` set), so it is admitted as a probe and succeeds; then one more silence on `p-plan` | `Query::PlanState`; the events; `/health` after the probe | after the probe: a second `plan.switched` row `reason: "primary_recovered"`, `account == "primary"` again, and `/health`'s `plan.escalation.count == 0`; then after **one** further silence the family is **still** `primary` (a fresh run at count 1) and `/health` reads `count: 1` | an implementation that leaves the run alive across the return (its `count` is 2 after the probe, and the single further stall re-spills the family — the loop §2.5.4 exists to close); one that resets on the probe *attempt* rather than on the successful return |
| **(e) the streak survives a restart** | after (a) but before the second fact, stop `serve`, start it again on the **same** state directory, then send the second silence and one more request | the events across both runs; `plan_state`; the third record | the second fact still completes a run of 2 (the `plan.switched` row's `silence.count == 2`), and the following request is served by `p-api` | an implementation that keeps the count in memory: the restart zeroes it, the second fact is a run of 1, and the family never spills |

### 4.2 `CONF-122` — the refusals: nothing over-triggers, and nothing outside a family moves

| Limb | Rig | Observation point | Asserted value | What turns it red |
|---|---|---|---|---|
| **(a) one fact never escalates — the over-trigger control** | the `CONF-121` rig with **one** silence, then a clean request on the family | `Query::PlanState`; the events; `/health` | **no** `plan.switched` row of any reason; `plan_state` absent or `account == "primary"`; the second request is served by `p-plan`; `/health`'s `escalation.count == 1` (the fact was counted — the rule just did not fire) | **the red control the card names**: an implementation that demotes on one row — it spills after the first silence, and `plan_state.account` reads `overflow` with one `p-plan` request behind it. Also red for an implementation that does not count at all (`count == 0`, and limb (a) of `CONF-121` could never pass) |
| **(b) facts outside W never accumulate** | one silence under `window: 1s` **from the initial config**, then a second silence 1.5 s later | the events; `plan_state` | two counted facts, **two** separate runs (`/health`'s `count == 1` after the second), **no** `plan.switched` row; the family stays `primary` | an implementation that counts the fortnight instead of a window (no `first_us` anchor), or one whose window is read from the wrong config (a reload that re-zeroes the run would also be visible here as `count == 1` — which is why limb (f) uses a *raising* reload) |
| **(c) the causes that must not count** | four requests on the family whose `p-plan` mock answers, in order: an SSE body that **ends early** with no terminal event; a body cut by `server.request_timeout`; a body the **client** abandons (the test drops the response, `peer_aborts()` on the mock proves it); a mid-body stall on the **buffered** path | the records' `details.stream_end`; the event log; `plan_state` | the four records carry `stream_end` of `upstream_ended_early`, `request_timeout`, `client_closed` (and the buffered leg carries no `stream: true`); the fold sees **0** counted facts and the family never spills | an implementation that counts "any truncation" (ADR-056 §2.5's own reason for separating the causes — a route that is answering would be punished); one that counts a client's departure; one that reaches the buffered path |
| **(d) a metered route's facts never reach a plan route's counter** | after the family has spilled (`CONF-121` (a)'s mechanism), make `p-api`'s mock return a mid-body-type failure twice (an abort after the head, which classifies as a truncation) | the events; `plan_state`; `/health` | the facts are recorded against `{provider: "p-api", model: "m1"}`; the family's `escalation.count` stays **0** and no `plan.switched` row is added; the requests keep being served by `p-api` (a metered route has no plan tier to leave) | an implementation that keys the streak by family rather than by route, or one that reads the overflow's failure as evidence about the plan's account |
| **(e) `block` never escalates, and no policy means no account-level anything** | two rigs: (i) the `CONF-121` family with `on_primary_exhausted: block`; (ii) a no-policy roster of two routes with a `fallback` chain, `p1` stalling twice | `Query::PlanState`; the events; `/health`'s `plan.configured` | (i) **no** `plan.switched` row; the two requests fail exactly as one fact's request does (§8's `quota_exceeded` refusal path is **not** reached — the failure is the attempt's own `502`/truncation); (ii) `plan.configured: false`, no `escalation` member, `plan_state` empty, and the walk behaves as ADR-056/ADR-063 left it (the advisory row still steers, nothing account-level moves) | an implementation that escalates under `block` (converting silence into a fabricated `quota_exceeded`); one that treats a provider-wide demotion or a cooldown as an account move; one that invents a `plan` block for a policy that does not exist |
| **(f) a reload does not zero the streak** | after `CONF-121` (a)'s **first** fact, rewrite the pair with `threshold: 3` and wait for `config_digest` to move, then send the **second** silence | `/health`; the events; `plan_state` | after the reload `/health` reads `threshold: 3` and `count: 1` (the run survived); the second fact makes `count: 2` and the family is **still** `primary` (K is now 3); a third fact spills it, with `silence.threshold == 3` on the row | an implementation that rebuilds the projection on reload (the count goes to 0 and the second fact is a run of 1 — indistinguishable from limb (b), which is why this limb raises K); one that retro-escalates on the reload itself (a `plan.switched` row appears with no fact behind it) |

---

## 5. The conformance amendment list (frozen, and it is **empty**)

**This ADR amends no existing case.** The claim is checkable and each neighbour is named:

| Neighbour | Why it stays byte-identical |
|---|---|
| `CONF-108/109/110` (ADR-056's three) | their rigs declare **no** `plan_policy`, so no family exists, no streak can escalate, and §4.20.5's trigger, TTL and advisory rule are untouched. `CONF-110`'s leaves read `provider_cooldown`, which this ADR does not write |
| `CONF-105` (head arrival) | its slow-head limb asserts the classified outcome, `no` retry, `no` `failover.triggered` and exactly one record — none of which moves when the arm additionally writes its `error.classified` row (the set of event kinds is not asserted, and `failover.triggered` still does not appear) |
| `CONF-21` (projection == rebuild) | it hand-builds an `error.classified` payload and asserts the cooldown projection's identity; the new `route` member is additive on the payload and `rebuild_cooldown` reads `demotion` only. The **new** projection's identity is owed by the store's own unit test (§6.2), not by this case |
| `CONF-87` (`/metrics`) | no series moves (§2.8.4) |
| `CONF-32/33/34/35/36/38/39/40/93/97/101/102/103` (the plan family's own) | every one of them moves the family with a `403` (or a state read), where nothing counted in this ADR exists; the state-driven displacement's `reason` word is unchanged for a `403`-caused state (§2.8.2's invariant: the word equals the state's cause, and a `403`-caused state's cause is `primary_exhausted`) |
| `CONF-71/72/76` (`/health`'s plan block) | they read members by name (`account`, `probe.blocked_by`, …) and none asserts the object's key **set**; an additive member cannot fail them. Verified by reading the three files at this card's HEAD |

If `R76-1`'s suite run turns any case red, that is a **finding for the owner** (recorded, not repaired under
this authorisation) — the `R74-0e` lesson.

---

## 6. Landing plan — what `R76-1` owes, and only this

### 6.1 The code (one change, these files)

| File | Change |
|---|---|
| `crates/vadis-core/src/config.rs` | `SilentEscalationCfg { enabled: bool = true, threshold: u32 = 2, window: DurationVal = 5m }` on the policy type; the two load refusals of §2.7.1 naming the key |
| `crates/vadis-core/src/plan.rs` | one **pure** predicate beside the existing policy predicates: `silence_escalates(streak: Option<&StreakRow>, policy: &SilentEscalationCfg, state: &PlanStateRow, route: &RouteId, now_us: i64) -> bool` — §2.4.1's five conditions, no I/O, no clock argument except the fact's own `ts_us` |
| `crates/vadis-core/src/store.rs` | `Projection::SilenceStreak`; `ProjectionWrite::SilenceStreak { provider, model, count, first_us, last_us, last_event }` and `SilenceStreakClear { provider, model }` (the return trip's delete, §2.5.4); no `EVENT_SCHEMA_VERSION` move |
| `crates/vadis-store/src/lib.rs` | DDL **v4** (`silence_streak`), the upsert, the clear, `Query::SilenceStreak { provider, model }`, and `rebuild_silence_streak` (the fold of §2.3.3) wired into `rebuild`/`Projection::All`; `plan_state.cause`(§6.2) read/written by the existing plan-state paths |
| `crates/vadis-proxy/src/stream_forward.rs` | the truncation arm (`record_classified`) and the head arm (`OpenHead::UnknownOutcome`) both call one `record_silence_fact(...)`: it writes the classification row (S2's new row included, §2.2.2, with the `route` member), the advisory row for S1 exactly as today, the streak upsert, and — when §2.4.1 holds — the escalation via the existing `record_plan_switch` with `reason: "primary_silent"` and the `silence` evidence member |
| `crates/vadis-proxy/src/forward.rs` | `record_plan_switch` accepts the cause word and writes `plan_state.cause`; its `to_account == "primary"` arm performs §2.5.4's streak clear; the bounded `Query::SilenceStreak` read for the guard's own family state (read-only, one row) |
| `crates/vadis-proxy/src/health.rs` (`health_json`) | the `escalation` member of §2.8.3, read from the projection and the current config (the `plan` block it already builds) |
| `config.example.yaml` + the binary's embedded copy | the **comment-only** note of §2.7.4, in the same change as the parser (GAP-Q15) |
| `tests/conformance/tests/conf_121_*.rs`, `conf_122_*.rs` | un-park (drop the `#[ignore]`), and — only if a limb needs it — add a `Query::SilenceStreak` assertion; the limbs' **contract** is the two tables in §4 and may not be weakened |

### 6.2 The two store-level obligations that the cases cannot witness

1. **`rebuild_silence_streak` == the incremental path** (CONF-21's rule, extended to the new projection):
   a store unit test folds a hand-written event sequence twice — through the writer and through the rebuild —
   and asserts byte-equal rows. This is a **unit** test in `vadis-store` (the `plan_state` precedent at
   `crates/vadis-store/src/lib.rs:1367`), not a conformance case, because a conformance case cannot reach the
   rebuild without a store double.
2. **`plan_state.cause`** joins the same identity test: an incremental move and a rebuilt one carry the same
   cause word, and a DDL-v3 database migrated forward reads `NULL` as **`primary_exhausted`** (the only cause
   that existed before this ADR), stated so the reading is not a guess.

### 6.3 The documents and the user-facing sentence

`docs/spec.md` §4.21's status line flips *contracted, not served* → *served*; `DESIGN §12.9`'s register line
records the landing; `DESIGN §12.8`'s two rows are unchanged (an ID is a contract); and **`book/` gains the
sentence an operator needs**, in the chapter that owns the cost/behaviour levers:

> *"If a plan route goes quiet twice within five minutes — a stream that stalls mid-body, or a head that never
> arrives — the family moves to its metered account even though the provider never said `quota exhausted`; that
> move spends money, is recorded with `reason: primary_silent`, and comes back the usual way (a probe after the
> cooldown). `plan_policy.silent_escalation` turns it off or retunes it."*

This card's write set is `docs/`, `design/` and the two parked case files by the round's own boundary, so the
book sentence is `R76-1`'s to land (AGENTS constraint 8).

### 6.4 What `R76-1` must **measure**, and what it must not claim

1. **The fortnight, replayed.** Over the audited trace JSONL (2026-10-04 → 2026-11-14, the same 15,040
   records), compute **how many escalations would have fired, in how many distinct runs, on which routes**, at
   three settings: `(K=2, W=60s)`, `(K=2, W=5m)` — the shipped default — and `(K=2, W=15m)`. A `python3`
   over the JSONL is the intended instrument. **Every figure is `inferred`**: it is a model over recorded
   facts, not a measurement of what the gateway did (AGENTS constraint 4, ADR-058).
2. **Say what the replay cannot see.** The audit predates `details.stream`'s presence on the head arm's
   failure and the `route` member (§2.2.2), so the S2 population must be reconstructed from
   `result.upstream_ms` pinned at the attempt knob plus `errors[0].details.error_class == "timeout"`; the
   report must say so, and must not present the reconstructed count as equal to a live count.
3. **Before/after on one rig**: `CONF-121` (a)'s two requests, the `p-plan`/`p-api` request counts, and the
   wall-clock from the first fact to the `plan.switched` row.
4. **The default's adequacy, stated as a finding if it fails.** If the `W = 5m` replay yields **zero**
   escalations on the 113-abort fortnight while `W = 15m` yields several, that is a finding for the owner
   (the frozen default is too tight for the deployment's retry shape) — and `R76-1` **does not change the
   default**: a default is a contract value, and changing it is a human decision (AGENTS 9 / ADR-012).
5. **Not measured, and said so**: the live effect after deployment (whether traffic actually moves) is read
   from the *next* window, exactly as ADR-056 §6.4 item 3 ruled for its own instrument.

---

## 7. Alternatives considered

| Alternative | Why rejected |
|---|---|
| count `provider_cooldown` rows with `reason = "stream_silence"` | §1.2 fact 3: the row is an upsert per route, so two facts inside one TTL leave **one** row; at W = 5 m the first has expired before the second lands. Counting rows is structurally wrong, not merely fragile |
| keep K = 1 and let the advisory row's repetition speak | the row is 60 s and never account-level (ADR-056 §2.5.3's availability reason). Repetition inside 60 s is invisible — the row was overwritten — so "it keeps happening" is exactly the fact the row cannot carry |
| a sliding window over the last W of facts | §2.1.2: it needs every timestamp or a per-read recomputation, and a steady drip of stalls can hold it open forever — one stall per 0.9 W spills the family in a loop |
| W = 60 s (the card's recommendation) | §2.1.1: the audit's own retry figure is that 83 of 106 truncations were retried within five minutes; a 60 s window reaches the burst but not the retry, i.e. it would miss most of the incidents this round exists for (≈1.1 expected fires in the whole three-day exhaustion window) |
| W = 15 m or wider | §2.1.1: detection rises to ≈13 fires in the exhaustion window, but the coincidental rate on a healthy fortnight rises to ≈1.6 events — 3× the expected false spills, each costing metered spend and a re-prefill |
| make the escalation **binding** (demote the provider) | that is ADR-011 item 4's unit for a *billing* fact. A silence is a fact about the route (§4.20.5) and, at the escalation's scale, about the *family's account being unusable*; a provider-wide row would take the account's other models out on one route's bad five minutes |
| move the account from the **guard** at the next decision instead of at the fact's write | the lazy form makes `/health`, the event log and the state disagree until traffic happens to arrive; with an expired window it would escalate nothing, and with a live one it would escalate hours later on evidence that has aged. The eager form is also the `403` path's own shape (the move happens where the fact is classified) |
| add a new `error.type` / class for silence | ADR-011's vocabulary is closed; the owner's sentence asks for a *move*, and a move needs no new word on the wire. `stream_truncated`'s `abort` stays, no class gains `fails_over` |
| reset the streak on every successful request | §2.5.4: a store write on the data plane's success path for a fact the run's own W already bounds. The loop that actually matters — a family returning inside the run that spilled it — is closed by the return trip's delete |
| escalate under `on_primary_exhausted: block` | §2.4.4: `block` means "do not spend", so there is no destination; and refusing would have to claim an exhaustion nobody declared |
| put the knobs in `server` | the escalation is a family's routing intent; a global knob would force one tolerance on every family and put plan semantics in the server block |
| two flat keys (`silent_threshold`, `silent_window`) and let the wizard ask | a half-written pair is meaningless, and it would put two new rows in §4.11's per-family set for a value whose defaults are the contract's. The object is one key, one default, no wizard row |
| land the surface and the contract in one change | the owner's split: the contract is reviewed alone (`R76-0`), the code lands against it (`R76-1`), and the cases are parked in between so the suite is never red |

---

## 8. Register

### 8.1 IDs spent

| ID | What it is |
|---|---|
| **`ADR-064`** | this file |
| **`CONF-121`** | §4.1's case — the escalation; file lands parked `#[ignore = "CONF-121: depends on R76-1"]`; `DESIGN` §12.8's heading moves `CONF-01…CONF-120` → `CONF-01…CONF-122` |
| **`CONF-122`** | §4.2's case — the over-trigger controls; parked the same way |
| **spec `§4.21`** | the new section (`§4.20` is ADR-056's; `§4.21` verified free at this branch's HEAD) |
| **store DDL version 4** | `silence_streak` + `plan_state.cause` |

No `GAP-Q` id, no gate, threshold, corpus or L1-envelope value (AGENTS 9).

### 8.2 What this ADR adds to the documents

- `docs/spec.md`: **§4.21** (the whole contract), **§4** (the schema sketch: the key and its three commented
  members, ADR-056's precedent), **§4.6** (one key row, three refusal rows, and rule 3's
  mover sentence naming §4.21 as the second — and only second — way an account may move), **§4.11** (one
  paragraph: the key is not askable and no row moves), **§6** (`plan_switch.reason`'s fifth member, the
  producer table's new row, and the state-driven arm's word), **§9.1** (`plan.escalation` and its three rows).
- `design/DESIGN.md`: **§12.6** (one bullet: `ResultRec.plan_switch.reason`'s value set and its state-driven
  arm; `schema_version` does not move), **§12.8** (heading, two rows, the allocation paragraph),
  **§12.9** (the round's register line, naming what is owed and what is deliberately not), **§12.10.4**
  (DDL v4, the table, the rebuild, the `cause` column), **§12.10.8** (the escalation's landing: the two fact
  writers, the pure predicate, the move, the session re-points, the return-trip clear).
- **§12.21 is deliberately untouched** (§2.8.4): no series moves, so the frozen set and `CONF-87` do not.
- `ADR-011`, `ADR-014`, `ADR-056`: one **dated note** each (§8.3).
- `tests/conformance/tests/conf_121_*.rs`, `conf_122_*.rs`: new, parked.

### 8.3 The three dated notes this ADR owes (the shape each takes on its own file)

> **On `ADR-056` — dated note (2026-11-14, `R76-0` — append-only).** §5.4's separation keeps its text and its
> reasoning: a single `stream_silence` row is a stream fact, never an account fact, and the probe gate stays
> blind to it. What this note records is the door §2.5 left closed: **inside a plan family that declared
> `on_primary_exhausted: spill`, a *run* of two such facts within five minutes is an account fact**, and it
> moves that family's state (ADR-064, spec §4.21). Nothing of §2.5 moves: K = 1 stays the row's trigger, the
> 60 s TTL stays a constant, the advisory (never-binding) rule stays, the probe gate stays blind to a single
> row, and §5.4's exclusion is exactly what makes the escalation's own scope narrow — it can only act where
> the config already declared a spill. §2.5.1's rejection of a streak is not reversed: it stands for the row
> (a row count cannot be a streak) and the streak this note admits is a **different artefact with a rebuild
> path** (ADR-064 §1.3, §2.3).
>
> **On `ADR-014` — dated note (2026-11-14, `R76-0` — append-only).** Item 2's *"the only signal that may move
> the account"* keeps its billing half and gains one narrow second member: a `403 quota_exhausted`
> classification remains **the only mover on a billing fact**, and `ADR-064` adds the only mover on a
> *silence* fact — a run of two counted `stream_silence` facts within 5 m on the family's active plan route,
> under `spill` (spec §4.21). Nothing else in item 2 moves: the local counter is still a warning, the window
> still only defers a probe. Item 3's probe predicate, the session-boundary hard rule, the 15 m cooldown, item
> 5's first-displacement record and item 9's rule order are untouched; the new move uses item 8's
> `plan.switched` event with `reason: primary_silent` and item 5's pricing convention unchanged (the move is
> still a cache-namespace change and is still priced as a failover).
>
> **On `ADR-011` — dated note (2026-11-14, `R76-0` — append-only).** `stream_truncated`'s action is untouched:
> it is still `abort`, no class gains `fails_over`, `demotes_provider()` still answers `true` for
> `QuotaExhausted` alone, and item 6 row 3's *no retry after a full write* is enforced rather than relaxed —
> `ADR-064` §2.4.5 makes that rule the reason the escalated family's K-th request is **not** retried onto the
> overflow route. What this note records is one gap this ADR had to close *in this taxonomy's own terms*:
> item 8 says *"every classification writes one event"*, and the streaming head arm's `unknown_outcome`
> (`timeout`) wrote none, so the log could not answer *"how many head timeouts hit route R"*. `R76-1` gives
> that arm the row item 8 already requires (`reason: "timeout"`, `action: "abort"`, `matched: false`,
> `stream: true`), additive on an events payload, no `EVENT_SCHEMA_VERSION` move, and no class's meaning
> changes (finding `R76-0-F1`).

---

## 9. Findings and gaps this card registers

| # | Class | Site | Effect | Owner | Due |
|---|---|---|---|---|---|
| **`R76-0-F1`** | unrecorded-fact | `crates/vadis-proxy/src/stream_forward.rs:1901-1930` (`OpenHead::UnknownOutcome`) — a `502` with `details.stage: "unknown_outcome"`, `error_class: "timeout"`, and **no** `error.classified` row (`record_failure_trace` → `finish_failure` writes the trace only; the two `ErrorClassified` writers are `forward.rs:2857` and `stream_forward.rs:2357`) | the log cannot answer *"how many head timeouts hit route R"* — the population of F3 (the 4 audited rows) is invisible to every projection, so no counter, tool or report can see the head half of the owner's sentence. **Repaired by `R76-1`** as a forced consequence of §2.2.2; no decision moves | `R76-1` | this round |
| **`R76-0-F2`** | registered-open | `on_primary_exhausted: block` families get no escalation (§2.4.4) | an operator who chose *fail rather than spend* still stalls on a silent plan; there is no word in §8 for "the plan went quiet", so refusing would need a new one. Deliberate this round | human | a later round, if the owner wants `block` to refuse on silence |
| **`R76-0-F3`** | unmeasured-default | `silent_escalation.window`'s default (5 m) is an **independence model over the audited counts**, not a replay (§2.1.1) | if `R76-1`'s replay at 5 m finds zero escalations on the 113-abort fortnight, the default is too tight and a later ADR must raise it — a human decision, never an implementer's | `R76-1` (measure) / human (change) | `R76-1`; the change is a later round |
| **`R76-0-F4`** | registered-absence | `/metrics` gains no series (§2.8.4) | the escalation's rate cannot be charted; its count and its switches can (`/health`'s `escalation.count`, `vadis_plan_switches`). A series would owe the §12.21 amendment and the `CONF-87` edit | human | if an operator asks for the chart |
| **`R76-0-F5`** | registered-boundary | the streak is keyed by route, so a family whose plan tier has several members accumulates one run per member | a tier of three plans that each stall twice in five minutes escalates on the first member that reaches K and then, being already off the tier, on nothing else until it returns; the other members' runs expire by W. Deliberate: the move is per family, and a per-member cascade would be three spills for one fault | human | if a multi-plan deployment is added |

---

## 10. What a future reader must not conclude

- **Not** *"a silence now demotes the provider."* It never does. One row is advisory and route-scoped
  (ADR-056 §2.5); a *run* of two moves one **family's account**, and only when that family declared `spill`.
  `demotes_provider()` is untouched.
- **Not** *"K = 1 was quietly implemented."* K = 2 is enforced by a load refusal below 2, and `CONF-122` (a) is
  the control that turns red on a one-fact demotion.
- **Not** *"ADR-056's K = 1 was reversed."* It stands for the cooldown row; the escalation counts a different
  artefact with a different scope and a rebuild path (§1.3).
- **Not** *"the truncated request is retried on the metered route."* It is not (§2.4.5): the bytes were on the
  wire or the attempt may have been billed. The escalation serves the requests that come **after** it.
- **Not** *"a head timeout writes a `stream_silence` cooldown row."* It does not; §4.20.5's trigger is
  unchanged. It contributes to the **streak** only, and that asymmetry is deliberate (§2.5.3).
- **Not** *"the escalation is on for every route."* It is inert outside a plan family, under `block`, and for
  every non-counted cause.
- **Not** *"a successful request resets the counter."* The return trip does (§2.5.4); an ordinary success does
  not, and the run expires by its own W.
- **Not** *"`primary_silent` means the plan is exhausted."* It says the route went quiet; whether the allowance
  is gone is the upstream's word to give (`403`), and this mechanism exists precisely because it never came.
- **Not** *"R76-0 changed a product byte."* The diff is `design/`, `docs/` and two parked case files;
  `crates/`, the example pair, `book/` and `README.md` are untouched.
