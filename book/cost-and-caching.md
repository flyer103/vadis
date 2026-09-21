# Cost and caching

Status: partly written for v0.1 — the plan-first section, the reporting notes and the "what is not on
today" section are written; the bullet list below is still an outline. This chapter explains the levers
and how to verify them, and it is careful about which lever exists: **cache fidelity and plan-first
routing are served; payload compression is not** (ADR-019, DESIGN §12.12). It does **not** contain price
numbers or type sketches: prices live in `config.example.yaml` (each entry with its `source` URL and
capture date), the accounting definitions live in `docs/spec.md` §7.

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
- **Breakeven**: the model-switch breakeven rule is implemented and is what prices a failover and a
  plan spill (the `cache.breakeven` keys in your config). It does not yet judge a transform — the
  mode's per-step figures are estimates until the paired measurement exists — and it will judge them
  by the same rule when one lands.
- **The transform pipeline** and its priority order: cache fidelity, then input-side
  payload reduction, then output-side discipline, then provider arbitrage (ADR-003). The
  **input-side tier is wired in v0.1 as an opt-in mode**: with a rule file configured and a
  request that asks for the mode, router trims tool-output payloads (build logs, search hits,
  diffs) by declared rules, records every edit in the trace and keeps the tee marker so nothing
  is silently lost. Output-side discipline and provider arbitrage beyond route choice remain
  future work. Anything that rewrites history in place — summarising, trimming by position,
  a rolling window — is deliberately out, because it breaks the upstream prefix cache and *raises* the
  bill (ADR-019). How to turn the mode on is described below under *Payload compression*.
- **Verified versus inferred**: only a measured usage difference counts as a saving, and a saving is a
  *difference between two worlds* — the request that ran and the one that did not. A local estimate is
  labelled `inferred` and can never be reported as measured; where no comparison happened, the honest
  figure is none at all ([`docs/spec.md` §7](../docs/spec.md)).
- **How to check the claim**: `router stats --config config.yaml --window 24h` to read the
  cost and cache report (the window is required, because a saving that does not state its window
  cannot be checked), and its `plan family` section for what the switches cost. `router replay`,
  which will recompute money over a fixed trace with the same code path, is planned and **not
  served** in v0.1 ([`docs/spec.md` §9.3](../docs/spec.md)).

## Two cache numbers: a predictor and a fact

When you look at a trace (or a `router stats` report) you will see two numbers that both talk
about the prefix cache. They are not rivals and they are not interchangeable:

- **`prefix.continuity` is a predictor.** Router computes it locally, from the shape of this
  request's conversation blocks against the previous request of the same session. It answers
  *"should* the upstream's prefix cache hit?" — 1.0 means "nothing at the front of your
  conversation changed, the cache should hold". It is a **local prediction**, computed before
  and independently of what the provider actually did, so it can be wrong: a provider may evict
  a cache for reasons router cannot see. A value below 1.0 on traffic you did not expect to
  change is a *trigger to go look*, not a verdict that money was lost.
- **The cache hit rate the provider reports is the fact.** Every response carries the
  provider's own usage accounting — how many of the input tokens were served from cache.
  That number is **measured by the party that owns the cache**, and it is the only one that
  counts when a saving or a loss is being claimed. If the predictor says 1.0 and the provider
  reports no cache hits, believe the provider — and expect the two to move together, because
  a genuine continuity drop almost always shows up in the next turn's measured hit rate.

Why both exist: the predictor is available immediately, per request, even when the provider
returns no usage at all; the fact is authoritative but only as good as the provider's own
reporting. When no usage came back, the hit rate is simply **unknown** — not zero, and never
written down as zero.

The precise definitions (what a block is, the block order, and the inferred/verified
accounting convention that decides which number a claim may rest on) live in one place:
[`docs/spec.md` §6](../docs/spec.md) and [§7](../docs/spec.md).

## Currency: what a price is denominated in

A price in the config is a number **and a unit**, and the unit is the provider entry's `currency`: `USD`
when you write nothing, or `CNY` for a deployment whose official page prices in yuan. It applies to that
entry's whole price table — every tier of every model — because those prices are one invoice from one
account.

- **Nothing is ever converted.** Router has no exchange rate, stores none, and does not translate a price, a
  cost, or a report total from one currency to another. A CNY table is transcribed as CNY from the CNY page;
  the accounting for a request served by that entry is in CNY. An "equivalent" figure computed from a rate
  would be a number no vendor published — and it would make the report depend on when someone looked up the
  rate, which is exactly what the fixed-point accounting exists to prevent.
- **Which page is evidence depends on the region, and that is a rule you can check.** Each entry carries the
  `source` URL it was transcribed from; a mainland entry cites the mainland page, an international entry the
  international one. One region's table is never used as the evidence for another region's number.
