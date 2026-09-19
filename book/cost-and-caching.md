# Cost and caching

Status: partly written. The plan-first section below is written for v0.1; the rest of the
chapter is still an outline. This chapter explains the levers and how to verify them. It does
**not** contain price numbers or type sketches: prices live in `config.example.yaml`
(each entry with its `source` URL and capture date), the accounting definitions live in
`docs/spec.md` §7.

router's cost model is built on one measured observation: on a real agent session the
prefix cache is fast and near-total, so the first-order lever is **keeping the prefix
stable**, and choosing a cheaper model is second-order.

## Outline

- **Prefix stability first**: every transform must be content-deterministic — the same
  content and stable config must always produce the same upstream bytes, and the prefix on
  turn N must still be a prefix on turn N+1. Breaking that silently raises the bill.
- **The five-tier price schema** (cache miss, cache hit, cache write, output, and a peak
  multiplier with time windows) — described by its shape in the config schema, never by
  numbers. Converted once at load time to fixed-point accounting, so the decision path is
  pure integer arithmetic.
- **Where prices come from**: the provider's official pricing page, recorded per model as
  a `source` URL plus capture date. Estimated or remembered prices are not acceptable.
- **Quotas**: subscription plans are declared per provider and can only reference that
  provider's own models; what happens when a quota runs out is a configured policy. The
  plan-first section below is the written version of this bullet.
- **Breakeven**: sticky sessions and any cache-writing transform are judged against an
  explicit breakeven rule rather than intuition.
- **The transform pipeline** and its priority order: cache fidelity, then input-side
  payload reduction, then output-side discipline, then provider arbitrage. Every step is
  reversible, declarative and individually accounted.
- **Verified versus inferred**: only a measured usage difference counts as a saving; a
  local tokenizer estimate is labelled as such and can never be reported as measured.
- **How to check the claim**: `router stats` to read the cost and cache report,
  `router replay` to recompute money over a fixed trace with the same code path.

## Plan-first routing: the subscription first, the metered account as the spill

If you pay for a coding plan and also have pay-per-token access to the same model, you want
the plan used first and the metered API to be the fallback — not the other way round, and not
a coin flip per request. That is what plan-first routing configures.

**How you configure it.** Each provider entry says what it *is*: `account: coding_plan` for the
subscription, `account: api` for the metered one (and `api` is the default when you write
nothing). One optional top-level `plan_policy` section then names the pair:

```yaml
plan_policy:
  family: <the model id both routes serve>
  primary: <the coding-plan route>
  overflow: <the metered route>
  on_primary_exhausted: spill        # or block
  recover: probe                     # or none
  cooldown: 15m
  # overflow_monthly_cap_usd: <usd>  # optional ceiling on metered spend
```

**When it spills.** Only when the provider itself says the plan is exhausted — an upstream
`403`. Everything else about the design is about *not* spilling early: the plan's token
allowance may not be published at all (a plan page often publishes a monthly price and no
token count), so the local allowance counter that router keeps is a **warning**, not a verdict.
You will see it in the trace (`cost.quota_after`) and in `/health`, and it can hold a probe
back until the plan's own reset has passed — but it will never refuse a request, and it will
never move you to the metered account on its own.

**It sticks per session.** A session stays on one account. Leaving the plan is forced (the plan
is gone); coming back is a *probe*, and a probe only ever happens at the **start** of a session
— a new session, or a session's first request. A session that is already running on the metered
account is never re-tried mid-way: it is moved back only once the provider has shown, elsewhere,
that the plan works again. After the cooldown (15 minutes by default, and never before the
plan's own reset point when router knows it) the next new session probes the plan; a success is
recorded and the following sessions go back to it.

**A spill really spends money.** While the family is on the metered account, every request is
billed at that model's pay-per-token price — the plan's zero marginal cost is gone. If that is
not acceptable, two knobs say so instead of you watching the bill:

- `on_primary_exhausted: block` — refuse the request with a readable reason (`quota_exceeded`)
  rather than serving it from the metered account. This is the "fail rather than spend" mode.
- `overflow_monthly_cap_usd` — a ceiling on the family's metered spend in a calendar month.
  Once the month's *measured* spend has reached it, the family's metered requests are refused
  (`cost_cap_exceeded`). The request that crosses the ceiling is served (nobody can know its
  cost beforehand), so the overshoot is at most one request.

**The cost nobody sees coming: a switch loses the upstream prefix cache.** Moving between
accounts changes which upstream cache holds your conversation, so the first request on the
other account re-prefills the whole prefix once. Inside the plan that re-prefill is not billed
separately (you have already paid for the subscription); on the metered account it is billed at
the cache-miss price. This is exactly why the policy switches as rarely as it does — once per
session, not once per request — and why a fix that "re-decides per request" would be worse than
no policy at all.

**How you read it.** On the request whose account moved, the trace's `result.plan_switch` says
`from` → `to`, why (`primary_exhausted`, `primary_cooling_down`, `primary_recovered`), whether
it was a probe, the re-prefill size and its price. That price starts as an *inferred* figure
(it is computed from the prefix, not from a bill); it becomes *verified* in the shadow of the
switched request's own measured usage. `/health` shows the current account and the probe
deadline, and `router stats` counts the switches and their verified cost.

**What router will not do.** It will not guess your plan's allowance, and it will not "fix" a
provider's quota. It reacts to what the provider says and comes back when the provider allows
it; the allowance itself stays the provider's business.

## Authoritative sources

- [`config.example.yaml`](../config.example.yaml) — the price table and the only place
  price numbers exist, each with `source`.
- [`docs/spec.md` §4.0](../docs/spec.md) — the price convention (why this document holds
  no numbers, how peak windows are encoded as a multiplier).
- [`docs/spec.md` §7](../docs/spec.md) — the accounting convention: `verified` vs
  `inferred`, and the reporting requirements for any savings claim.
- [`docs/spec.md` §6](../docs/spec.md) — cache metrics exposed to the user
  (`cache_hit_rate`, `prefix_continuity` percentiles).
- [`design/DESIGN.md` §5](../design/DESIGN.md) and [§6](../design/DESIGN.md) — the cost
  engine and the cache policy.
- [`docs/spec.md` §4.6](../docs/spec.md) — `account` and `plan_policy`: the per-key
  semantics, the defaults and the hard rules of plan-first routing.
- [`design/decisions/ADR-014-plan-first-routing.md`](../design/decisions/ADR-014-plan-first-routing.md)
  — why the preference is session-scoped, why the upstream (not the local counter) is the
  authority on exhaustion, and why a probe only happens at a session boundary.
- [`design/DESIGN.md` §12.10.8](../design/DESIGN.md) — where the policy lands: the guard
  rule, the account-state projection and the trace field.
- [`design/decisions/ADR-003-cost-pipeline.md`](../design/decisions/ADR-003-cost-pipeline.md)
  and [`ADR-006`](../design/decisions/ADR-006-integer-nanousd-accounting.md) — the pipeline
  discipline and the fixed-point money rule.
