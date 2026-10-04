# ADR-013 — online iteration rails: shadow first, a session-bucketed canary, automatic rollback, and no mid-session exploration

- Status: accepted
- Date: 2026-09-19
- Related: ADR-002 (`ctx.isolate` gives two coexisting sets of bindings; `ctx.intercept` sets the sample rate or shadow switch without changing a binding; config-level coordination never needs a rebuild), ADR-003 (artifacts are revertible and individually accounted), ADR-005 (the trace is the only product-analysis-loop interface), ADR-008 (rule files load at startup and on a keyed config diff), ADR-009 (the store; a lost projection is rebuilt), ADR-010 (events and write ordering; the restart story), ADR-011 (`error.classified` and the demotion are this ADR's error-rate signal), ADR-012 (the ladder, the envelope, and the never-mutable list); AGENTS hard constraints 2 (content determinism) and 3 (the observation boundary); spec §4 (the plugin schema: `isolate`, `intercept`, `disabled`), §4.2 (fallback), §6 (metrics), §7 (the accounting convention); DESIGN §4 (realms and `intercept`, and the config-diff reload), §6 (prefix stability), §9 (`vadis replay`), §11 (risks); the loop charter (the per-round flow)

## Background

ADR-012's L1 means a parameter changes on **live traffic** with no human in the loop. Two properties of
this system make that different from an offline adoption:

1. **There is no undo for bytes already sent.** The cost is real the moment the request goes out, and the
   project's first-order lever is the prefix cache (AGENTS gotchas), so the cheapest way to lose money is
   to change a policy in the middle of a session: the next turn re-prefills the whole conversation at the
   miss price.
2. **The offline gate cannot answer the question.** `vadis replay` (DESIGN §9) answers "what would this
   policy have cost over the frozen corpus" — the corpus is yesterday's traffic by construction. The
   question a canary asks is "what is it costing now, on the traffic that is actually arriving".

The mechanism already exists and is named for this use: `ctx.isolate` gives the same key two independent
sets of bindings so two policy versions coexist, and `ctx.intercept` sets the sample rate or shadow switch
for one plugin without touching its binding (DESIGN §4, ADR-002). What is missing is the rail around it —
which is what this ADR adds, and the rails are mostly about *what the experiment is not allowed to do to
the measurement*.

## Decision

1. **Shadow first: zero impact, and it can never touch the cost gate.**
   Before a candidate is canaried it runs in an isolated realm (`ctx.isolate`) over the *same* inbound
   request: its decision path executes, its transform output is produced for comparison, and **nothing is
   sent upstream**.
   - What shadow establishes: divergence (candidate bytes differ from baseline bytes; candidate prefix
     blocks differ), the candidate transform's correctness on real content, and its local token estimate.
   - What shadow **cannot** establish: cost. There is no upstream `usage`, so a shadow result is
     `inferred`, and by spec §7 an inferred number cannot enter a gate. Shadow is a *correctness and
     divergence* filter, never a measurement.
   - Where a shadow's result goes **without inventing a trace field**: the shadow fiber's identity rides in
     the existing free-form `decision.plugin_chain[]` (variant-suffixed, e.g. `cache-guard@shadow`), and its
     byte/prefix verdict rides in the existing `transforms[].cache_impact` / `verdict = inferred` / `error`
     fields of the request it shadowed. The *comparison* is `vadis replay`'s job (DESIGN §9), not the
     serving path's. The suffix convention is fixed by the implementing round and recorded in DESIGN §12.6's
     value conventions; a dedicated trace field would be an additive spec §6 change (an optional field does
     not move `schema_version`, DESIGN §12.6) and is deliberately **not** taken here.

2. **The unit of exploration is the session, never the request or the turn.** Assignment is a pure function
   of `(session key, canary salt)`:

   ```
   bucket = u32::from_str_radix(&hex_sha256(session_key, salt)[..8], 16) % 10_000
   in_canary = bucket < share_basis_points
   ```

   - The digest is the sha256 of `session_key` concatenated with the salt, and the session key is the one
     the gateway already prefers (`prompt_cache_key`, spec §4/§6, AGENTS gotchas) — no new identity is
     invented.
   - **Eight hex chars, not four.** Four would let one modulo bias the split: buckets 0..5535 would get one
     extra draw out of 65,536 (+6.8% / -8.4% around the mean). With 32 bits the deviation is ~0.0002%, and
     the project's 16-hex hash convention is still just a prefix of the same digest.
   - The **salt is drawn per canary and recorded with it**, so the same sessions are not the guinea pigs
     every time. The share is declared as basis points (an integer), so the split is reproducible rather
     than floating-point.
   - **Rationale (the load-bearing rule of this ADR):** a mid-session policy change breaks the prefix cache
     for that session — the most expensive single operation the gateway can perform (ADR-011 item 9) — and
     it destroys the measurement, because a canary that changes mid-session compares a mixed policy against
     the baseline and attributes the re-prefill to the candidate.
   - Consequence, stated honestly: bucketing is decided at session start and **not re-rolled**. A
     **rollback** is the one event allowed to change a live session's policy; its cost (a re-prefill for the
     affected bucket) is bounded by the bucket's share and is recorded in the round file.
   - Content determinism (AGENTS constraint 2) is not violated: the bucket is a function of the session key
     and stable config — not of the turn number, the wall clock or an RNG — and two sessions legitimately
     taking different policies is *routing*, not payload rewriting.
   - No state is needed for the assignment (it is derived), so a restart re-derives it exactly (ADR-009/010's
     restart story is unchanged). The live artifact is config, applied as a keyed diff (ADR-002); the
     rollback history is the round file plus the loop state record.