- **A tier the page prices in two tiers.** Some providers publish a cache-write price per cache lifetime
  tier. Your config records the one your requests fall into — the page's own default when a request states
  no lifetime — and the entry's comment names the other, so the number has a reason you can read.
- **Reports are per currency, never mixed.** The cost lines of `router stats` are printed once per currency
  present in the window, each labelled, and a window that holds two currencies shows **no** combined total:
  adding them is the one thing router will not do, and it will not guess which unit you meant. The counts
  (how many requests, how many switches) stay single, because a request is a request whichever account served
  it. If you want a single-currency report, route only through entries of that currency — that is a
  configuration choice, not a conversion.
- **One cap is denomination-bound.** `overflow_monthly_cap_usd` is a ceiling in **USD**; if you write it on a
  family whose metered route is priced in another currency, the config **refuses to load** instead of
  comparing a dollar ceiling with a yuan spend (spec §4.6). Leave it out, and the family still spills — the
  cap is a guardrail, not a requirement.

The precise rules live in [`docs/spec.md` §4.8](../docs/spec.md) (what each key is and is not) and
[§4.0](../docs/spec.md) (the price convention).

## Plan-first routing: the subscription first, the metered account as the spill

If you pay for a coding plan and also have pay-per-token access to the same model, you want
the plan used first and the metered API to be the fallback — not the other way round, and not
a coin flip per request. That is what plan-first routing configures.

**How you configure it.** Each provider entry says what it *is*: `account: coding_plan` for the
subscription, `account: api` for the metered one (and `api` is the default when you write
nothing). The same model therefore appears **twice** in the roster — once per account — and the
two entries carry their own `urls`, `wire_api` and API key, because those belong to the
account and not to the model:

```yaml
providers:
  - name: zai-plan
    account: coding_plan
    urls:
      anthropic: <your plan's Anthropic endpoint, in full>
    api_key_env: ZAI_CODING_API_KEY
    wire_api: anthropic            # a coding plan is usually handed out in the Anthropic format
    models: [ { id: glm-5.3, ... } ]   # taken from config.example.yaml (no price is copied here)
  - name: zai
    account: api                   # the default when the key is absent
    urls:
      chat: https://api.z.ai/api/paas/v4/chat/completions
    api_key_env: ZAI_API_KEY
    wire_api: chat
    models: [ { id: glm-5.3, ... } ]
```

The subscription entry may also carry the plan's own `quota` block — but it does not have to, and
that is deliberate: a plan page often publishes a monthly **price** and no token allowance, and a
plan whose allowance nobody can read is still a plan.

**When the two accounts do not use the same model id.** Vendors frequently serve the same model under
different ids on the coding endpoint and on the metered platform — a plan endpoint's `k3-256k` and the
platform's `kimi-k2.7-code` are one model to you and two strings to the vendors. Give both model entries the
same `family` tag and write that tag in `plan_policy.family`: the router then treats the two routes as one
family, while each route still sends the id **its own** provider expects, and the trace still records the
string your client asked for. When the ids already agree you write nothing and the tag is the id — which is
what a config written before tags existed keeps doing. Two things the tag is not: it is not a route (a client
still writes `provider/model`, and a bare tag resolves to nothing), and it is not inferred (the router never
guesses that `k3` and `kimi-k3` are the same model — you state it or it is not true)
([`docs/spec.md` §4.8](../docs/spec.md)).

One optional top-level `plan_policy` section then names the pair:

```yaml
plan_policy:
  family: <the family tag both routes' model entries carry>   # §4.8 — see below when their ids differ
  primary: <the coding-plan route>   # must be an account: coding_plan route
  overflow: <the metered route>      # must be an account: api route, distinct from primary
  on_primary_exhausted: spill        # or block
  recover: probe                     # or none
  cooldown: 15m
  # overflow_monthly_cap_usd: <usd>  # optional ceiling on metered spend
```

`plan_policy` appears **at most once** in v0.1 (one family). A second plan account that no policy
names is a legal roster entry — it is simply not routed specially yet.

**When it spills.** Only when the provider itself says the plan is exhausted — an upstream
`403`. Everything else about the design is about *not* spilling early: the plan's token
allowance may not be published at all (a plan page often publishes a monthly price and no
token count), so the local allowance counter that router keeps is a **warning**, not a verdict.
You will see it in the trace (`cost.quota_after`), and the state it describes is visible in
`GET /health`'s plan section ([`docs/spec.md` §9.1](../docs/spec.md)): the account the family is on,
its probe deadline, and why a probe would not be admitted yet. It can hold a probe back until the
plan's own reset has passed, but it will never refuse a request, and it will never move you to the
metered account on its own.

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
switched request's own measured usage. `GET /health` shows the current account and the probe
deadline, and `router stats` counts the switches and their verified cost — the two surfaces,
their fields and the rule that decides which of the two numbers a claim may rest on are
[`docs/spec.md` §9](../docs/spec.md).

