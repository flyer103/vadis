# ADR-060 — a request cannot vanish, a refusal is diagnosable, and a cut stream is retried only where the wire and the bill allow it

- Status: **accepted (2026-10-24), contract only.** It amends `docs/spec.md` and writes **no** product byte,
  **no** test, **no** conformance case, **no** price and **no** gate/threshold/corpus (AGENTS constraint 9:
  `tests/conformance/` and the measurement are outside the mutable scope, and every sentence below is chosen
  so that no pinned assertion has to be edited for it).
- Kind: three decisions about the **observation medium** (the trace is the product, constraint 3) and one
  about **recovery** (§4.2's walk). One ADR because they share one question: *what does the process owe a
  reader when a request ends badly or does not end at all?*
- Authority: the owner's card of 2026-10-24, three rulings, answering three measured findings of one week of
  production traffic — 12 requests with no terminal record anywhere, 2 clusters of deterministic refusals
  whose `details` carries only vadis's own summary, 3 streams cut after the head was forwarded.
- Amends: `docs/spec.md` §6 (the record: one new class, one new field, one new `details` member and the kind
  vocabulary), §4.2 (the bounded same-route retry and `details.retry_refused`), §4.16 (one paragraph: what
  `/metrics` does and does not carry of this), §4.20.3 (a fourth `stream_end` value), §4.20.4/§4.20.5 (its
  bucket and the cooldown it must not write).
- Related: **ADR-005** (the trace is the only product → analysis channel), **ADR-010 item 4** (an intent
  without a response is an `unknown_outcome`; a closure event is never fabricated), **ADR-011 item 6** (the
  not-billed evidence table this ADR's retry rule obeys) and item 9 (a failover's re-prefill is a priced
  cost), **ADR-009 item 3** (bodies are never persisted *in the store*), **ADR-053/ADR-056** (the three
  bounds and `stream_end`'s closed vocabulary), **ADR-058** (one word, one meaning).

## 1. The three decisions

**D1 — a request that reached the process has a terminal row, and the row says which kind of unknown it is.**
The word for the class is `unknown_outcome`, which the contract already owns (DESIGN §12.10.3 R5 names a
client disconnect as exactly this case; §8's crash-window clause names the same word); what is missing is the
**row**. Two shapes, one class: the client closed before any response head existed (`result.status: 499`,
`errors[0].kind: "canceled"`), or it closed mid-stream after the head was forwarded (`result.status: 200`,
a `stream_truncated` entry whose `stream_end` is `client_closed`). The invariant is stated in §2.3 and it is
mechanical, not editorial: **at most one terminal row per request**, and the writer that produces it runs on
the cancellation path the shipped binary actually executes.

**D2 — a deterministic refusal carries a bounded, redacted slice of the upstream's own error object.**
Allowlisted fields only (`message` / `type` / `code` / `param`, and `error` nested one level), drawn from the
body **bytes** the classifier already reads, never from headers, never the whole body, redacted against the
process's own configured credentials plus a key-shape table, bounded per field and per capture, written
through the record's own JSON encoder so a row stays one physical line. This is a **response-side** record:
it touches no request byte and no forwarded byte, so constraint 1 is not engaged at all (§3.1).

**D3 — a truncated stream is retried, and the honest split is not "before/after the head arrived" but
"what is already on the wire" × "what is already on the bill".** Both must clear:

| | output already forwarded to the client | nothing forwarded |
|---|---|---|
| what a retry does | duplicates the client's output | produces no duplicate |
| the ruling | **never retried, structurally** — record, close, let the client redo the turn (§4.1) | retried **iff** ADR-011 item 6 says the attempt was not billed (§4.2) |

An attempt that may have been billed is an `unknown_outcome` and is **not** retried (ADR-011 item 6 row 3;
`WrittenNoResponse` already implements that on the buffered path and its `details.stage` already carries the
word) — so the retryable set is small, and that is the ruling, not a limitation to be worked around: a
same-route retry is allowed exactly where nothing was written **and** nothing was billed, one attempt, 250 ms
before it, and the record names both facts (§4.2).

## 2. D1 — the vanish

### 2.1 Why "the process may write it" is not enough for the alternative

The 12 rows are the residual of two mechanisms that were each correct on their own: ADR-010 forbids
fabricating a closure event, and the trace's single writer is only reached at a terminal outcome. A dropped
request future reaches neither. The alternative — reconcile the trace from the event log after the fact —
was rejected because the trace is read *without* the store (constraint 3; ADR-005) and because a reconciler
that invents rows from intents is the fabricated-closure rule with a new name.

### 2.2 The word, and why `result.status` stays an integer

`result.status` is a `u16` and stays one. The cancelled-before-head shape writes **499**, nginx's own
"client closed request": a word this format owns, **never written to a socket**, and never a status a client
received. It is not `null`, and that is a deliberate trade — a nullable `result.status` changes how *every*
existing reader reads the field (the `cost.currency` class that moved the version to 2), which would force
`schema_version: 3` and with it an edit to five pinned conformance assertions; the additive **value** changes
no type, so `schema_version` stays **2** and a v2 reader buckets the row into `failed` by the partition's own
existing predicate (`result.status` is not 2xx). That read is honest for this shape — no answer was produced
for the client — and `errors[0].kind` is what separates it from a refusal (§2.4).

One consequence is stated rather than hidden: **a cancelled request counts in `failed`**, so
`succeeded + failed + stream_incomplete == requests` keeps holding without amendment, and the availability
figure of a window that contains cancellations is lower than one that does not. A fourth bucket would be the
more precise shape and it is a **measurement definition** (constraint 9): it is named as owed work in §5, not
taken here.

### 2.3 The invariant, and where the writer lives

1. **One terminal row, at most.** The record's context is owned by one guard. The ordinary terminal writer
   **takes** it (`Option::take`) *before* it writes; the guard writes only if it still holds the context. Two
   rows for one request are therefore impossible by construction, not by discipline.
2. **The take comes before the write, deliberately.** If the ordinary write then panics or fails, the row is
   lost rather than duplicated. That ordering is the trade this ADR takes: a missing row on a dying process
   is the residual state we already tolerate (`trace_write_failed`), while a second, *unknown-outcome* row on
   a request that already has a terminal one would poison the class this decision exists to make trustworthy.
3. **The guard lives on the serving path, and it owns the facts its row needs.** It is constructed where the
   request commits to being served (after the boundary, before the first attempt) and it is owned by the
   future/body the server actually runs: the buffered medium's handler future, and the SSE medium's response
   body (the body owns the relay stream, so dropping the body drops the guard). A unit-test-only helper that
   is wired nowhere does not satisfy this decision — the guard's `Drop` *is* the mechanism, and it is reached
   because `hyper` drops the future/body on a client disconnect or on the shutdown drain. **Because the drop
   destroys the relay's own state, the guard must carry what its row needs**: the head's status when one
   arrived, a byte counter the relay updates as it forwards (shared, not copied at drop time), the attempt
   count, and the request's `wrote_full_request` evidence. The arm follows from those two facts alone — no
   head status ⇒ arm A; head status present ⇒ arm B — so the choice of shape is determined, never guessed.
4. **The writer it calls is synchronous.** `TraceWriter::write` is a synchronous call, so `Drop` needs no
   runtime and no timer; the sink is closed only after the graceful-shutdown drain has dropped the in-flight
   futures, or the drain's rows are lost (§5 names that ordering as an obligation, not an option).
5. **The honest boundary.** A `SIGKILL` writes nothing — no in-process mechanism can. That residual is the
   **event log's** `upstream.submitted` intent, counted by `unknown_outcome_requests`, and this ADR neither
   claims it nor hides it: the trace-side count and the event-side count are different populations by
   construction, and §4.16 says so on the surface that prints one of them.

### 2.4 The two shapes

| | arm A — no response body in flight | arm B — `2xx` head forwarded, body in flight |
|---|---|---|
| `result.status` | **499** (never sent) | **200** (the head the client has) |
| `result.attempts` | the attempts made (0 when none) | the attempts made |
| `result.upstream_status` / `upstream_ms` | the answering head's status if one had arrived (a refusal being composed), else `null` / `null` | the head's status / measured |
| `usage` / `cost` / `quota_after` | zeroed / `0` / `null` | zeroed unless the tap completed; never invented |
| `errors[0].kind` | **`canceled`** | `upstream_error` (the truncation shape, §4.20.3) |
| `errors[0].details` | `{ stage: "unknown_outcome" \| "not_billed", canceled_by: "client" \| "shutdown", attempt, wrote_full_request }` | `{ error_class: "stream_truncated", stream: true, stream_end: "client_closed", relayed_bytes, since_last_byte_ms, retry_refused: "forwarded" }` |
| accounting class | the attempt may have been billed (`unknown_outcome`) when one was submitted and fully written; `not_billed` when nothing could have been charged | the upstream may have billed what it generated; `usage_missing` says no measurement exists |

`canceled` is the **second trace-only kind** (`trace_write_failed` is the first): a request the client
abandoned produces no client-visible body, so §8's `error.type` table gains no row and must not. The kind
says what the process recorded, never what a client received.

### 2.5 `/metrics`, quoted

`/metrics` declares its own omission in-band, and this decision does not change that sentence:

> `# vadis: unknown_outcome_requests omitted — the figure lives in the event log and this surface does not
> scan it`

- **What it adds, for free:** arm A's rows are non-2xx records of the window, so `vadis_requests_failed`
  rises by them and `vadis_failures_by_kind{kind="canceled"}` is where a reader finds them — no new series,
  no new derivation (§4.16's single-owner rule makes the trace renderer the only formatter).
- **What it does not add:** the event-side `unknown_outcome_requests`. Two different populations, and the
  asymmetry is the point: the event figure counts intents with no closure (a `SIGKILL` leaves one and no
  row), the trace figure counts rows a live process wrote. Neither is derivable from the other, and this
  surface may not scan the log to pretend otherwise.

## 3. D2 — the diagnosable refusal

### 3.1 It is not a byte-boundary question

Constraint 1 governs the **request**: nothing here reads, reorders, re-encodes or records a request byte, and
the slice is taken from the upstream's **error response body** that the classifier already reads (§4.2's own
clause on the classification's evidence). The request bytes and the prefix they were hashed from are
untouched, the sent bytes are untouched, and no transform is engaged. This ADR adds a **response-side
record** and nothing else.

It is nonetheless a new persisting surface for content, and that is stated rather than smuggled: ADR-009
item 3 keeps bodies out of the **store**, and ADR-011 item 8 says the failure's reason survives only because
the body does not. D2 keeps that boundary exactly — the slice lives in the **trace JSONL only**, never in an
event row — and buys the diagnosis with four hard limits rather than with a promise.

### 3.2 The member, the bound, the redaction set

`errors[].details.upstream_body`, present iff a body arrived and at least one retained field is non-empty;
absent means *no body arrived, or nothing survived the bounds* — never "the body was empty".

| Rule | Value |
|---|---|
| capture bound | **1024 bytes** of the body, from the classifier's own read (no second read, no larger buffer, no new bound on the wire) |
| field allowlist | `message`, `type`, `code`, `param`, and `error.{message,type,code,param}` one level down when `error` is an object. **Every other key is dropped**, so a body that echoes the request contributes nothing |
| field bound | `message` ≤ **256 bytes**; `type` / `code` / `param` ≤ **64 bytes** each, cut on a UTF-8 boundary |
| shape | `shape: "json"` for an allowlisted extraction; `shape: "text"` **carries the byte count and no bytes** — an unparsable body is never copied, which is what keeps "never a full message body that could carry user content" a property of the format rather than of a provider's manners |
| redaction set | (a) every configured credential's exact value, replaced by `[redacted]` — a value-level rule, no guessing; (b) auth schemes `Bearer <x>` / `Basic <x>`; (c) key shapes `sk-…`, `sk_…`, `pk-…` and `(api[-_]?key\|token\|secret\|password)["']?\s*[:=]\s*"?<value>`; (d) a ≥32-char run of `[A-Za-z0-9+/=_-]` adjacent to a key-ish label. **Never a header** — no header value is captured at all, and the only header-derived fact a record already carries is `Retry-After`'s duration |
| JSONL | written through the record's own encoder, so control characters (CR/LF included) are escaped and the row stays **one physical line**; the bounds count **pre-escape** bytes, so a written line grows by at most the encoder's escape factor |

What it cannot promise, said plainly: a provider's `message` may quote a fragment of the client's own input
("invalid character in message 3: …"). The 256-byte bound, the redaction set and the fact that the value is
the **refusal's** text are the whole of the guarantee; the guarantee is not "no user content ever appears",
and it is not written as if it were.

### 3.3 On by default, and why there is no key

The alternative — a `trace.*` key defaulting to `off` — was considered and rejected: a knob nobody knows
about does not answer the finding, because the operator who needs the diagnosis is exactly the one who does
not know the key exists, and the audit's own evidence is that the *default* deployment produced six
undiagnosable refusals in one week. The four limits above are what make default-on defensible, and the
`trace` group's `deny_unknown_fields` stays untouched (no key is added, so no config surface, no wizard
question, no book chapter moves: the book links to this contract instead of copying it, constraint 8).

## 4. D3 — the retry

### 4.1 Shape A: output already on the wire — never retried

A retry after ≥1 body byte reached the client duplicates output on that wire: the client would receive two
beginnings of one answer with no way to tell which bytes belong to which. **The wire mechanism that would
make it safe does not exist in this product**, and naming it is the honest alternative: SSE resumption
(`Last-Event-ID`) would require the client to re-ask from a resumption point and the upstream to continue a
generation — none of the three wired protocols carries resumable event ids in vadis's passthrough, and none
of the three upstreams continues a generation. So the ruling is terminal: **record the cause, close the
stream without a fabricated terminal (never `[DONE]`), and let the client redo the turn** —
`details.retry_refused: "forwarded"` is how the record says a retry was considered and refused. The
`failover_zero_byte` gate that already refuses the walk after the first relayed byte is the mechanism; D3
adds the word, not a second gate.

### 4.2 Shape B: nothing forwarded — retried only where nothing was billed

The class table decides, and it is ADR-011 item 6's own evidence table rather than a new one:

| evidence | billed? | retry the same route |
|---|---|---|
| the request bytes were never fully written (connect / TLS / pre-write transport failure) | no | **yes**, bounded |
| an **error-status** head arrived and the attempt served none of its body (a refusal generates nothing) | no | **yes**, bounded |
| the request was fully written and nothing came back (`unknown_outcome`; the `WrittenNoResponse` class, whose `details.stage` already carries the word) | **maybe** | **no** |
| a `2xx` head arrived and any byte of its body arrived, or the body was cut before its terminal event | **yes** — the model was invoked | **no** |
| `request_timeout` tripped (our own outer bound) | unknown, and there is no budget | **no** |
| `format_error` / `content_policy_blocked` (deterministic for the unchanged request) | — | **no** |
| any other answer that was read (5xx / 429 / 401) | no | **no** — the walk's business, unchanged; a retry would only double the wait before the move |

So the retry this ruling permits is **narrower than "the stream was cut"**, and that is the decision rather
than a residue of it: a cut after a `2xx` head is a *served* attempt, and every served attempt is terminal.
The tightening this buys is worth naming, because it is where the shipped gate is a proxy and this rule is the
fact: `failover_zero_byte` decides on `relayed == 0` — bytes forwarded **to the client** — while the bill turns
on a different question, and a `200` head with no body byte yet is a model already generating. The two facts
must both hold, and "no byte reached the client" is only the first of them.

- **Attempt cap: one retry per route per request** (`STREAM_RETRY_CAP = 1`; two attempts on a route). The
  request-level cap is unchanged — the walk's candidate count is still the walk's — so the total attempts a
  request can make is `candidates × (1 + cap)`.
- **Backoff and floor:** a fixed **250 ms** before the retry (with one retry a ladder would be invented
  complexity, and jitter exists to desynchronise a fleet, which this is not; a drawn wait would make two
  identical requests record different latencies with nothing to attribute the difference to). The retry is
  attempted only when the remaining `server.request_timeout` budget is **≥ 1000 ms after that wait**;
  otherwise it is refused with `retry_refused: "budget"`. The retry's own attempt window is unchanged
  (`server.upstream_attempt_timeout`, bounded by the request deadline — ADR-053's rule, not re-opened).
- **Same route first, then the walk.** The retry is `provider/model`-identical to the attempt that failed,
  and it happens **before** the candidate walk, because a walk is a route change and a route change destroys
  the prefix cache and pays a re-prefill (ADR-011 item 9 prices it). A retry that succeeds therefore
  writes **no** `failover.triggered` row and no `result.failover_from` — the honest reading of that pair is
  "nothing moved", which is exactly what happened. This ordering is the one behavioural change of D3 for the
  `connect_failure` class, and it is a cost rule before it is an availability rule.
- **Both media.** The class table does not know the medium: the buffered path retries where the SSE path
  retries. Shape A simply cannot occur on the buffered medium (its head is produced only once the body is
  complete), so the split's first arm is the SSE medium's, and §8's "both paths produce the same shape"
  stance is preserved for everything else.
- **Interaction with `unknown_outcome`, stated once:** the class that ADR-010 item 4 calls *maybe billed* is
  the class this rule refuses by name. A retry there is a probable double charge, so the record says
  `retry_refused: "billed"` and the client's own retry — which knows its idempotency story — is the
  recovery. That is ADR-011 item 6's asymmetry, applied where the audit found it violated in spirit.

### 4.3 What the record says per attempt

- **`result.attempts`** (new, additive, always present; `0` on the classes that attempted nothing and `≥ 1`
  otherwise): how many upstream attempts this request made, across all routes. It answers the card's question
  without a second field: `attempts == 1` is *the retry never happened*; `attempts ≥ 2` with `failover_from`
  **null** is *the same route was retried and it served*; `attempts ≥ 2` with `failover_from` set is *the walk
  moved*.
- **`errors[0].details.retry_refused`** (one of `forwarded` / `billed` / `deterministic` / `budget`, present
  iff the class table was consulted): why the retry that a reader might expect did not happen.
- **Per attempt, the event log is the enumeration** — one `upstream.submitted` per attempt with its
  `attempt_index`, one `error.classified` per failed attempt. `error.classified` gains **`stream_end`** when
  the failure was a truncation (additive payload member), so the *sequence* of causes across attempts is
  readable from the log while the trace keeps the one terminal row.
- `stream_end` on the record remains the **terminal** attempt's cause word (§4.20.3). A retry that rescued a
  cut stream leaves a `200` row with `errors[]` empty and `attempts: 2` — the cut itself is the log's, and
  §6 does not grow a second error entry for a request that succeeded.

## 5. Consequences, and what is owed

- **Owed (a measurement change, human gate; constraint 9):** the fourth bucket. `failed` currently absorbs
  arm A; the precise shape is `unknown_outcome` as its own bucket with
  `succeeded + failed + stream_incomplete + unknown_outcome == requests`. It is not taken here because
  `CONF-109` pins the three-way partition and the invariant, and a bucket is a measurement definition.
- **Owed (implementation, not a ruling):** the graceful-shutdown drain must drop the in-flight futures
  **before** the trace sink is closed, or the drain's rows are lost; and the guard must be constructed after
  the boundary refusals (§4.7/§4.13 write their row synchronously and have no in-flight attempt).
- **Non-consequence, stated:** `schema_version` stays **2** (one added field, one added `details` member, one
  added kind and one added enum value are all additive; `result.status`'s type does not move). The
  `stream_incomplete` figure's **definition** is untouched by D1 — its *population* gains client-caused cuts
  that previously wrote no row at all, so a comparison of that figure across this change is not meaningful
  (the same stance §4.20.6 took for `overhead_ms_p99` across R74).
- **Not a saving, not a price:** nothing in this ADR is a token figure or a money figure; the retry's cost
  appears only as the ordinary `usage`/`cost` of the attempts that happened, labelled by §7's convention.

## 6. Alternatives considered

| Alternative | Why rejected |
|---|---|
| reconcile the trace from the event log after the fact | the trace is read without the store (constraint 3), and a reconciler that invents rows from intents is a fabricated closure with a new name (ADR-010 item 4) |
| a nullable `result.status` for the cancelled shape | changes how every existing reader reads a field that is typed `u16` — the `cost.currency` class that moved the version to 2 — and would force `schema_version: 3` plus five pinned conformance edits |
| a fourth bucket now | it is a measurement definition (`CONF-109` pins the three-way invariant); constraint 9 makes it a human decision, so it is owed, not taken (§5) |
| an explicit `unknown_outcome` status *string* beside `status` | two fields for one fact, and the word already has one owner in `details.stage` (`WrittenNoResponse` writes it), which `canceled` reuses |
| the slice off by default under a `trace.*` key | a knob nobody knows about does not answer the finding the ruling exists for (§3.3) |
| the slice as a raw body prefix (no allowlist) | it is the one shape that can carry arbitrary request fragments; the allowlist is what makes "never a full message body" a format property |
| the slice in the event log, where the classifier already sits | ADR-009 item 3 / ADR-011 item 8 keep bodies out of the store on purpose; the trace is the analysis medium, the store is the state truth |
| retry shape A over a resumable stream | none of the three wired protocols carries resumable event ids and none of the three upstreams continues a generation — the mechanism does not exist, so the ruling is terminal (§4.1) |
| retry everything after a full write ("make it robust") | ADR-011 item 6's measured asymmetry: a probable double charge is not a recovery |
| walk the chain instead of retrying the same route | a walk destroys the prefix cache and pays a re-prefill (ADR-011 item 9); it is the second resort, not the first |
| jittered backoff | one retry, one operator: a ladder and an RNG buy nothing and make two identical requests record different latencies |

## 7. Reversibility

- **D1** is reversible in one direction only: dropping the guard leaves the row shape inert (no rows of the
  class are written, and every reader's rules still hold, because the class was absent before). The `499`
  value is the part that would be expensive to retract — a reader that keyed on it must be told — which is
  why it is one sentence in §6 and not a bucket.
- **D2** is fully reversible: removing the member returns `details` to today's shape, and no reader may
  treat its absence as a fact about the body (§3.2's rule is what makes that safe).
- **D3** is reversible per arm: the cap, the backoff and the floor are one constant each; the ordering
  (same-route retry before the walk) is one call site. The `retry_refused` words are additive on an optional
  object.