3. **The blast radius is declared before the canary starts.** A canary declares, in its round file: the
   variant, the salt, the bucket share, the horizon (turns and sessions), the minimum sample
   (`envelope.min_sample`, ADR-012 item 3), and the tolerances item 4 compares against. One canary at a
   time, matching the project's serial pipeline (the loop charter).
   - **No promotion without the minimum sample**, and **no permanence by neglect**: a canary that reaches its
     horizon without the minimum sample is **rolled back**, not left running. Otherwise a "temporary"
     canary becomes the default with the measurement never having happened — which is the quiet way an
     experiment becomes an unmeasured production change.

4. **Automatic rollback triggers**, each evaluated over the same window against the baseline bucket, each
   behind the minimum-sample gate (below it the trigger does not fire: "inconclusive" is not "fail", but it
   is also not "pass"):

   | Trigger | Signal | Source |
   |---|---|---|
   | conformance failure | any CONF case fails on the candidate build | the pinned evaluator (ADR-012 item 2) |
   | cost over envelope | the canary bucket's verified $/1M exceeds the baseline's by more than the declared tolerance | the trace's `cost` and `usage`, verified convention only (spec §7) |
   | quality degradation | structured-output parse rate / sampled judge comparison below the baseline by the declared margin | the semantic-corroboration signal (the loop charter) |
   | error-rate spike | the canary bucket's `error.classified` rate over the content-independent reasons (`rate_limit`, `overloaded`, `server_error`, `timeout`) rises past the declared factor | ADR-011 item 8's events |

   - Restricting the error trigger to the **content-independent** reasons is deliberate: `content_policy_blocked`
     and `format_error` depend on what the user sent, so a bucket that happened to draw a refusal-heavy
     workload would otherwise trigger a rollback for the wrong reason.
   - A triggered rollback: write the previous artifact back (an artifact revert — nothing in the product
     changes, ADR-003), record the round file line and the loop state record, and apply a **cooldown**: the same
     candidate may not be re-proposed without new evidence, which is what stops an oscillating
     adopt-rollback-adopt loop from consuming the pipeline.
   - **The rollback path must exist before the canary starts.** It is a precondition of adopting, not a
     feature of the loop; an experiment whose only exit is a human waking up is not an L1 experiment.

5. **Only in-envelope parameters may be auto-adopted; everything else is a human gate.** A new model, a new
   plugin (including every L2 candidate), a new rule, or a new config key is never a canary outcome — it can
   be canaried only *after* a human has promoted it to the candidate slot (ADR-012: automatic to canary,
   human to default). A new *model* is additionally out of L1's reach because model choice carries a quality
   dimension that the frozen corpus only partly sees; spec §1's automatic-selection non-goal is unchanged by
   this ADR.

