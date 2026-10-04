# ADR-011 — upstream failures are classified once, into a reason and a recovery action; a dead provider is demoted, not re-probed

- Status: accepted
- Date: 2026-09-19
- Related: ADR-003 (a revertible, individually accounted transform pipeline), ADR-004 (clients are stateless, so the gateway is the only place a failure is remembered), ADR-005 (the trace is the only product-analysis-loop channel), ADR-006 (integer NanoUsd), ADR-009 (store, bodies are never persisted, explicit failure modes), ADR-010 (event vocabulary, write-ahead, the `unknown_outcome` crash window), ADR-012 (the pattern tables are code, outside the auto-adoptable envelope), ADR-013 (the error mix is an automatic-rollback trigger); spec §3 (selection plus guard policy), §4.0 (a plan may only reference its own provider's models), §4.2 (fallback chain), §4.5 (state), §6 (observation), §7 (accounting convention), §8 (error behaviour); DESIGN §2 (crate roles), §3 (pipeline), §5 (breakeven = the switch-cost function reused here), §7 (translation), §8 (state), §10 (test strategy), §11 (risks), §12.3 (per-step failure semantics), §12.4 (pure cost functions), §12.5 (config), §12.6 (DecisionRecord), §12.7 (error surface)

## Background

Round 2 lands forwarding, so the gateway finally meets real upstream failures. What the design of record
says about them today is one sentence: "upstream 5xx / 429 / quota exhaustion -> switch per the fallback
chain" (spec §4.2, §8). One rule for a family of failures that are not alike:

- a `429` resolves in twenty seconds; an exhausted account does not resolve this month;
- a content-policy refusal is **deterministic for the unchanged request**: re-probing it reproduces the
  refusal and burns a paid attempt each time;
- a context overflow is not a routing problem at all — another provider accepts the same oversized prompt
  and bills it, so failing over makes the request *more* expensive, not less;
- a read timeout after the request bytes were fully written may already have been billed, and ADR-010
  item 4 already refuses to guess about that case: a gateway retry there is a probable double charge, not
  a recovery.

Two further problems are structural rather than semantic:

1. **Judgement spread over call sites.** Without one classifier, the decision is re-implemented where the
   error surfaces — the proxy, the guard chain, the provider adapter — each with its own string matching.
   The sites drift, "why did this request switch?" becomes unanswerable, and it is the same skew ADR-005
   forbids on the research side, now inside the product.
2. **The failure is forgotten between requests.** A provider that is out of quota is out of quota for
   every request that follows. A per-request check rediscovers that fact, and pays for the rediscovery,
   once per request.

The prior art is a working gateway's classifier, read for this ADR (not measured here):
`hermes-agent/agent/error_classifier.py` (1,316 lines: a `FailoverReason` enum of about 25 values, one
narrow pattern table per class, a priority-ordered pipeline, and a `ClassifiedError` whose payload is
exactly a set of recovery hints — `retryable`, `should_compress`, `should_rotate_credential`,
`should_fallback`), `hermes-agent/agent/rate_limit_tracker.py` (246 lines: the twelve `x-ratelimit-*`
headers, per-minute and per-hour, for requests and tokens), and
`hermes-agent/agent/credential_pool.py` (2,182 lines: per-credential exhaustion with a TTL chosen by the
status that caused it, a terminal-auth state that does not re-enter rotation, rotate-on-failure, soft
leases). What is adopted is the **method** — centralized taxonomy, narrow per-class tables, and a
classification whose output is an *action*. No code is copied; vadis's classifier is a pure Rust
function with its own class list (its failure surface is smaller, and its decisions are unlike hers:
she decides what to do for one conversation, vadis decides what to do with the money and the cache).

And one measurement from the previous project is the reason this ADR is a *state* decision and not a
per-request one: in its post-round analysis (recorded as H33 in this round's card) **62% of its failures
were "the retry selected the same dead provider again"**. That is the human's figure from that project,
not re-measured here. It is quoted because it names the failure mode precisely: the outage was cheap; the
retry policy that kept choosing the outage was not.

## Decision

1. **One classifier, in one place, as a pure function.** `vadis-core` owns

   ```rust
   pub enum FailoverReason { Auth, AuthPermanent, Billing, RateLimit, Overloaded, ServerError,
       Timeout, ContextOverflow, PayloadTooLarge, ModelNotFound, ProviderPolicyBlocked,
       ContentPolicyBlocked, FormatError, Unknown }            // the v0.1 slice, 14 classes

   pub enum RecoveryAction { Retry { after: Option<Duration> }, RotateCredential,
       FallbackProvider, Compress, Abort }

   pub struct Classification { pub reason: FailoverReason, pub action: RecoveryAction,
       pub matched: &'static str,          // the table entry that decided it (reproducibility)
       pub status: Option<u16>, pub retry_after: Option<Duration>, pub demote: Option<Demotion> }

   pub fn classify_upstream_error(ev: &ErrorEvidence<'_>, ctx: &AttemptCtx) -> Classification;
   ```

   `ErrorEvidence` is the raw material — status, headers, error-body bytes, and whether the request bytes
   were fully written before the failure. `vadis-providers` **passes it up and decides nothing**
   (DESIGN §2: the provider layer makes no decisions); the proxy, the guard chain and the plugins never
   branch on an upstream error body's text. The pattern tables live in one module: **one table per reason
   class**, whose entries are **narrow verbatim strings observed from a real provider** — never a generic
   word like "policy", "limit" or "quota", which collide across classes (that collision is a real defect
   class in the prior art, which is why its tables are deliberately phrase-level).
   - A class may not exist without a table and at least one fixture. An untested routing rule is a rule
     nobody can argue with.
   - The v0.1 slice is 14 classes against the prior art's ~25: vadis has no image pipeline, no
     multi-tenant policy layer and no per-feature entitlement zoo, so classes with no vadis failure to
     describe are not carried "for symmetry".

2. **The classification is the only input to the failure path, and its priority is stated** (so two
   implementers cannot disagree about which rule won):

   | # | Rule | Why it must sit here |
   |---|---|---|
   | 1 | provider-specific narrow patterns (content-policy refusal, rejected encrypted replay blob, rejected grammar/parameter forms) | a per-prompt deterministic decision must not be downgraded to a generic 400 or 5xx, and a status-less refusal (some SDKs raise without one) must not fall into the retryable catch-all |
   | 2 | HTTP status, refined by body text | the skeleton: 401/402/403/404/413/429/400/5xx each have a default class |
   | 3 | the structured `code`/`type` inside the error body | providers with a real error-code vocabulary describe the failure better than the status does |
   | 4 | message patterns (only when no status is available) | transport-level and shim-level failures arrive without a status |
   | 5 | transport heuristics (error type / `isinstance`-style shape) | the last mechanical signal |
   | 6 | `Unknown` | the catch-all is retryable, subject to item 6's evidence rule |

   Two ordering lessons are load-bearing and are adopted explicitly, because each of them was a defect the
   prior art fixed (the ordering, not the code, is what is being adopted):
   - **a 5xx whose body carries unambiguous request-validation text** ("unknown parameter", "unsupported
     parameter", "invalid_request_error") is `FormatError` and **not retryable** — every retry gets the
     identical rejection, so the generic "5xx is retryable" rule turns one bad request into a retry flood;
   - **a connection close on a large session means context overflow, while an SSL/TLS alert mid-stream
     means a transport hiccup** — the two look alike and have opposite recoveries (compress vs retry), so
     their relative order is part of the contract and gets fixtures.

3. **Three semantics are adopted as stated**, because they are what a classification is for:

   | Reason | Semantics | Why |
   |---|---|---|
   | `ContentPolicyBlocked` | deterministic for the unchanged request: `Abort` locally, `FallbackProvider` if a route remains, **never retried unchanged** | re-probing the same prompt reproduces the same refusal and spends paid attempts to learn nothing |
   | `ContextOverflow` | `Compress`, **not** failover | failing over bills another provider for the same oversized prompt; the defect is on this side of the wire (compression is a P1 lever, ADR-003) |
   | `PayloadTooLarge` | `Compress`, then retry the **same** route | the route is healthy; the payload is not |

   The rest of the v0.1 mapping, stated so it is not improvised: `Auth` -> rotate credential, then fall
   over; `AuthPermanent` -> fall over or abort, with no refresh loop; `Billing` -> item 4, then fall over;
   `RateLimit` -> item 5 (honor the provider's clock), then rotate or back off; `Overloaded` /
   `ServerError` -> bounded backoff and fall over; `Timeout` -> item 6; `ModelNotFound` -> fall over and
   surface the roster entry as a config defect; `ProviderPolicyBlocked` -> surface it, do **not** fall over
   (an account-level data/privacy setting applies to every call on that account, so the next provider
   fails for the same reason); `FormatError` -> abort or fall over, never retried unchanged; `Unknown` ->
   bounded retry.

4. **Vadis-specific addition 1: the demotion unit is the provider, not the credential.** A failure whose
   class is `Billing` (or a plan reaching zero) is a fact about the **account**, and every model under that
   provider shares it — spec §4.0 already says a plan may only reference models of its own provider. So a
   quota/billing-class failure **demotes the whole provider**: all of its models are unavailable until the
   reset instant, and no request pays to rediscover that.
   - Triggers: a 402/403/429 whose body matches the account-exhaustion table (`access_terminated`,
     `usage limit`, `insufficient_quota`, `quota exceeded`, `plan does not include`, `key limit exceeded`,
     `credits exhausted`, `model_not_supported_on_free_tier`, ...), which is distinct from the
     rate-limit table (`rate limit`, `too many requests`, `throttled`, `resource_exhausted`, ...);
     and a configured plan (`quota.over_quota = block`, spec §4) reaching zero.
   - **The demotion is state, not a local variable.** It outlives the request and must survive a restart,
     so it is written through the store (ADR-009, ADR-010): `error.classified` carries it (item 8), and the
     roster view is a projection (`provider_cooldown`) behind the existing service traits (DESIGN §8,
     §12.2). Losing the projection costs one doomed attempt; it is rebuilt from `events` like any other.
   - **Its effect on selection is a guard input, not a new selection mode.** A demoted route is a route the
     guard refuses (`GuardOutcome`, DESIGN §12.3): with a configured `fallback` chain that is a
     `Downgrade` and `failover_from` is recorded; with an exhausted chain it is the spec §8 error. So the
     observable surface is spec §3 / §4.2 / §8 applied to "unavailable until T", exactly as it already
     applies to "over quota" — and the expensive part (a doomed paid attempt) never happens.
   - **A demotion ends by evidence, never by hope:** its TTL is the provider's reset instant when it sends
     one (`Retry-After`, an `x-ratelimit-reset-*` header, or a reset epoch in the body), otherwise a
     declared default cooldown; and a successful attempt is what clears it early.
   - **A demotion nobody can see is indistinguishable from "the fallback chain is now the production
     configuration".** It is surfaced in `/health` and counted by `vadis stats`.
   - Honest boundary: this is per-process local state (spec §1's single-operator scope), and it reduces
     wasted work for a dead account — it does not repair a revoked key or a mispriced quota.

5. **Vadis-specific addition 2: the provider's own clock is honored.** `Retry-After` (delta-seconds or an
   HTTP date) is honored, and the `x-ratelimit-*` family is captured whenever a provider emits it (the
   twelve-header limit/remaining/reset schema for requests and tokens, per minute and per hour), because a
   signal that arrives *before* the 429 is cheaper than the 429.
   - **The cap is vadis's own budget and has to be stated:** an inbound request has
     `server.request_timeout` (default 10m) and an upstream attempt has `server.upstream_attempt_timeout`
     (default 60s, spec §4). A `Retry-After` larger than what remains of the request's budget **cannot be
     honored inside that request**: the route is demoted until the reset instant (item 4) and the fallback
     chain is used instead. The client is not made to wait for a provider, and the wait is carried across
     requests by the demotion, which is the only place it can live. (The prior art caps its own honor
     window at 120s for the same reason; the difference here is that the surplus becomes state.)
   - A rate limit is not a provider death: `RateLimit` demotes the **route** (or the credential) until the
     reset, while `Billing` demotes the **provider** until the plan resets. Two TTLs, one table.

6. **Vadis-specific addition 3: retry is allowed only with not-billed evidence.** This is ADR-010's
   asymmetry, applied to failover rather than to a crash:

   | Evidence at the failure | Retry the same route? | Fall over? |
   |---|---|---|
   | no connection was ever established (connect or TLS failure before the request bytes went out) | yes, bounded | yes |
   | the provider answered (any status, including 5xx / 429 / a refusal) | yes, bounded, unless the class is deterministic | yes |
   | the request was fully written and the response never arrived (read timeout, or a connection closed mid-response) | **no** | **no** |

   After a full write the upstream may already have billed, so a gateway retry is a probable double charge
   exactly as a crash-retry is (ADR-010 item 4). That attempt becomes an `unknown_outcome`: the ambiguity
   is recorded and reported, the quota is not re-charged, and the decision belongs to the client, which at
   least knows its own idempotency story. What vadis does for the *next* request is state — demote or
   re-probe the route (item 4) so the client's own retry lands somewhere healthy.
   - Retry is bounded: a fixed attempt count and a capped, jittered backoff, with the attempt budget
     (`upstream_attempt_timeout` x attempts) inside `request_timeout`. A retry is never an unbounded loop —
     the retry flood that follows from a full-write retry is the failure mode being designed out.

7. **Credential rotation is a different axis from route failover, and v0.1 says so honestly.** A provider
   may hold several keys, and a subscription allowance belongs to the key/account, so `RotateCredential`
   moves to the next key **of the same provider** (per-credential exhaustion with a TTL chosen by the
   causing status, a terminal-auth state that does not re-enter rotation, rotate-on-failure — the shape
   the prior art's pool implements), while `FallbackProvider` moves down the `fallback` chain. They are
   not alternatives: a 401 rotates first and then falls over; a 429 rotates if another key has headroom
   and otherwise backs off.
   - v0.1 has exactly one `api_key_env` per provider (spec §4), so rotation is a **slot** whose
     implementation is a key list — an additive config key with its own spec change. Until it exists,
     `RotateCredential` is a no-op that falls through to `FallbackProvider`, and the trace records which
     of the two actually happened. Not rotating is stated; pretending to rotate is not.

8. **Every classification writes one event and mirrors it in the trace.**
   - `error.classified` joins ADR-010 item 2's vocabulary: `upstream.responded` -> classify ->
     `error.classified` -> the action's effect (an `upstream.submitted` for a retry or a failover attempt,
     a `session.bound` move, or the error response). Payload: `request_id`, `attempt_index`, status,
     `reason`, `action`, the **matched table entry**, `retry_after_s?`, and `demotion?` (provider or route,
     plus its TTL).
   - Durability: **`NORMAL`** (ADR-009 item 4's test is "may this fact be recomputed?"). It authorizes no
     charge — the paid attempt it leads to is authorized by its own FULL `upstream.submitted` intent row
     (ADR-010 item 3) — so a lost classification row costs diagnosability, not correctness. It is *not*
     recomputable, because the upstream error body is never persisted (ADR-009 item 3, ADR-010 item 6);
     that is precisely why it is written: it is the only surviving evidence of **why** the gateway
     switched.
   - Trace mirror: the existing `errors[]` element carries it (spec §6 "failure details") as
     `kind = upstream_error` with `details = { reason, action, matched, retry_after_s?, attempt }`.
     **No spec §6 field is added**, so `DecisionRecord.schema_version` does not move (DESIGN §12.6).
   - Why an event rather than a counter: the loop must answer "how often did we switch, for which reason,
     and what did each switch cost" from replayable records (ADR-005's channel); a rolled-up counter cannot
     be re-interrogated after the fact, and ADR-013 makes the reason mix a rollback trigger.

9. **Vadis-specific addition 4: a failover records the cost of the cache it broke.** Switching
   provider/model destroys the prefix cache (spec §4.2's own clause); this design turns that caveat into a
   recorded, priced number, split by the convention it can honestly claim (spec §7, AGENTS constraint 4):
   - **At decision time**, before the effect: `failover.triggered` (ADR-010's event) carries
     `reprefill_tokens` = the session's prefix tokens from the ledger's per-block tokens (spec §6) and
     `switch_cost_nano` = the same pure function DESIGN §5/§12.4 already specifies for model switching
     (`prefix_tokens x p_miss_new`), in integer NanoUsd (ADR-006). This is an **`inferred`** figure: it is
     a local computation over a planned route, and no upstream usage exists yet.
   - **When the switched attempt's usage lands**, the measurement that makes it `verified`: the
     miss-priced tokens actually billed on the first post-switch turn (from the normalized `Usage`) and
     their delta against what the same token count would have cost at the origin route's `input_hit` price
     (`extra_cost_nano`). The token counts are measured; only the price table is config.
   - A failover with no post-switch turn keeps the inferred figure and says so (spec §7's reporting rule:
     convention plus sample size plus window). So `vadis stats` can report *the verified cost of the
     failovers in this window* — a number that today does not exist anywhere in the project.

10. **Failure modes of the classifier itself (fail by design).**

    | Failure | Behaviour |
    |---|---|
    | no table matches | `Unknown` -> bounded retry, and the unmatched error is recorded; a table that stops matching a provider's new wording must show up as a rising `Unknown` count, not as a silent behaviour change |
    | two tables match | item 2's priority decides, and `matched` makes the verdict reproducible; a fixture asserts each class's intended landing |
    | the error body is not JSON, or not UTF-8 | classification proceeds on status and headers; parsing the body is never a precondition, so a malformed error body cannot turn a 429 into an `internal` |
    | a cooldown projection is stale or lost | rebuilt from `events` (ADR-009 item 5); the cost of losing it is one doomed attempt |
    | `Retry-After` is absurd (hours) | never honored inside the request; it becomes a demotion TTL (item 5) |
    | the provider returns a 200 with an error embedded in the stream | out of scope here: it is an SSE passthrough failure (CONF-13) and its own change |

11. **Where the tables live.** In v0.1 the pattern tables are **code**, versioned with the binary and
    unit-tested — not a config or rule artifact. A mistuned table silently changes routing and money
    behaviour, and its failure signal (a shift in the reason mix) is weaker than the inline-test discipline
    a rule file carries (ADR-008 item 4). They are therefore **outside ADR-012's auto-adoptable envelope**:
    a later round may move them into a rule file, and if it does, that is a **new rule** and goes through
    the human gate.

## Alternatives considered

| Alternative | Why rejected |
|---|---|
| per-call-site string matching (the status quo) | judgement re-implemented wherever an error surfaces; the sites drift, "why did we switch" stops being answerable, and it is the skew ADR-005 forbids on the research side, now inside the product |
| retry every failure with backoff (the "make it robust" default) | reproduces a deterministic refusal and a dead account until the budget is gone, and a retry after a full write is a probable double charge (item 6) |
| status-code-only classification (4xx non-retryable, 5xx retryable), no tables | cheap, and exactly the rule that turns one bad request into a retry flood and mislabels a content-policy block; kept as the skeleton under item 2's tables, not as the design |
| demote the credential only, like the prior art | correct in a per-key world; wrong here, where a plan allowance is a property of the provider/account and every model of it shares the exhaustion — this is the shape H33 measured |
| demote nothing; let each request rediscover the dead provider | the measured failure mode of the previous project (62% of its failures) |
| classify per attempt from the raw error, no event | loses the "why did we switch" evidence exactly where the body is gone forever (bodies are never persisted), and it cannot be aggregated by the loop |
| honor `Retry-After` by holding the client's request open | turns a provider's outage into this gateway's latency, up to `request_timeout`; state (a demotion) carries the wait instead |
| make the tables a config/rule artifact now | a money-affecting tuning surface with no inline-test discipline and no envelope; deferred deliberately (item 11) |

## Rationale

- The failure path **is** a money path: every decision here changes what gets billed, so it gets the same
  treatment as the transform pipeline (ADR-003) — one classifier, one action, one accounting label, one
  record.
- Centralizing is what makes the priority order *statable*. The interesting cases are all ordering
  problems (a 5xx that means "your request is bad", a connection close that means "too large", an SSL alert
  that means "hiccup"); an ordering that lives in one file with fixtures can be argued about and tested,
  and one spread across three call sites cannot.
- The provider demotion is the transferable lesson of the previous project, and it is a *state* decision
  for the same reason ADR-010 is: the expensive failure was not the outage, it was the retry policy
  re-selecting the outage.
- Pricing the cache a failover broke is the one place where this project's first-order lever (the prefix
  cache, AGENTS gotchas) and its availability mechanism (failover) pull against each other. Naming the
  conflict and measuring it beats noting it in a caveat.

## Consequences

- DESIGN §7 gains the "upstream errors are normalized in exactly one place" clause; §8 gains the upstream
  failure path (classify -> act -> record), the demotion projection and the failover-cost split; §11 gains
  the dead-provider and scattered-matching risks; §10's unit row names the taxonomy's fixtures.
- The observable surface is unchanged: spec §3 (the guard may reject or downgrade), §4.2 (the next
  unattempted route), §8 (the normalized error) already cover it, and this ADR **adds no `error.type`**. It
  tightens *when* a route counts as attemptable; the sentence that makes the demotion explicit in the spec
  ("a route whose provider is inside a declared cooldown is treated as unavailable until it expires and is
  not attempted") lands with the implementation change, docs-first, as a §4.2/§8 clarification.
- The implementing round allocates the new conformance cases; this ADR deliberately allocates none
  (ADR-010's precedent, and DESIGN §12.8's IDs only grow). Candidate coverage: a 429 with `Retry-After`
  demotes and then falls over; a 403 with an account-exhaustion keyword skips the provider without an
  attempt on the next request; a 5xx carrying request-validation text is not retried; an error body that
  is not JSON still classifies.
- `vadis-core` gains the classifier and the tables — pure, I/O-free, unit-testable. They are the only
  place in the product that knows a provider's error wording, which is what makes "no scattered string
  matching" checkable rather than aspirational.
- The store's projections gain the cooldown/roster view; `/health` and `vadis stats` must surface
  demotions and the `Unknown` count.
- The loop gains a cheap, replayable observable (reason mix, failed-switch count) that ADR-013 uses as an
  automatic-rollback trigger.
- Honest boundary: the classifier is **empirical**. Its tables are observations of one provider's wording
  on one day, and a provider may reword tomorrow, so the `Unknown` count and the reason mix are
  first-class metrics, not diagnostics. Nothing here is a guarantee about a provider's behaviour.
- Not covered here: pre-flight quota probing beyond the header capture of item 5; the credential key list
  (item 7) as an additive config key; and a 200-with-an-error-in-the-stream, which belongs to the SSE
  path.

## Publication note (2026-10-03, R62-2)

The `R<n>` labels and finding ids in this document name iterations of the project's own
private analysis loop — a loop that is not part of this repository, so no label here is
resolvable by a reader of it; they are kept as the provenance of the decision. This
publication pass removed only the dead-pointer class: every reference into that loop's
working tree (its file paths and round-record names, its state record, charter, execution
model and replay contract, its scripts and module names, and the kanban card ids), each
replaced by the neutral phrase the sentence needs. Nothing else moved — no figure,
threshold, `§`/`ADR`/`CONF` id, code sample or contract sentence.