**What router will not do.** It will not guess your plan's allowance, and it will not "fix" a
provider's quota. It reacts to what the provider says and comes back when the provider allows
it; the allowance itself stays the provider's business.

## Payload compression: an opt-in mode, off by default

In an agent loop, the tokens are not mostly the conversation — they are the **tool output** piling up
inside it: build logs, `grep` results, diffs, JSON returned by MCP servers. The client resends the whole
history on every turn, so those bytes are paid for again and again. Trimming them is the obvious lever,
and router already has the rule *data* for it ([`rules/tool_output.toml`](../rules/tool_output.toml):
strip noise and progress lines, truncate over-long lines, cap the total, and leave a marker line saying
what was dropped).

**None of it runs unless your config and the request both ask for it** — the rules are inert as
configured data until a request carries the mode header, and a request without the header is
byte-identical to what the client sent no matter what the file says. A content edit is in direct
tension with the promise the rest of this chapter rests on (the bytes reaching the provider are the
bytes the client sent, apart from two documented substitutions), so the contract was settled before
the code. What it settles:

- **It is a mode the client asks for, per request** — a request header, not a setting. A request that
  does not ask is byte-identical to what the client sent, modulo those two substitutions (router-owned
  fields removed, the provider's own model id written into `model`). That guarantee does **not** depend
  on your configuration: a config full of rules cannot weaken it, and no key in the file can turn the
  mode on for a request that did not ask (ADR-019).
- **Edits are declared, and only tool/environment payloads are touched** — never your words, never the
  system instruction, never the tool schemas. Every edit is recorded with the path it touched and the
  bytes before and after, so "what did router change" is a list you can read, not an investigation.
- **Three invariants hold**: the same content always produces the same bytes (no dependence on the turn
  number, the clock or randomness); a growing conversation *extends* the previous turn's bytes instead
  of rewriting them, which is what keeps the prefix cache alive; and a request that did not ask for the
  mode is unchanged even when matching rules are loaded.
- **An edit that drops bytes can still cost money.** The cache is keyed on the prefix, so rewriting
  something the provider had already cached is paid for once at the miss price — and that is why a rule
  is admitted only with its inline tests green, the cache regression measured, and its net gain
  measured rather than estimated (the D3 gate). Until such a pair of measurements exists, every
  per-transform figure in `router stats` is an estimate and must be read as one: nothing here is a
  reported saving yet.

To turn the mode on: declare the rule file in your config under `plugins:` (an entry of kind
`builtin/transform_rules` with `config.rules_file` pointing at it — see
[`config.example.yaml`](../config.example.yaml)), and have the **client** send
`X-Router-Transform: transform` on the requests that want it. Both halves are required: the
config decides which rules exist, the request decides whether they run, and a request without
the header is byte-for-byte your own bytes no matter what is configured. Every figure a rule
reports is an estimate until a paired on/off measurement exists, so the report gains its
measured per-transform saving only when that measurement lands — until then the rule file's
numbers are shapes, not savings.

## Authoritative sources

- [`config.example.yaml`](../config.example.yaml) — the price table and the only place
  price numbers exist, each with `source`.
- [`docs/spec.md` §4.0](../docs/spec.md) — the price convention (why this document holds
  no numbers, how peak windows are encoded as a multiplier).
- [`docs/spec.md` §7](../docs/spec.md) — the accounting convention: `verified` vs
  `inferred`, and the reporting requirements for any savings claim.
- [`docs/spec.md` §6](../docs/spec.md) — cache metrics exposed to the user
  (`cache_hit_rate`, `prefix_continuity` percentiles).
- [`docs/spec.md` §9](../docs/spec.md) — the reporting surfaces: `router stats`' report, each
  figure's provenance and its `verified` / `inferred` label, and what is not served yet.
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
- [`design/decisions/ADR-019-transform-mode-and-the-content-edit-contract.md`](../design/decisions/ADR-019-transform-mode-and-the-content-edit-contract.md)
  — the ruling behind the section above: why content compression is a mode the client asks for, what the
  byte promise degrades to inside it, and which number may be called a saving.
- [`docs/spec.md` §2.1](../docs/spec.md) and
  [`§4.4`](../docs/spec.md) — the mode's exact byte promise, the rule-file contract, and the one
  capability that is explicitly **not** implemented (retrieving a tee'd original).
- [`design/DESIGN.md` §12.12](../design/DESIGN.md) — where the pipeline lands: the shared composition
  step, the three invariants as assertions, and the failure semantics of every step.
