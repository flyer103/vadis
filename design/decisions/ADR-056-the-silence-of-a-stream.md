# ADR-056 — the silence of a stream: the gap bound gets its own key, a truncation names its cause, a request lands in exactly one bucket, and a silent route loses its **next** selection

- Status: **accepted, contract only** (R74-0, 2026-10-17). The contract below is frozen; the code that
  makes it real lands in a **separate** change (`R74-1`), and §6 says exactly what that change owes.
  Until it lands, `server.stream_idle_timeout` is a key today's loader **refuses** (`deny_unknown_fields`),
  no trace record carries `details.stream_end`, `/metrics` has no incomplete series, and `CONF-108/109/110`
  are parked `#[ignore]`d.
- Kind: **a contract change with no product byte.** It amends three documents (`docs/spec.md`,
  `design/DESIGN.md`, one dated note each on `ADR-053` and `ADR-011`), allocates three conformance IDs,
  writes their three case files **parked**, and freezes a short amendment list for three existing cases
  (§5). Nothing under `crates/`, `config.example.yaml`, `book/` or `README.md` is touched by this card.
- Authority: the owner's round ruling of 2026-10-17 (*"open R74 — the silence of a stream"*), which
  reserved the numbering this ADR spends (`ADR-056`, `CONF-108/109/110`, spec `§4.20`). The evidence is a
  13-day trace audit on the owner's own deployment (2026-10-04T05:17Z → 2026-10-17T09:15Z, 7,036
  records over 173 rollover files); the durable copy of that audit lives with the round's own evidence,
  and every number quoted below is **read out of it**, never estimated.