6. **What these rails do not authorize.** No automatic exploration of the selection policy beyond the
   envelope (spec §1's non-goal stands: `auto` remains a plugin slot, and a selector plugin is a new plugin
   and therefore a human gate). No multi-variant simultaneous testing — one canary at a time, because the
   sample sizes a single operator's traffic can generate do not support a matrix, and a matrix would divide
   the traffic that makes any single verdict sound. No re-bucketing inside a session (item 2). No canary
   whose measurement depends on a change to the corpus, the harness or the gate (ADR-012 item 2).

## Alternatives considered

| Alternative | Why rejected |
|---|---|
| per-request A/B (the usual online-experiment default) | it changes policy mid-session, breaking the prefix cache and confounding the metric being measured — the one thing this project cannot pay to learn |
| simultaneous multi-variant testing with automatic winner selection | needs sample sizes this deployment cannot reach; a matrix also splits the traffic that a single comparison needs to be conclusive |
| random request sampling without session stickiness | same mid-session problem, plus one session receiving both policies inside an hour |
| shadow-only iteration (never touch live traffic) | shadow has no upstream `usage`, so it can never answer the only question the cost gate asks |
| make the canary a product feature (a managed experiment framework in `crates/`) | it would put experiment state into the serving path and add a config surface to the spec for something the loop can do with primitives that already exist (`isolate` / `intercept` plus a keyed config diff) |
| rely on the offline replay alone and send L1 changes straight to default | the corpus is frozen and yesterday's traffic; the canary is the only instrument that sees today's |
| automatic promotion at the end of the horizon (no human) | outside the envelope the promotion is a judgement, not a measurement (ADR-012 item 5); and a metric moving is not a decision |
| roll back only on a human's verdict | an L1 adoption's whole point is the bounded unattended move; without an automatic exit the bound is not real |

## Rationale

- Every rail follows from one property of the system: **the session is the cache unit and the traffic is the
  experiment.** Anything that changes policy per request pays the first-order cost to answer a
  second-order question.
- Automatic rollback is what makes automatic adoption defensible: the loop may move a bounded knob *because
  the knob moves back by itself* when the declared signals go wrong — not because its first measurement was
  trusted.
- Shadow-before-canary is an ordering that costs nothing and removes the cheapest class of mistakes: a
  candidate that produces different bytes at all is caught without a single paid request.
- "No promotion without the minimum sample" plus the cooldown are the two rails that keep this an empirical
  mechanism rather than a habit: the first stops noise from becoming policy, the second stops an argument
  with the data from becoming a loop.

## Consequences

- DESIGN §11 gains the mid-session-experiment and auto-adoption risks. DESIGN §4's `isolate` and `intercept`
  rows, which already name the A/B and shadow use case, now have a normative consumer.
- The analysis loop's round template gains a canary block (variant, salt, share, horizon, minimum sample, declared
  tolerances) and a rollback record; the loop charter's per-round flow gains one step between Execute
  and Gate ("canary") for L1 adoptions. Its gate table is unchanged for L0, L2 and L3.
- ADR-011's `error.classified` events become load-bearing for a gate (item 4's error-rate trigger), which is
  why that event is written per classification rather than aggregated.
- `book/operations.md` gains one sentence for the operator: a canaried parameter touches a share of *new*
  sessions, is never changed mid-session, and reverts on its own — so an operator seeing a policy change in
  the trace knows where it came from.
- Honest boundary: a canary can miss a regression that only appears outside its sample (a rare provider, a
  long session, a workload the bucket did not draw). The verdict machinery is empirical (ADR-012 item 5),
  and the mitigation is the trigger list plus the cooldown, not a proof.
- Not covered here: the tier-B candidate's isolation requirements (timeout, crash isolation) belong to
  ADR-002 and the plugin protocol; and the mechanics of *which* artifact file the loop writes are a round
  template detail, not a product surface.

## Publication note (2026-10-03, R62-2)

The `R<n>` labels and finding ids in this document name iterations of the project's own
private analysis loop — a loop that is not part of this repository, so no label here is
resolvable by a reader of it; they are kept as the provenance of the decision. This
publication pass removed only the dead-pointer class: every reference into that loop's
working tree (its file paths and round-record names, its state record, charter, execution
model and replay contract, its scripts and module names, and the kanban card ids), each
replaced by the neutral phrase the sentence needs. Nothing else moved — no figure,
threshold, `§`/`ADR`/`CONF` id, code sample or contract sentence.
