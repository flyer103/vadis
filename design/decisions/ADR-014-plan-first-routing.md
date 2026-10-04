# ADR-014 — plan-first routing: the subscription is the preferred account, the metered API is the spill, and the recovery is a session-boundary probe

- Status: accepted
- Date: 2026-09-20
- Related: ADR-003 (a revertible, individually accounted transform pipeline), ADR-004 (clients are stateless, so the gateway holds every binding), ADR-005 (the trace is the only product↔analysis-loop channel), ADR-006 (integer NanoUsd), ADR-009 (one local store, tiered durability, rebuildable projections), ADR-010 (the event log is the state truth; write ahead, then execute), ADR-011 (one classifier; a quota/billing failure is a fact about the **account**; a failover prices the cache it broke), ADR-012 (the measurement is not part of the search space; a routing knob with money behaviour goes through the human gate), ADR-013 (online iteration rails); spec §1 (v0.1 scope), §3 (selection and the Guard stage), §4 (`quota`; a plan references only its own provider's models), §4.0 (price convention), §4.2 (the fallback chain), §4.5 (state), §6 (observation), §7 (accounting convention), §8 (error behaviour); DESIGN §5 (the switch-cost function), §6 (stickiness), §8 (state), §12.3 (`GuardOutcome`), §12.4 (the quota pure functions), §12.5 (config parsing), §12.6 (DecisionRecord), §12.9 (GAP-Q1), §12.10.2 (config landing), §12.10.4 (store and projections), §12.10.5 (event wiring), §12.10.6 (prefix blocks)

## Background

A coding-plan subscription and a pay-per-token API are **two accounts that can serve the same model**. Inside
the plan the marginal price is 0 (spec §4 `quota`, DESIGN §5); on the metered account every token is billed at
the model's real price. When both are configured, the economically correct behaviour is not in doubt — use the
plan while it lasts, and only then spend money. What the design of record has to say about it is nothing:

- `quota` exists **per provider** (spec §4) and the local counter may already reject on it, but the vadis has
  no notion of an *account*: nothing distinguishes "this provider is a subscription" from "this provider is
  metered", so nothing can express a preference between two providers that serve one model;
- the only mechanism that can move a request from one provider to another today is the global `fallback` chain
  (spec §4.2), which is a **failure** mechanism, not an economic preference: it does not prefer the plan while
  both routes are healthy, it is not session-aware (every request re-decides), and it cannot express "spill to
  the metered account now, and come back when the plan is usable again";
- ADR-011 item 4 already treats a plan reaching zero as a fact about the **account** and demotes the whole
  provider — that is the availability half of the problem. The routing half (which account this *session*
  should be on, and what the move costs) has no owner.

Three constraints make the naive answers wrong, and they are the reason this is an ADR rather than a config
key:

1. **A route change is a cache event.** Switching account switches the upstream cache namespace, which is
   exactly what spec §4.2's own clause prices ("switching loses the prefix cache"). A policy that re-decides
   per request — prefer whichever account the local counter has room on, re-probe whenever a probe may have
   expired — makes the route flip within one session and pays a re-prefill every time it does. That would
   trade a first-order lever (prefix stability, AGENTS "Environment gotchas") for a second-order one (model
   choice), which is the trade this project exists to refuse.
2. **The local allowance number may be a placeholder.** GAP-Q1: the official coding-plan page publishes a
   monthly price, not a token allowance. A configured `tokens` value therefore may be an operator placeholder,
   and a router that **rejects** on it refuses work on the authority of a number nobody can verify — while the
   only party that actually knows the allowance (the upstream, via `403 quota_exhausted`) is never asked.
3. **The two directions are not symmetric.** Leaving the plan is *forced* (the plan is gone); returning is a
   *bet*. A bet belongs where it is cheap and observable, not inside a running session whose next turn will be
   billed for it either way.

## Decision

1. **The switch granularity is session stickiness.** A session (spec §4's `key_sources`, `prompt_cache_key`
   preferred — measured to be the client's own session id) is pinned to an **account** — its subscription
   (`coding_plan`) or its metered API (`api`) — for as long as it lives, and it changes account **only** when
   it is forced to (the primary account is exhausted) or when the primary has been *proven* usable again
   (item 3). A per-request decision between the accounts is forbidden, for the reason in the Background: it
   flips the route inside one session and destroys the prefix cache the session was built on.
   - The row that carries this is the existing sticky binding (`session.bound`, ADR-010 item 2): a session's
     effective account is a property of its binding, so no new per-request bookkeeping is introduced and
     nothing about the binding's TTL changes.
   - **Recovery means new sessions go back to the primary.** While the plan is usable, the preferred account
     is the plan for every session, new or in flight (the in-flight case is item 3's pull-back).
2. **The upstream's word is the authority; the local counter is a warning.** (Ruled explicitly, because the
   difference decides whether a request is refused.)
   - **Authoritative:** an upstream `403` classified as `quota_exhausted` (ADR-011's classifier; the class that
     `demotes_provider()` returns `true` for). This is the only signal that may **move the account**.
   - **A warning:** the local `quota_counters` projection and the pure `charge()` verdict of DESIGN §12.4. It
     **may not** `Reject` a request, and it **may not** force a spill, on its own. GAP-Q1 is the reason: its
     denominator (`quota.tokens`) may be a placeholder, and a placeholder that refuses work is a fabricated
     gate. It keeps exactly two honest uses:
     - **it is recorded and visible:** `cost.quota_after.verdict` (`inside` / `spill` / `blocked`) already
       carries it in the trace (spec §6), and `/health` plus `vadis stats` surface it, so the exhaustion is
       visible *before* the upstream says so;
     - **it may defer the probe** (item 3): when the local counter says the allowance is exhausted and the
       plan's own window boundary (`quota.window`, `reset_day`) has not passed yet, the probe is not worth
       spending — the window's reset instant is a config fact with a `source` (spec §4), unlike the allowance
       value. **Deferring an experiment is not blocking a request:** the request itself still goes wherever the
       state says it goes.
   - What this changes about today's wiring is stated plainly: `over_quota: block` on a plan whose `tokens` is
     a placeholder must not produce a refusal (DESIGN §12.4's `QuotaVerdict::Blocked` stays the *local* verdict
     and remains the recorded warning; the `Reject` follows upstream evidence, item 7). Registered as GAP-Q16.
3. **The recovery trigger is a probe, and a probe is admitted only at a session boundary.** After the cooldown
   (default **15m**, configurable) has passed since the account moved, the next request that is **a session's
   first request** (`turn_index == 1`, spec §6's own field, read from the sticky projection — not a clock and
   not a count of user messages) is admitted as a probe on the primary account. A successful probe flips the
   account back to the primary and **records the event**; a failed probe is an ordinary classified failure
   (ADR-011: `error.classified`, demotion refreshed) and changes no plan state.
   - **The hard rule, derived from items 1 and 3 (second half), stated so an implementation cannot drift:**
     > **A probe happens only at a session boundary — a new session, or a session's first request — and never
     > mid-session.** A session that is already on the overflow account is pulled back to the primary only when
     > the primary has been **judged usable by the upstream** (for example because another session's probe
     > already succeeded, which flipped the account state); that migration must record `plan.switched`.
   - **A request with no session never probes.** It has no boundary to be admitted at (`turn_index` is 1 for
     every sessionless request by definition, so "first request" is not a discriminating test there), and
     probing on every sessionless request is precisely the per-request flip item 1 forbids. A sessionless
     request follows the current account state and no more.
   - The probe's admission predicate, in full (so two implementations cannot differ): the request's session is
     non-null; `turn_index == 1`; the account state is `overflow`; `now >= until_us` (the cooldown deadline the
     last transition implies); ADR-011's cooldown projection does not refuse the primary route (two constraints
     on one attempt, deliberately not merged: ADR-011's is *route availability*, this one is *family routing
     intent*); and the probe is not deferred by item 2's window rule.
4. **The plan is the preferred account when both accounts serve the model.** The policy names one model family
   (`family`, the model id both routes carry) and the two routes that serve it (`primary`, `overflow`). A
   request whose resolution (spec §3, alias included) lands on either of those two routes is *inside the
   family* and is routed by the state: primary while the state is primary, overflow while the state is
   overflow. A request outside the family is untouched — the policy is not a global routing rule.
5. **Spillover, and what it is recorded as.** A primary attempt classified `quota_exhausted` moves the account
   state to `overflow`; the request is then retried on the overflow route, and every subsequent request in the
   family follows the new state. For a request inside a plan family the family's `overflow` route is the **first
   candidate after `primary`**, whether or not it appears in the global `fallback` list (spec §4.2 is refined by
   one sentence to say so); only if the overflow attempt itself fails does the global chain continue from there.
   - **The move is priced like a failover, because it is one.** Changing account changes the upstream cache
     namespace, so the same pure function DESIGN §5/§12.4 already specifies for a model switch applies:
     `reprefill_tokens` (the session's prefix tokens, GAP-Q14's proportional attribution — therefore
     `inferred`) and `switch_cost_nano = prefix_tokens × p_miss(destination account)` (integer NanoUsd,
     ADR-006): the metered account's real miss price on the way out, and **0** on the way back, because an
     in-plan destination's marginal price is 0 (item 6). ADR-011
     item 9 is the precedent and its convention is adopted unchanged: `inferred` at decision time, `verified`
     once the switched attempt's usage lands — and for a plan-first switch the verified figure needs no new
     field, because the in-plan marginal price is **0**: the switch's verified cost *is* the measured
     `cost.total` of the first post-switch request, which rides in that request's own record next to its
     `plan_switch` marker.
   - `result.failover_from` keeps its own meaning and is set only when a **failure-class** fact moved the
     request off a route (an in-request failure, or ADR-011's cooldown refusing a route before the attempt).
     A move that the *plan state* alone caused sets `plan_switch` and leaves `failover_from` null: nothing
     failed in that request, and saying otherwise would make the field mean "the route changed", which is what
     `plan_switch` is for.
6. **The cost convention.** In-plan requests are accounted at the plan's **marginal cost 0** and record
   `quota_after` (spec §6, DESIGN §5 — unchanged); overflow requests are accounted at the model's **real
   price** through the ordinary five-tier path. The account a request was billed under is not a new trace field:
   it is a property of the provider entry (`account`, item 8), so `decision.provider` already determines it.
7. **`on_primary_exhausted` decides between spending and refusing, and a refusal is readable.**
   - `spill` (default): item 5 — the family continues on the metered account.
   - `block`: a request that cannot be served on the primary is **refused**, with the spec §8 error body and a
     message naming the family, the account state and the reason (`quota_exceeded`, 429 — the existing error
     type; the plan allowance is exhausted and the policy forbids spending). It is never a silent failure and
     never a quietly downgraded request: this is the mode an operator chooses when spending is not acceptable,
     and a `200` that quietly came from the metered account would defeat it.
   - **The optional `overflow_monthly_cap_usd` guardrail** caps the family's *metered* spend in a UTC calendar
     month and, once reached, refuses the family's overflow requests with `cost_cap_exceeded` (403, spec §8).
     It may reject where item 2's warning may not, and the difference is not a loophole: the cap is compared
     against **measured** usage priced by the config table, while the plan allowance's denominator is the
     unverified one. The check is evaluated before the attempt against the month's already-measured spend, so
     the request that *crosses* the cap is served (its cost is unknowable beforehand) and later ones are
     refused — the overshoot is bounded by one request and no number is ever estimated.
8. **One new event kind, `plan.switched`, and one new config-level word, `account`.**
   - `account: coding_plan | api` is a property of a **provider entry** (the account, not the model: the key,
     the endpoint and the allowance all belong to it — ADR-011 item 4 says as much). Absent means `api`: the
     metered account is the ordinary case and is what today's roster already means.
   - `plan.switched` joins ADR-010 item 2's vocabulary. Payload: `family`, `from_account` / `to_account`,
     `from_route` / `to_route`, `reason` (`primary_exhausted` | `primary_recovered`), `probe` (true when the
     transition was caused by an admitted probe), `reprefill_tokens` and `switch_cost_nano` (item 5's inferred
     figures), and the `session` that carried the evidence (null when the evidence came from a sessionless
     request). **Durability `FULL`.** Its job is to make the family's account state a projection with a
     rebuild path (ADR-009 item 5) instead of a memory: after a restart a spilled family resumes on the
     overflow account with its probe deadline intact, which is the difference between "the plan expired
     yesterday" and "the gateway forgot and re-sent to a dead plan".
     - Why `FULL` and not `NORMAL` (ADR-011's `error.classified` is `NORMAL`): that row authorizes *one* paid
       attempt, which has its own FULL intent row; this row changes the **route of every subsequent request in
       the family** and therefore changes what the client is billed (metered instead of free). It is an
       accounting-direction fact of the class ADR-009 item 4 lists alongside `cost.computed` / `quota.charged`,
       and its loss is not recomputable from anything else — the 403 that caused it is never persisted
       (ADR-009 item 3). One row per transition (a handful a month) at ADR-009's measured ~0.1 ms p99 per FULL
       commit is not a cost worth designing around.
   - **Why no other kind is added.** A new kind is a vocabulary change that every reader must tolerate, so each
     candidate is refused explicitly:
     - `plan.exhausted` — the upstream's `403 quota_exhausted` is already one fact with one owner
       (`error.classified`, ADR-011 item 8). A second row for the same fact would give the account state two
       writers.
     - `plan.probed` / `plan.probe_failed` — a probe is not a transition: a successful probe *is* a
       `plan.switched` (with `probe: true`), and a failed probe's evidence is the attempt itself
       (`upstream.submitted` on the primary route, an off-preference route by construction) plus the existing
       `error.classified`. Neither needs a name of its own.
     - `quota.warned` — item 2's warning is not a state transition (nothing changes), and ADR-010 item 1 makes
       the log the record of transitions; the warning is already visible in `cost.quota_after.verdict` and in
       the `quota_counters` projection.
     - `plan.recovered` — the return is the same transition in the other direction; one kind with
       `to_account: primary` distinguishes them, and a reader that wants recoveries filters on that field.
9. **The landing point: one Guard rule, no new pipeline stage.** The policy is expressed through the
   `GuardOutcome` vocabulary the pipeline already has (DESIGN §12.3: `Pass` / `Downgrade(RouteSpec)` /
   `Reject { code, message }`) and is evaluated as the **first rule of the resident quota guard** — before the
   allowance rule, because deciding the route is a precondition for the allowance verdict being about the right
   account. It adds no fiber (no new `plugins:` entry, no unload/LIFO obligation), no config section beyond
   §4.6, and no step to DESIGN §3's pipeline. Its rule order is part of the contract, because the order
   decides which of two legitimate answers wins:

   ```
   for a request inside a plan family:
     1. probe admission (item 3's predicate holds)  -> Pass on `primary`   (the preferred account, free)
     2. overflow cap reached (item 7)               -> Reject{cost_cap_exceeded}
     3. account state is `primary`                  -> Pass on `primary`
     4. account state is `overflow`                 -> Downgrade(`overflow`)      [on_primary_exhausted: spill]
                                                    -> Reject{quota_exceeded}     [on_primary_exhausted: block]
   ```

   Rule 1 before rule 2 is deliberate: a request admitted as a probe is an attempt on the *free* account, so it
   must not be refused by a cap on the metered one.
10. **Failure modes of the policy itself (fail by design).**

    | Failure | Behaviour |
    |---|---|
    | the plan state projection is stale or lost | rebuilt from `plan.switched` rows (ADR-009 item 5); the cost of losing it is one off-preference attempt, never a wrong charge |
    | the overflow route is unavailable too | the ordinary path: the classification walks on into the global `fallback` chain, and an exhausted chain is the spec §8 error — the policy never invents a route |
    | a probe fails every time | bounded by construction: one attempt per cooldown *and* one per session *and* (item 2) not before the plan's window boundary; there is no probe loop |
    | the plan's window boundary is not declared (`quota` absent) | there is nothing to defer to, so the cooldown alone governs the probe; the state is still driven only by upstream evidence |
    | `recover: none` | no automatic probe. The family returns to the primary at the plan's own window boundary when one is declared (one attempt per window), and otherwise only by an operator action — stated because "recover: none and no window" means "the family stays on the metered account until you change something" |
    | the config changes `cooldown` mid-flight | the probe gate is recomputed from the *current* config against the last `plan.switched` instant; a policy knob changing a future deadline is not a rewrite of history, and events are never rewritten |
    | the client asks for the overflow route directly | legal (spec §3) and harmless: the request is inside the family, so the state routes it — to the primary while the plan is usable (which is what the operator wants) and to the overflow otherwise |
11. **Location: this ADR is the contract, and the implementation is Round 3.** The four rulings above, the hard
    rule of item 3, the semantics of items 4–9 and the two new config words land in `docs/spec.md` (§4, §4.2,
    §4.6, §6, §8), `design/DESIGN.md` (§12.4, §12.5, §12.6, §12.8, §12.9, §12.10.2, §12.10.4, §12.10.5,
    §12.10.8) and the book with this ADR, docs-first (AGENTS constraint 8). No code, no `config.example.yaml`
    change and no new conformance case is part of this ADR: the keys and the parser must land together
    (GAP-Q15), because `deny_unknown_fields` makes an example that carries a key the parser does not know
    unservable, and the implementing round allocates the case IDs (ADR-010/ADR-011's precedent).

## Alternatives considered

| Alternative | Why rejected |
|---|---|
| decide per request (prefer the primary whenever the local counter has room / whenever the last probe looks recent) | the route flips inside one session; every flip is a new upstream cache namespace, so the policy pays a re-prefill per flip to save money that the *same* re-prefill already spent. It also makes `prefix_continuity` — the project's fidelity metric — an artefact of the routing policy |
| the local counter is the authority (refuse or spill on it) | GAP-Q1: the allowance denominator may be a placeholder, so the refusal would be fabricated; and the only party that knows the real allowance is never asked. Kept as a warning and as a probe-deferral input, which is what it can honestly support |
| come back only at the plan's window reset (never probe) | it can idle on the metered account for up to a month after the plan became usable again (a provider-side reset, a re-issued allowance, a billing fix — all invisible to a monthly schedule). Kept as the `recover: none` option for operators who prefer no speculative attempts, not as the default |
| probe on every request until the primary answers | a doomed paid-latency attempt per request, which is the "retry the dead provider" failure mode ADR-011 item 4 was written to remove, one level down |
| probe mid-session to fix a session quickly | the session's next turn would pay a re-prefill for a *bet* — and if the bet loses, a second one. The boundary is where the bet is free of an in-flight session's context; the in-flight session is served by the next successful probe elsewhere (item 3's hard rule) |
| express it with the fallback chain alone (put the metered route after the plan route in `fallback`) | the chain is a failure mechanism: it does not prefer the plan while both are healthy, and it has no state, so it cannot express "spilled" vs "healthy" — every request re-decides, which is the flip above |
| make the policy per model instead of per provider-account (`account` on a model entry) | an allowance, a key and an endpoint belong to an account (ADR-011 item 4); a model-level flag would let one entry claim a subscription the account does not have, and would put the same fact in N places |
| a new pipeline stage between the selector and the guard | the guard chain already answers exactly this question ("may this request go on this route?"), and a new stage would need its own failure semantics, trace attribution and ordering story for no gain |
| hold the client's request open until the probe succeeds | turns the account's outage into this gateway's latency (ADR-011 rejected the same shape for `Retry-After`) |

## Rationale

- **The preference is not a heuristic, it is arithmetic:** inside the plan the marginal price is 0 and outside
  it is the model's real price, so the only question worth a policy is *when* to move and *how to come back* —
  which is exactly what items 1–5 answer.
- **Session granularity is what makes the preference affordable.** The plan's advantage is per-token, but the
  switch's cost is per-session (one re-prefill, ADR-011 item 9). Making the switch once per session instead of
  once per request is the difference between a policy that saves money and one that spends it.
- **The asymmetry in item 2 is the whole point of this ADR.** The upstream is authoritative for a fact only it
  knows (the allowance) and the config is authoritative for a fact only it can know (the window schedule);
  assigning each signal to the job it can honestly do is what keeps a placeholder from refusing work, and it is
  why the local counter keeps a real job (visible warning + probe deferral) instead of being deleted.
- **A probe is an experiment, and experiments belong at boundaries.** The boundary is where the cost of being
  wrong is one paid attempt with no in-flight session context to rewrite; item 3's hard rule is the sentence
  that keeps an implementation from "improving" the policy into a per-turn probe.
- **The event vocabulary pays for its growth.** One row per transition, with the reason and the recompute cost
  in the payload, is what makes "how often did we spill, why, and what did each spill cost" answerable from
  replayable records (ADR-005's channel, ADR-013's rollback trigger) — and item 8's refusals are what keep the
  vocabulary from growing by one kind per question someone asks.

## Consequences

- `docs/spec.md` §4 gains `account` on the provider entry and the top-level `plan_policy` section
  (`family` / `primary` / `overflow` / `on_primary_exhausted` / `recover` / `cooldown` /
  `overflow_monthly_cap_usd`, with per-key semantics and defaults) as **§4.6**; §4.2 gains one sentence
  (the family's `overflow` route precedes the global chain for a request inside a family); §6 gains
  `result.plan_switch` (from / to / reason / probe / `reprefill_tokens` / `switch_cost_nano`) and the
  recompute-cost convention of item 5; §8's `cost_cap_exceeded` row learns that the overflow cap is one of its
  triggers, and its `quota_exceeded` row that `on_primary_exhausted: block` is another. **No new
  `error.type`**, no new §6 field group and no `DecisionRecord.schema_version` bump
  (`plan_switch` is an optional field — DESIGN §12.6's rule, the same stance as `requested_model`).
- `design/DESIGN.md` §12.4 gains the "which signal may reject" refinement (item 2), §12.5 the two new parsing
  rules, §12.6 `ResultRec.plan_switch` + its record type, §12.10.2 the load-time validations (below),
  §12.10.4 the `plan_state` projection as **store DDL version 2** (an additive migration: one table, no row
  rewritten), §12.10.5 the `plan.switched` row and where it sits relative to the classifier and the next
  intent row.
- **`config.example.yaml` is not changed by this ADR** and must not be, since the keys and the parser land in
  the same round (GAP-Q15). Registered, with GAP-Q16 for the local-verdict refinement, so the drift is visible
  rather than remembered.
- Load-time validation added to §12.10.2's table: `account` ∈ {`coding_plan`, `api`}; `primary` and `overflow`
  are distinct roster routes whose `model` id equals `family`; `primary`'s provider is `coding_plan` and
  `overflow`'s is `api`; the family's model is covered by the primary provider's `quota.models` when that
  provider declares a plan; `overflow_monthly_cap_usd` ≥ 0 (and absent = no cap); at most one policy per
  family and one `plan_policy` section in v0.1 (a second family is an additive new key, the `state:`/`retention`
  precedent, never a reshaped one).
- `/health` reports the family's account state and its probe deadline, and `vadis stats` counts the switches
  with their verified cost — for ADR-011 item 4's reason, restated: a state nobody can see is
  indistinguishable from "the metered account is now the configuration".
- The implementing round allocates the conformance cases; **this ADR allocates none** (ADR-010/ADR-011's
  precedent, DESIGN §12.8's rule that IDs are allocated once by a human decision). Candidate coverage: a
  session spilled mid-flight keeps its account and does not probe; a new session after the cooldown probes and
  records `plan.switched`; a probe deferred by the window boundary; `block` refuses with a readable reason while
  `spill` continues on the metered account; the overflow cap refuses with `cost_cap_exceeded`; `plan_switch`'s
  presence/absence against `failover_from` (item 5's table).
- The book gains the user-facing section (what to configure, when it spills, that a spill really spends money,
  and that a switch loses the upstream prefix cache) — `book/cost-and-caching.md`.
- `vadis-core` gains the policy's pure predicate (probe admission + the rule order of item 9) and its config
  types; the routing decision itself stays where every guard decision lives. The `plan_state` projection is the
  same shape as the existing ones (ADR-009 item 5) and obeys the same rebuild rule.
- Honest boundaries: this is per-process local state in a single-operator gateway (spec §1) and it coordinates
  nothing across processes; the family is assumed to be *one* model id served by *two* accounts (a different
  pairing is a config defect, caught at load); and nothing here verifies a plan's allowance — it reacts to what
  the upstream says and comes back when the upstream allows it, which is the strongest claim the protocol
  layer supports.

## Publication note (2026-10-03, R62-2)

The `R<n>` labels and finding ids in this document name iterations of the project's own
private analysis loop — a loop that is not part of this repository, so no label here is
resolvable by a reader of it; they are kept as the provenance of the decision. This
publication pass removed only the dead-pointer class: every reference into that loop's
working tree (its file paths and round-record names, its state record, charter, execution
model and replay contract, its scripts and module names, and the kanban card ids), each
replaced by the neutral phrase the sentence needs. Nothing else moved — no figure,
threshold, `§`/`ADR`/`CONF` id, code sample or contract sentence.