- Supersedes: **ADR-053 §2.1 item 2's *value* only.** The construction of the body's bound is untouched
  (one read at a time, one idle arm inside `read_chunk`, R4's original letter restored by ADR-053);
  what moves is **which config key it reads** — `server.upstream_attempt_timeout` becomes
  `server.stream_idle_timeout` (§2.2). ADR-053's head-arrival ruling, its `request_timeout`
  enforcement and its `CONF-105` allocation all stand. A dated note closes that clause on ADR-053
  itself (§8.3); not one sentence of ADR-053's ruling text is rewritten (append-only, AGENTS 9).
- Amends: `design/DESIGN.md` §12.10.3 **R4** (the gap bound's key), §12.6 (an additive `details` key),
  §12.8 (heading + three rows + the allocation paragraph), §12.9 (the round's register line), §12.21
  (the new series and the frozen-set amendment); `docs/spec.md` §4 (the schema block), §4.2 (the
  classification-evidence clause's bound sentence), §4.15 (one clause: the new key is *not* refused),
  §4.16 (one series row), §6 (the cause vocabulary and the bucket predicate), §9.2 (the three-bucket
  figure list), and the new §4.20; `ADR-011` (a dated note: item 4's *"a fact about the attempt or the
  route"* is the door §5.4 walks through; `demotes_provider()` is untouched).
- Cases: **`CONF-108`** (the gap bound is its own key, the three causes are distinguishable, the healthy
  stream is untouched, reload does not reach an in-flight request), **`CONF-109`** (the three buckets
  partition `requests`, and `usage_missing` is orthogonal), **`CONF-110`** (a silent route orders one
  short **route-scoped** advisory row; `QuotaExhausted` keeps its semantics). The three files land with
  this card, parked `#[ignore = "CONF-10N: depends on R74-1"]` (the `CONF-20…25` parking rule).
- Related: `AGENTS.md` constraints **1** (no relayed byte changes — §2.5), **2** (determinism: nothing
  here depends on a clock the content does not), **3** (the observation boundary), **8** (docs before
  code — §6.3 names the `book/` sentence the *implementing* change owes), **9** (**the measurement is
  not part of the search space** — §5's amendment list is the owner-authorised act, and §6.4 says what
  is *not* measured here); **ADR-005** (the trace is the interface), **ADR-010** (the event log is state
  truth), **ADR-011** (the error taxonomy and the cooldown projection), **ADR-041** (the `/metrics`
  single-owner rule), **ADR-053** (head-arrival + the outer bound), **ADR-024** (a pre-attempt skip
  narrates `failover_from`, never a `failover.triggered` row), **ADR-021/ADR-012** (the register).

---

## 1. Why

### 1.1 One root cause behind three findings

The audit's first, second and fourth findings share a shape: **after the response head has arrived,
vadis has no concept of "this stream has gone quiet."** The head-arrival work (ADR-053) named the
clock for the *head*; the body's gap bound was then read from the *attempt* knob, so the only thing
that could end a silent stream was a budget meant for "connect and get a head".

Measured, on the owner's 13-day window:

| # | Fact | Evidence (from the audit, trace-only) |
|---|---|---|
| **F1** | The per-read gap bound was the attempt budget (**120 s** live). A stream whose head arrived in ~5 s then sat silent for ~115 s, and vadis only then declared truncation. | 106 records with `result.status = 200`, `errors[0].kind = upstream_error`, `details.error_class = "stream_truncated"`, `usage_missing = true`; their `overhead_ms` median **120,010 ms** against an `upstream_ms` median of **5,163 ms**. 106 × 120 s ≈ **3.5 h** of pure waiting; 83 retried within five minutes, only 43 of those met a clean `200` → **63 turns substantively broken**. Per provider: `zai-cn-plan` **102/3,392 ≈ 3.0 %**, `deepseek` 24 explicit `502` + 4 hidden. |
| **F2** | Those 106 were **counted as successes**. | At the audit's instant `/metrics` reported `vadis_requests 22`, `vadis_requests_succeeded 22`, `vadis_requests_failed 0` in the same window that `vadis_overhead_ms_p99 81,320` described. The event log knew (`error.classified`, `reason: stream_truncated`, `action: abort`, 103 rows) — the **aggregation layer** did not. |
| **F4** | A route that had just stalled kept taking **all** of the traffic. | `provider_cooldown` held **0 rows**. `rebuild_cooldown` projects only the `demotion` object of `error.classified` / `failover.triggered`, and `demotes_provider()` answers `true` for `QuotaExhausted` alone. In the ten minutes after each of the 106 truncations: **1,113 requests, 100 % still on the same provider**; zai's 3.0 % failure rate persisted across 10-06 → 10-08 (97 truncations) with nothing moving it. |

### 1.2 What the code does today, and the two facts the audit's own wording hides

`crates/vadis-proxy/src/stream_forward.rs:891` builds the streaming client with
`Duration::from_millis(self.config.server.upstream_attempt_timeout.0)`, and the relay's per-read window is
`std::cmp::min(ctx.idle, window)` (`:1815`) — one knob, two meanings. The relay already distinguishes three
terminal shapes internally, and **the record collapses them into one sentence**:

| the relay's own arm (`stream_forward.rs`) | what the record says today |
|---|---|
| `StreamRead::Failed { timed_out: true }` — the idle arm tripped | `error_class: "stream_truncated"`, message prose |
| `StreamRead::Failed { timed_out: false }` — a transport/decode error mid-body | the same `error_class`, the same prose |
| `request_deadline` passed — **our own** outer bound | the same `error_class`, the same prose |
| `StreamRead::Ended` **before the wire's terminal event** | **nothing** — `record_terminal(…, None)`, `stream_completed: true`, `errors[]` empty |

The last row is the sharp one, and it is not a matter of taste: the completeness fact is **already
computed and thrown away**. `SseUsageExtractor` carries `terminal_seen: bool`
(`crates/vadis-protocol/src/sse.rs:59`, set for `[DONE]`, the `responses` terminal events and
`message_stop`, `:202`/`:221`/`:248`), and `finish()` already branches on it (`:114-116`). The relay
never asks. So a stream that ends cleanly one event early is written as a **completed** request with no
usage, and no operator can tell it from a healthy end.

**Vintage note, stated because it changes how the 106 are re-read.** Their message text
(`"… not retried: error decoding response body"`) is the **transport-error** arm's prose, not the idle
arm's (`"idle bound exceeded: …"`) — consistent with a body whose read was cut by reqwest's per-request
`.timeout` at the knob's instant, i.e. by the clock ADR-053 removed (the deployed binary predates it: the
audit's own F6 records that the live process runs a build older than the R73 merge). The **bound** that
killed them was the attempt budget either way — `overhead_ms ≈ 120 s` is the measurement of it — and that is
F1's substance. The **cause word** R74 introduces is what a *new* record will carry; §2.3 keeps the bucket
predicate valid for both vintages so the 13-day window can be recounted in one pass, and `R74-0-F3`
registers that the *cause split* of those 106 is not recoverable from the record.

### 1.3 Why this is a contract, and why now

F1 is a knob's meaning; F2 is an aggregation invariant; F4 is a state write. Left implicit, each of the
three would be implemented three times and disagree — which is exactly what happened between the event
log and the dashboard. The owner's round ruling splits the work the way this register always does: the
contract is reviewed on its own (this card), the code lands against a frozen contract (`R74-1`), and the
three cases are parked in between so the suite is never red.

**R74 is the first of four rounds on the same audit** (F1/F2/F4 here; F5/F8, F6/F7 and F3 follow). This
ADR therefore **does not** touch F3: `overhead_ms_p99`'s definition is R77's ruling, and R74 makes only
the consistency changes its own figures force (§6.4).

---

## 2. The ruling

### 2.1 The new key: `server.stream_idle_timeout`

**One key, one meaning, and it is not the attempt budget.** The contract:

| Property | Value |
|---|---|
| Name | `server.stream_idle_timeout` |
| Bounds | the **maximum silence between two forwarded bytes** of a streamed response body — the per-read gap bound, and *only* that |
| Grammar | the config duration grammar (spec §9.2's own: `300ms`, `90s`, `15m`, `1h30m`; no `d` unit) |
| Default | **30 s** — a product default, not a measured optimum (§2.6) |
| Range | **1 s ≤ value ≤ 300 s**; outside the range the load is **refused** naming the key (the loader is the same gate the reload uses, spec §4.15 rule 1) |
| Read site | **once per request**, at the streaming driver's entry — the value the request drives with |

And the three bounds are now **named apart**, because two of them used to share a value:

| Bound | Key | What it bounds |
|---|---|---|
| head arrival | `server.upstream_attempt_timeout` | connect + how long the upstream may take to produce the response **head** (ADR-053 §2.1 item 1, unchanged) |
| the body's gap | **`server.stream_idle_timeout`** | **the silence between two forwarded bytes**; a gap past it ends the relay as a declared truncation (§2.2) |
| the whole request | `server.request_timeout` | the absolute deadline of the whole inbound request, head phase **and** relay — enforced on this path by its single owner (ADR-053 §3, unchanged) |

**The existing min() stays.** The relay's per-read window is `min(gap bound, time-until-deadline)` —
the outer bound still wins when it is nearer, and no read may sit past it. What changes is only the
first term.

**Connect stays with the attempt knob.** The streaming client's `connect_timeout` is the attempt budget
(ADR-053 §2.2's "connect window is unchanged"): the new key never bounds a connect, a TLS handshake or a
head.

**The relation to `request_timeout` is stated, not enforced.** A value at or above `request_timeout`
makes the key **inert** (the outer bound fires first). That is a fact, not a refusal: refusing a config
because a *default* exceeded another key's value would make an innocent file unstoppable.

### 2.2 Why this is an amendment to ADR-053 and not a new idea

ADR-053 §2.1 item 2 wrote the body's bound as *"a gap with no upstream bytes longer than
`server.upstream_attempt_timeout`"* — it restored R4's **construction** (one read at a time, one idle
arm) and, unavoidably, reused the attempt knob's **value** to express it. This ADR keeps the
construction verbatim and moves the value to the key that names it. Nothing else in ADR-053 moves: the
head-arrival wrap, the conservative `unknown_outcome` arm for a slow head, the single-owner enforcement
of `request_timeout`, and `CONF-105`'s allocation all stand.

### 2.3 A truncation names its cause: `details.stream_end`

Today every mid-body truncation writes the same entry. From R74-1, the entry is:

```json
{"kind": "upstream_error",
 "message": "stream truncated before its terminal event; not retried: <the transport's own words, or ours>",
 "details": {"error_class": "stream_truncated", "stream": true,
             "stream_end": "idle_timeout",
             "relayed_bytes": 4096, "window_ms": 30000, "since_last_byte_ms": 30104}}
```

`details.stream_end` is the **closed vocabulary** (one of exactly three, never null when the entry is
present):

| `stream_end` | The cause | Produced when | `window_ms` present? |
|---|---|---|---|
| `idle_timeout` | **the upstream went quiet** | the read's idle arm tripped with the connection still open | **yes** — the window that was in force for that read |
| `upstream_ended_early` | **the upstream closed first** | the body ended (a clean EOF, or a transport error) **before the wire's terminal event** | **no** — no bound tripped |
| `request_timeout` | **our own outer bound** | `server.request_timeout`'s deadline was reached | **yes** — the effective window of the tripping read |

and the three observables that make the causes checkable, each **always present on the entry**:

| Key | Value |
|---|---|
| `relayed_bytes` | the number of body bytes this request **wrote through to the client** (a count; the event's own `bytes_relayed` boolean keeps its meaning — *did anything go out* — and is not retyped) |
| `window_ms` | the window that was **in force** for the tripping read, in integer ms — the `min()` above, never the configured key alone. Present iff a bound tripped (the table above) |
| `since_last_byte_ms` | the interval from the last byte received from the upstream to the verdict, integer ms; when no byte was ever received, measured from the head's arrival. Always present |

**The completeness rule that makes `upstream_ended_early` reachable.** A stream is **complete** iff the
wire's own terminal event was seen (`[DONE]` for `chat`, the `responses` terminal events,
`message_stop` for `anthropic` — `SseUsageExtractor::terminal_seen`, already computed). A clean body end
without it is a truncation, not a completion; §1.2's last table row is the defect it retires. The event's
`upstream.responded` payload keeps `stream_completed` as that same predicate and gains `stream_end`
(present iff truncated).

**Why this vocabulary and not "silence vs not".** F4's consequence must be able to target **silence**
alone: `upstream_ended_early` is the shape an upstream's *graceful* early close takes (a provider-side
content cut, a proxy's own idle policy, a client-side cancel arriving as a close), and punishing it with
the same instrument as a stall would make the breaker fire on a route that is answering. The two causes
therefore differ by **which bound tripped, if any** — a fact the relay already holds, not a
classification of prose.

**Vintage.** A record written before R74-1 carries `error_class: "stream_truncated"` and **no**
`stream_end`. A reader takes the missing key as *"the cause was not recorded"* (the additive-vintage rule
`config_digest` uses), never as `idle_timeout`. The **bucket** predicate (§2.4) does not depend on it,
which is what lets one window hold both vintages.

### 2.4 One record, exactly one bucket

`/metrics` and `vadis stats` counted a truncated-but-2xx record as a **success** because success was
`2xx`. It is not: the client received a head and a partial body. The ruling:

**`requests` is partitioned by a three-way predicate, and the predicate is one definition owned in one
place** (spec §6's metric definitions; `/metrics` renders it, `vadis stats` prints it — ADR-041's
single-owner rule):

| Bucket | Predicate over one record |
|---|---|
| `failed` | `result.status` is **not** 2xx |
| `stream_incomplete` | `result.status` **is** 2xx **and** `errors[]` holds an entry with `details.error_class == "stream_truncated"` |
| `succeeded` | everything else (2xx **without** such an entry) |

The invariant, stated as the contract: **`succeeded + failed + stream_incomplete == requests`**, for any
window and any mixture of vintages.

**`usage_missing` is orthogonal — it is not a fourth bucket, and it may not become one.** The audit's 106
records satisfy `status = 200` **and** `errors[]` non-empty **and** `usage_missing: true` at the same
instant, so a contract that made `usage_missing` a bucket would have three, four or five mutually
exclusive bins depending on how the reader combined them, and `succeeded + failed + incomplete ==
requests` would contradict itself on the very records this round exists for. The ruling is therefore:

- the three buckets are a partition of **`requests`** (one bin per record, exhaustive);
- `usage_missing` is a **second, independent partition** of the same `requests` (a record either has a
  measurement or it does not), reported on its own line and *never* added to, subtracted from or
  intersected out of the three;
- a truncated record is counted **once** in `stream_incomplete` and, when its usage never arrived,
  **once** in `usage missing` — the second count says nothing about the first.

**What is *not* the discriminator, and must not be used as one: "`errors[]` is non-empty".** Three
existing success shapes carry a non-empty `errors[]` on a 2xx record — `transform_error` (spec §8: a
failing transform falls back to the original and the request is forwarded **as usual**),
`trace_write_failed` (a missing observation never blocks a request), and any future additive entry. Only
the class word `stream_truncated` moves a record out of `succeeded`, because only it states *the body
did not complete*. §5's `CONF-109` limb (c) is the witness that keeps that sentence honest.

**The metric.** One new series, `/metrics` (spec §4.16) and `vadis stats` (§9.2) in step:

| Series | Labels | Unit | Provenance | Value |
|---|---|---|---|---|
| `vadis_requests_stream_incomplete` | — | count | `count` | the bucket's count |

`vadis_failures_by_kind` stays the **`failed`** side's split by `errors[].kind`; the incomplete bucket has
one class, so it needs no label. **`result.status` does not change** — the client really did receive a
`200` head, and that fact is true; what moved is the aggregate. `usage missing`, `requests`, `failed`'s
own definition, every money line and every ratio are unchanged.

**Cost, stated.** `succeeded`'s value drops by the number of truncated records in the window (the audit's
rate: 106/7,036 ≈ 1.5 %) with **no behaviour change** — which is the point, and the reason the drop is a
*correction* rather than a regression. Any gate that reads `succeeded` sees that shift; no gate
definition, threshold, corpus or envelope value is edited here (AGENTS 9 / ADR-012), and §6.4 names the
comparison that becomes meaningless.

### 2.5 The silent route loses its **next** selection (and nothing else)

F4's question is narrower than "should a stall demote a provider". ADR-011 item 4 fixes the demotion
**unit** for billing-class failures (*the provider, not the credential*), and the code's own summary of that
item is the door this section walks through: `demotes_provider()`'s doc comment reads *"only quota/billing
exhaustion is a fact about the account; every other failure is a fact about the attempt **or the route**"*
(`crates/vadis-core/src/error_class.rs:77-82`). The route-scoped half of that sentence has never been
written. This is it — one row, one short window, **selection-level only**:

| Property | Ruling |
|---|---|
| Scope | **`route`** — `provider_cooldown.scope = "route"`, `model` = the route's **native model id** (never an alias, never `""`) |
| Reason word | **`stream_silence`** — the row's `reason`, and the discriminator that makes it *advisory* |
| Trigger | **one** truncation whose `details.stream_end == "idle_timeout"` (K = 1, §2.5.1) |
| TTL | **60 s** — a code constant, not a key (§2.5.2) |
| Where written | two sites, one source: the `demotion` object inside the `error.classified` payload, and the `provider_cooldown` projection write **derived from that same object** (so `rebuild_cooldown` reproduces the row exactly — CONF-21's identity) |
| What it does | the walk **prefers** an eligible candidate whose route is not silence-cooling, and leaves a cooling one only when such a candidate exists |
| What it never does | it **never refuses a request**, never appears in `skipped[]`, and never produces `no_available_route` (§2.5.3) |
| What it does not touch | `QuotaExhausted`'s semantics, `demotes_provider()`'s answer for every existing class, the probe gate's `primary_cooling_down` (`/health`'s `blocked_by`, which keeps asking the **provider-wide** question, §2.5.4), and every buffered-path behaviour |

**§2.5.1 Why K = 1.** A "consecutive K" rule needs a streak counter, and a counter is **state the
event log cannot rebuild**: `rebuild_cooldown` reconstructs the projection from the `demotion` objects
alone (CONF-21 asserts projection == rebuild row by row), so a streak held anywhere but in an event
payload would make the two paths disagree by construction. K = 1 keeps the mechanism a pure function of
one record. The anti-flap property a streak would buy is bought instead by the **short TTL**: a route
that keeps stalling re-arms its own row on every stall, and one that recovers is out of the window within
a minute. The audit's own numbers make this the honest choice: the failures were *not* bursty-but-single
(97 truncations across three days on one route), so a counter would never have saved a request a
one-stall rule does not.

**§2.5.2 Why the TTL is a constant and not a key.** A key would owe three things this round has no basis
for: a default (no measurement distinguishes 30 s from 120 s here), a reload story (the value is read on
the request path, so it would be reloadable, and the refusal list would grow a second exception), and a
`vadis setup` askable-key decision. The audit's recommendation was *"a short window, e.g. 60 s"*; the
contract takes exactly that and spends no knob. If the owner later wants it tunable, that is an additive
key with its own ADR — registered as `R74-0-F2` (§9).

**§2.5.3 Why advisory, and why that is not a loophole.** A **binding** route cooldown (today's shape for
every existing demotion) can end a chain: when every candidate is cooling, the walk answers §8's
`no_available_route` refusal. For a *billing* verdict that is right — the account cannot serve. For a
*stall* verdict it would be a self-inflicted outage: a one-route deployment whose upstream hiccups twice
would answer `502` for a minute rather than attempt a route that was, 97 times out of 100, perfectly
healthy. So the row steers and never refuses: **`stream_silence` is the one `reason` word whose row may
not be the sole cause of a refusal.** The distinction is a value on the row (observable in
`provider_cooldown`, in the `demotion` object and in `/state/events`), not an unstated convention.

**§2.5.4 Why the probe gate is deliberately excluded.** `probe.admitted` / `probe.blocked_by:
"primary_cooling_down"` (`/health`, spec §9.1) decide whether a **family's account** may return to its
plan. That is a much larger act than "prefer another route for the next request", and a 60 s stall
preference is not evidence about an account. The probe keeps asking the provider-wide question it asks
today (`Query::Cooldown { model: None }`), and a `stream_silence` row is invisible to it. Stated as a
boundary so a later reader does not "fix" the asymmetry: it is the design.

**§2.5.5 The read side already supports it.** `provider_cooldown` has carried `scope` and `model` since
the store's first DDL, `Query::Cooldown { model: Some(m) }` already answers *"the union of the
provider-wide and route-scoped rows, whichever blocks the route now"*
(`crates/vadis-store/src/lib.rs:861-873`), and `rebuild_cooldown` already reads `scope` / `provider` /
`model` / `until_us` / `reason` from the payload. What has never happened is a **writer** that emits a
route-scoped row and a **caller** that passes the model. Both are one line each; neither invents a
concept.

### 2.6 What a healthy stream sees: zero

The byte path is untouched: the relay's per-chunk write-through, the tap, the ordering and the terminal
rules are all unchanged (AGENTS constraint 1; §12.10.3 R1/R2/R7). What narrows is **the wait**: a stream
whose every gap is under the new key behaves exactly as it does today, byte for byte and record for
record. A stream that stalls now fails at the new bound instead of the attempt budget — earlier, and in
the same declared shape (R6: no fabricated terminal, no `[DONE]`, one `errors[]` entry).

**The default's honesty.** 30 s is a product choice: half the shipped attempt knob (60 s), inside the
band the audit recommended, and far above the millisecond-to-second gaps a healthy stream shows. This
ADR does **not** claim a measured gap distribution — no such measurement exists in this window, and
inventing one would be the fabricated number AGENTS constraint 4 forbids. `R74-1` records the gap
distribution its rig exercises, and if a default change is ever wanted it is that round's evidence, not
this paragraph.

---

## 3. Blast radius, in both directions

**Gained.** A stalled stream ends in 30 s instead of 120 s (the audit's 3.5 hours of pure waiting becomes
~52 minutes, before any client-side retry is counted — arithmetic on the measured 106 × 120 s, not a
forecast). A cut stream is counted as incomplete, so the dashboard stops overstating availability by the
truncation rate. A route that has just stalled is de-preferred for the next minute, so a repeating
provider fault can move traffic instead of holding it.

**Paid.** `succeeded` drops by the truncation rate with no behavioural regression (a correction, §2.4).
A **cleanly early-ended** stream that today reads as complete now reads as incomplete — a real change of
recorded outcome for a real fault (§2.3). And a config file that today loads will, after R74-1, apply a
30 s gap bound it never asked for: the one value that moves without an operator writing anything, which
is why §4.15's reload clause and the `book/` sentence (§6.3) exist.

**Unchanged, deliberately.** No relayed byte; no `result.status`; no `error.type` or HTTP status (§8's
table untouched — a truncation remains a `200` head with an `errors[]` entry); no `schema_version` move
(`details` keys are additive on an `Option<Value>`); no money, no ratio, no `usage`; no buffered-path
timeout; no `QuotaExhausted` behaviour; no `overhead_ms_p99` definition (R77's).

---

## 4. The cases, and what each limb observes

Each case runs through the real `serve` assembly over loopback mocks (the existing testkit), reads the
trace JSONL, the event log and `/metrics` as a stranger would, and asserts **values**, never shapes.

### 4.1 `CONF-108` — the gap bound is its own key, and a truncation names its cause

| Limb | Rig | Observation point | Asserted value | What turns it red |
|---|---|---|---|---|
| **(a) silence is cut at the new key** | mock: `200` head + one SSE event, then the socket held open; config `stream_idle_timeout: 400ms`, `upstream_attempt_timeout: 3s`, `request_timeout: 8s` | elapsed time of the client call; the one trace record | elapsed **≥ 400 ms and < 3 s** (the attempt knob did not fire); `details.stream_end == "idle_timeout"`; `details.window_ms == 400`; `details.relayed_bytes` equals the bytes of that one event; `details.since_last_byte_ms ≥ 350`; `upstream.responded.stream_completed == false`; **no** `[DONE]` in the client's body | the base tree (waits 3 s → elapsed ≥ 3 s, no `stream_end`); an implementation that keeps reading the attempt knob; one that reports `window_ms` as the configured value while the deadline clamped it |
| **(b) a clean early end is a different cause** | mock: canned SSE whose body **ends** after one event, no terminal event, connection closed normally | the one trace record | `details.stream_end == "upstream_ended_early"`; **no** `window_ms` key; `since_last_byte_ms` present; `relayed_bytes` equal to what was relayed; `errors[]` non-empty with `error_class: "stream_truncated"` | the base tree, which records this as **complete** (`stream_completed: true`, `errors[]` empty); an implementation that collapses the two causes into one word |
| **(c) a healthy stream is untouched, and the new key does not bound the head** | mock: head delayed **800 ms** (under the 3 s attempt knob, **over** the 400 ms idle key), then six events 50 ms apart and `[DONE]` | the client's bytes; the one record | body **byte-identical** to the mock's (via `dechunk`), ends with `[DONE]`; `errors[]` empty; `stream_completed == true`; the record carries **no** `stream_end` | an implementation that applies the new key to the head phase (the 800 ms head would be refused/truncated) or that mistakes a busy stream for a stalled one; and any change to the relayed bytes |
| **(d) reload reaches the next request, not the one in flight** | start with `stream_idle_timeout: 4s`; request #1 drives a silent upstream; while it is open, rewrite the pair to `400ms` and wait for `/health`'s `config_digest` to move; then request #2 | the two records' `details.window_ms` and elapseds | #1 ends at ≈ 4 s with `window_ms == 4000`; #2 ends at ≈ 400 ms with `window_ms == 400`; the reload was **applied** (the digest moved — not a refusal) | the base tree (the key does not exist: the config is **refused** at load); an implementation that rebuilds the streaming client per read (it would re-read the key mid-flight and #1 would trip at 400 ms); an implementation that adds the key to §4.15's refused list |

### 4.2 `CONF-109` — one record, exactly one bucket

| Limb | Rig | Observation point | Asserted value | What turns it red |
|---|---|---|---|---|
| **(a) the partition** | three requests in one window on one process: a clean `[DONE]` stream (2xx), an explicit failure (a streamed upstream `500` refused — non-2xx), and a silent-head-then-body-cut stream (2xx with a truncation entry) | `GET /metrics` | `vadis_requests == 3`, `vadis_requests_succeeded == 1`, `vadis_requests_failed == 1`, `vadis_requests_stream_incomplete == 1`, and the sum equals `vadis_requests` | the base tree (there is **no** incomplete series, and `succeeded == 2`) |
| **(b) `usage_missing` is orthogonal** | the same run: none of the three carried a measurement (chat carries usage only when the client asks for it) | `/metrics` and the three records | `vadis_requests_usage_missing == 3` **and** the three buckets unchanged by it; the truncated record is counted in **both** `stream_incomplete` and `usage_missing` (its own record's `usage_missing: true` is asserted); the `vadis stats` equality is `CONF-87`'s (its aggregator adopts the same predicate under §5 A3) | an implementation that makes `usage_missing` a fourth bucket (the sum then misses the truncated record); one that excludes `usage_missing` records from the buckets; two derivations that disagree |
| **(c) `errors[]` non-empty is not the discriminator** | a clean, byte-complete stream that carries **no usage** (no `stream_options.include_usage`, no usage carrier) — `[DONE]` seen, so the stream is complete, and `usage_missing: true` | the one record and `/metrics` | the record has `errors[]` **empty** and `usage_missing: true`; it is counted in `succeeded` **and** in `usage missing`, and **not** in `stream_incomplete` | an implementation that buckets on `usage_missing` or on "`errors[]` non-empty", which would file this healthy request as incomplete |

### 4.3 `CONF-110` — the short, route-scoped, advisory row

| Limb | Rig | Observation point | Asserted value | What turns it red |
|---|---|---|---|---|
| **(a) one silent truncation leaves one route-scoped row** | two roster routes (`p1/m1`, `p2/m2`), `fallback: [p2/m2]`; `p1`'s mock answers a head + one event then holds the socket open; the idle key is short | after `serve` stops: `Query::Cooldown { provider: "p1", model: Some("m1") }` and `{ model: None }`; the `error.classified` event | a row exists with `scope == "route"`, `model == "m1"`, `reason == "stream_silence"`, and `until_us − <the event's own ts_us>` is 60 s (asserted with a 0.5 s tolerance: the two values are stamped by two calls inside one request); the **provider-wide** query answers **no row**; the `demotion` object equals the row's own `until_us` (one value, two readers) and carries `{scope: "route", provider: "p1", model: "m1", until_us, ttl_s: 60, reason: "stream_silence"}` | the base tree (`provider_cooldown` stays empty — the audit's own sighting); a provider-wide row (the QuotaExhausted shape misapplied); a TTL that is not the constant; an incremental write the log cannot rebuild (different fields from the payload's object) |
| **(b) the next request walks away — and only the next one's selection moves** | immediately after (a), a second request resolving to `p1/m1` | the two mocks' recorded request counts; the second record | `p1`'s mock sees **no** second request, `p2`'s sees exactly one; the second record is a `200` served from `p2`, with `result.failover_from == "p1/m1"` and **no** `failover.triggered` row (ADR-024's skip shape) | an implementation that writes the row but never reads it (the walk still hits `p1`); one that narrates a displacement it did not make (a `failover.triggered` row, or no `failover_from`) |
| **(c) `QuotaExhausted` is unchanged, side by side** | a streamed `403` carrying the quota wording on `p1` (a two-route rig with `fallback: [p2/m2]`, **no** plan policy — the taxonomy's own arm, not the plan walk's) | the `demotion` object and the two cooldown queries | the demotion's `scope == "provider"` with `model == ""` and a reason word that is **not** `stream_silence`; the provider-wide row exists (`scope: "provider"`, `model: ""`) and it shadows the route; the request is still served (by `p2`) | an implementation that widens `demotes_provider()` instead of adding a route arm, or one that files billing exhaustion under the new route-scoped reason |
| **(d) advisory means never a refusal** | one route only (`p1/m1`, empty `fallback`), a silence row live for it (written by limb (a)'s mechanism on this rig) | the client's status and the record | the request is **served** (`200`), the record carries **no** `no_available_route`, and no `skipped[]` entry names the cooling route | an implementation that treats the silence row as binding: a `502` with `details.stage: "no_available_route"` (`skipped[].reason: "demoted"`) — the availability cliff §2.5.3 refuses |

**Expiry is asserted as arithmetic, not as a 60-second sleep.** Limb (a) pins the row's own bound
(`until_us − ts_us == 60 s`) and its live-ness at the answering instant (`until_us > now`); the read
predicate it must satisfy is the store's existing `until_us > ?now`. A minute of wall clock to observe
`t > until_us` would slow the case by a minute to re-assert a comparison the row already carries, and it
is stated here rather than hidden in the fixture.

---

## 5. The conformance amendment list (frozen, owner-authorised)

Three existing cases cannot stay byte-identical once this contract lands, and **an assertion is not a
card's to edit** (AGENTS constraint 9). The owner's round ruling authorises exactly the edits below; the
list is frozen here so `R74-1` re-decides nothing. Every other case file stays **byte-identical**, and if
`R74-1`'s suite run turns one red, that is a **finding for the owner** (recorded, not repaired under this
authorisation).

| # | File | Edit | Why |
|---|---|---|---|
| **A1** | `tests/conformance/tests/conf_105_attempt_bound_is_head_arrival.rs` — limb (d) only | the fixture config gains `stream_idle_timeout: 2s` (equal to its attempt knob, so the rig's story is unchanged), and the limb adds one assertion: the truncation entry's `details.stream_end == "idle_timeout"` | limb (d) asserts *the idle arm fires at the attempt knob*. After §2.2 the idle arm reads the new key, so the limb's **fixture** must name it; without the key the rig would idle for the 30 s default and the limb would be asserting a different bound than it says |
| **A2** | `tests/conformance/tests/conf_41_reporting_surfaces.rs` — limb (c)'s partition assertions only | the independent computation becomes the **three-bucket** one (spec §6's predicate), and `failed` stops being `len − succeeded` and becomes the non-2xx count; the sum invariant is asserted | the case's own job is to recompute the contract independently; a two-way partition is the old contract. Its rig holds no truncated record, so the numbers do not move — only the assertion's statement of the rule |
| **A3** | `tests/conformance/tests/conf_87_metrics_single_owner.rs` — `FROZEN_SERIES` and its own aggregator | the frozen set gains `vadis_requests_stream_incomplete` (21 → **22**), and the case's independent aggregator adopts the three-bucket predicate | spec §4.16's series set is a **frozen list** asserted by name; a new series is a contract change, and the case that freezes the list must be moved by the same change that adds it (otherwise the case fails the moment the series appears) |

**Not amended, and why not:** `CONF-30`'s and `CONF-13`'s truncation limbs use a mid-stream **abort**
(a transport error), which `CONF-108` limb (b) reclassifies only in its *cause word* — both limbs assert
`error_class == "stream_truncated"`, which does not move. `CONF-82`'s failure-head limbs use aborts, not
stalls. `CONF-90`'s limbs end with `[DONE]`. A corpus read at this card's HEAD found **no** completeness-
asserting fixture that ends before its wire's terminal event except the ones named above.

---

## 6. Landing plan — what `R74-1` owes, and only this

### 6.1 The code (one change, these files)

| File | Change |
|---|---|
| `crates/vadis-core/src/config.rs` | `ServerCfg.stream_idle_timeout` (default `30s`, range `1s..=300s`, load refusal naming the key), the example/embedded pair in step |
| `crates/vadis-providers/src/stream.rs` | the client carries **two** windows (`head_timeout` = the attempt budget, `idle_timeout` = the new key); `read_chunk` keeps its signature; `open` keeps the head wrap. The client stays **per request** (it is today, `stream_forward.rs:891-892`) — hoisting it to a startup-built object would silently make the key reload-refused |
| `crates/vadis-proxy/src/stream_forward.rs` | the driver reads the new key **once** at entry; `st.relayed` becomes a byte counter; `record_classified` / `record_terminal` write `stream_end`, `relayed_bytes`, `window_ms`, `since_last_byte_ms`; the `Ended` arm asks `terminal_seen` before declaring completeness; the truncation arm emits the `demotion` object and lands the projection |
| `crates/vadis-protocol/src/sse.rs` | expose `terminal_seen` (it exists; `finish()` already branches on it) to the relay |
| `crates/vadis-proxy/src/forward.rs` | the demotion object's **one shape** (`scope, provider, model, until_us, ttl_s, reason` — §6.2), and the walk's per-candidate availability read passes the model (`Query::Cooldown { model: Some(..) }`) with the advisory rule |
| `crates/vadis-core/src/error_class.rs` | the `Demotion` doc comment ("this round writes no route-scoped cooldowns") — comment only; **`demotes_provider()` is not touched** |
| `crates/vadis-cli/src/stats.rs`, `src/metrics.rs` | the three-bucket predicate in **one** function; the new line/series |
| `config.example.yaml` + the binary's embedded copy, `setup/sections.rs` if the key joins the askable set | the key with its default and a one-line comment. **Recommendation: not askable** — the wizard's set is bounded by §4.11 and the key has a good default; adding it is a separate §4.11 change |

### 6.2 The `demotion` object's one shape (a repair R74 forces, stated so it is not hidden)

`forward.rs:2339-2344` writes `{"scope", "ttl_s"}` while `rebuild_cooldown` reads
`{"scope","provider","model","until_us","reason"}` (`vadis-store/src/lib.rs:544-572`) — so a cooldown
rebuilt from the log today lands `until_us = 0`, i.e. **expired on arrival**, and the live projection and
the rebuild disagree (**registered finding `R74-0-F1`**, §9). One writer emits both the new arm and the
existing one, so the shape is fixed **once**, for every demotion: `{scope, provider, model, until_us,
ttl_s, reason}`. This changes **no decision**: `QuotaExhausted` still demotes provider-wide, still takes
its TTL from `Retry-After` or the declared default, and `demotes_provider()` still answers `true` for it
and `false` for every other class.

### 6.3 The documents and the user-facing sentence

`docs/spec.md` §4.20's status line flips *contracted, not served* → *served*; `DESIGN` §12.9's register
line records the landing; `DESIGN` §12.8's three rows are unchanged (an ID is a contract);
`config.example.yaml`'s comment and **`book/` gain the sentence an operator needs** — *"a streamed
response may go silent for at most `server.stream_idle_timeout` (30 s by default) between bytes; past
that the stream is ended and recorded as incomplete, and the request is not a success"* — in the chapter
that owns the cost/behaviour levers, **beside the existing `server:` block example
(`book/connecting-clients.md:263`), which gains the key** so the user-facing snippet is not a file the
loader would refuse. This card's write set is `docs/`, `design/` and `tests/` by the round's own
boundary, so both are `R74-1`'s to land (AGENTS constraint 8).

### 6.4 What `R74-1` must **measure**, and what it must not claim

1. **Before/after on one rig**: the 30 s (new default) vs 120 s (the live attempt knob) silence-to-
   truncation interval, the bucket counts, and the two mocks' request counts for `CONF-110`'s walk-away.
2. **The 13-day window, recounted honestly**: the three buckets over the existing trace corpus are
   computable (the predicate does not need `stream_end`), and the **cause split of the 106 is not** —
   those records carry no `stream_end`, and their message prose names a body-decode carrier. The round
   reports the recount as a *bucket* statement and says the causes are unrecorded (**`R74-0-F3`**, §9).
3. **Not measured, and said so**: the F4 effect on the live deployment is unobservable in the existing
   trace (no cooldown rows ever existed). Whether the live roster's chain actually moves a request away
   from a cooling route is a **config-shaped** fact (`responses` traffic on a plan route has at least one
   chain entry declaring that protocol, per the roster's own `supports`), and the round asserts it on a
   **rig**, then reads the live window after deployment (**`R74-0-F4`**).
4. **`overhead_ms_p99` is not R74's**: the definition stays as §6 writes it, truncated records still
   inflate it, and **no comparison of that figure across R74 is meaningful** — F3 is R77's ruling.

---

## 7. Alternatives considered

| Alternative | Why rejected |
|---|---|
| keep one knob and lower it (120 s → 30 s) | that is the defect: the attempt budget would then bound a **head** at 30 s, refusing slow-but-legitimate first tokens, and a body's gaps at 30 s. Two meanings cannot share one value; the audit's F1 is precisely the observation that they were |
| make the new key bound the **whole stream** (another total-elapsed ceiling) | that is ADR-044's clock, which ADR-053 superseded after measuring it truncate busy streams. A stream may legitimately run for minutes as long as it keeps talking; `request_timeout` is its outer bound |
| a **boolean** "silence vs not" in `details` | the two causes must be distinguishable *and* a third exists (our own deadline). A boolean forces `request_timeout` into one of the two bins, which is a false statement in one of them |
| infer completeness from `usage_missing` | usage absence has several causes (a client that did not ask for usage, a protocol with no carrier, a real truncation). `terminal_seen` is the wire's own fact and is already computed (§1.2) |
| make `usage_missing` the fourth bucket | it would break the partition on the very records this round exists for (§2.4). It is a dimension, not a bucket |
| drop truncated records from `requests` | an invisible request is worse than a mis-filed one, and the client did receive a response |
| change `result.status` to a non-2xx for a truncated stream | the client really did receive `200` and a partial body; rewriting the status would make the trace disagree with the wire (AGENTS constraint 3's honesty, and CONF-28's one-line rule) |
| a **provider-wide** cooldown for silence | the audit's 3 % is not an account fact; a provider-wide window would take the account's other models (and the plan family) out on a route's bad minute — ADR-011 item 4's own reason for keeping demotion narrow |
| a **binding** route cooldown | the availability cliff of §2.5.3 |
| a configurable TTL for the silence window | §2.5.2: a key needs a default nobody measured, a reload story and a wizard decision |
| a consecutive-K trigger | §2.5.1: it needs state the event log cannot rebuild |
| put the silence counter in a new store table | same reason: `rebuild_cooldown` must reproduce every projection row from the events alone (CONF-21) |
| land the surface and the contract in one change | the owner's split: the contract is reviewed alone (`R74-0`), the code lands against it (`R74-1`), and the cases are parked in between so the suite is never red (the `CONF-20…25` rule) |

---

## 8. Register

### 8.1 IDs spent

| ID | What it is |
|---|---|
| **`ADR-056`** | this file |
| **`CONF-108/109/110`** | the three cases of §4; their files land with this card, parked `#[ignore = "CONF-10N: depends on R74-1"]`; `DESIGN` §12.8's heading moves `CONF-01…CONF-107` → `CONF-01…CONF-110` |
| **spec `§4.20`** | the new section (the round's reserved number; `§4.19` is R73's `GET /v1/models`) |

### 8.2 What this ADR adds to the documents

- `docs/spec.md`: **§4.20** (the whole contract), **§4**'s schema block (the key + its comment),
  **§4.2**'s classification-evidence clause (the gap bound's key), **§4.15** (one clause: the new key
  travels with the revision; the attempt knob stays refused), **§4.16** (one series row + the provenance
  word), **§6** (the cause vocabulary, the completeness rule, the bucket predicate and the orthogonality
  rule), **§9.2** (the new figure, `succeeded`/`failed`'s stated definitions, the orthogonality
  sentence).
- `design/DESIGN.md`: **§12.8** (heading, three rows, the allocation paragraph), **§12.9** (the round's
  register line), **§12.10.3 R4** (the gap bound's key), **§12.6** (one additive bullet: the `details`
  keys, `schema_version` stays 2), **§12.21** (the new series and the amendment A3).
- `ADR-053`: a **dated note** (§8.3). `ADR-011`: a **dated note** (item 4 fixes the demotion *unit* for
  billing classes and the code's own summary of it names *the attempt or the route*; the route-scoped
  consequence is what this ADR writes, and `demotes_provider()` is untouched).
- `tests/conformance/tests/conf_108…110_*.rs`: new, parked.

### 8.3 The dated note this ADR owes ADR-053 (the shape it takes on that file)

> **Dated note (2026-10-17, R74-0 — append-only).** §2.1 item 2's clause *"a gap with no upstream bytes
> longer than `server.upstream_attempt_timeout`"* is **superseded as to the value only**: the gap bound's
> construction is unchanged (one read at a time, one idle arm inside `read_chunk`), and the value it
> reads is now `server.stream_idle_timeout` (default 30 s, `design/decisions/ADR-056-the-silence-of-a-
> stream.md` §2.1). The head-arrival ruling (§2.1 item 1), the conservative `unknown_outcome` arm (§2.2),
> the `request_timeout` enforcement and its single owner (§3), the blast radius (§4), the measurement
> obligations (§5) and the `CONF-105` allocation (§4, §8.3) all stand as written. §2.1 item 3's outer
> bound is untouched and still wins when nearer (`min(gap bound, time-to-deadline)`).

---

## 9. Findings and gaps this card registers

| # | Class | Site | Effect | Owner | Due |
|---|---|---|---|---|---|
| **`R74-0-F1`** | inconsistent-projection | `crates/vadis-proxy/src/forward.rs:2339-2344` (the `demotion` payload writes `{scope, ttl_s}`) vs `crates/vadis-store/src/lib.rs:544-572` (`rebuild_cooldown` reads `scope/provider/model/until_us/reason`) | a `provider_cooldown` row rebuilt from the event log lands `until_us = 0`, i.e. **expired on arrival**: live projection and rebuild disagree, which CONF-21's identity forbids. Not observable in the audited window because no row was ever written (`QuotaExhausted` never fired). **Repaired by `R74-1`** as a forced consequence of §2.5's landing (§6.2); no decision moves | `R74-1` | this round |
| **`R74-0-F2`** | registered-open | the 60 s silence cooldown is a constant | a deployment with a slower or faster upstream cannot tune it. Deliberate (§2.5.2); the trigger for a key is an owner ruling plus a measurement that distinguishes two TTLs | human | a later round |
| **`R74-0-F3`** | unrecorded-cause | the 106 audited records carry `error_class: "stream_truncated"` with no cause word (pre-R74 vintage) | the **bucket** recount over the 13-day window is exact; the **cause split** is not recoverable — the prose names a body-decode carrier for all of them. `R74-1` reports the recount as a bucket statement and says the causes are unrecorded | `R74-1` | this round |
| **`R74-0-F4`** | unmeasured-effect | F4's live effect (does traffic actually move?) | unobservable on the existing trace: no cooldown row ever existed, so there is nothing to correlate. `R74-1` asserts the mechanism on a rig (`CONF-110` limbs b/d) and the live window is read **after** deployment | `R74-1` | this round |
| **`R74-0-F5`** | registered-bounded | `/health`'s `probe.blocked_by: "primary_cooling_down"` does not see a `stream_silence` row | deliberate (§2.5.4): a 60 s selection preference is not evidence about an account. A reader must not read the asymmetry as a bug | human | if a round revisits the probe gate |
| **`R74-0-F6`** | registered-out-of-scope | `overhead_ms_p99` still measures the model's generation time for streamed requests (the audit's F3) | R74 changes the bucket, not the figure; the truncated records keep inflating it, so **no comparison of that figure across R74 is meaningful** | `R77` | R77 |

---

## 10. What a future reader must not conclude

- **Not** *"the idle bound was removed."* It exists, it is 30 s by default, and it is now *its own* key
  instead of the attempt budget's value.
- **Not** *"the attempt knob got shorter."* Its value and its meaning (connect + head arrival) are
  untouched; `CONF-105` still asserts the head-arrival clock.
- **Not** *"a truncation is now a failure status."* It is still `200` + an `errors[]` entry; what moved is
  the **aggregate** (§2.4). A client sees the same bytes and the same status.
- **Not** *"`usage_missing` became a bucket."* It is a second partition, and the two are never added.
- **Not** *"a cleanly-ended stream is now penalised."* `upstream_ended_early` is recorded as incomplete
  and **never** orders a cooldown; only `idle_timeout` does (§2.5).
- **Not** *"silence demotes the provider."* The row is `scope: "route"`, TTL 60 s, advisory, and
  invisible to the probe gate. `QuotaExhausted` remains the only provider-wide demotion.
- **Not** *"a silence row can refuse a request."* It cannot — that is the availability cliff §2.5.3
  exists to avoid.
- **Not** *"R74-0 changed a product byte."* The diff is `design/`, `docs/` and three parked case files;
  `crates/`, `config.example.yaml`, `book/` and `README.md` are untouched.
