# Cost and caching

Status: partly written for v0.1 — the plan-first section, the reporting notes and the "what is not on
today" section are written; the bullet list below is still an outline. This chapter explains the levers
and how to verify them, and it is careful about which lever exists: **cache fidelity and plan-first
routing are served; payload compression is not** (ADR-019, DESIGN §12.12). It does **not** contain price
numbers or type sketches: prices live in the **roster** — `providers.example.yaml`, the file the shipped
config names with `providers_file:` ([`docs/spec.md` §4](../docs/spec.md), §4.14) — each entry with its
`source` URL and capture date; the accounting definitions live in [`docs/spec.md` §7](../docs/spec.md).

vadis's cost model is built on one measured observation: on a real agent session the
prefix cache is fast and near-total, so the first-order lever is **keeping the prefix
stable**, and choosing a cheaper model is second-order.

## Outline

- **Prefix stability first**: every transform must be content-deterministic — the same
  content and stable config must always produce the same upstream bytes, and the prefix on
  turn N must still be a prefix on turn N+1. Breaking that silently raises the bill.
- **The five-tier price schema** (cache miss, cache hit, cache write, output, and a peak
  multiplier with time windows) — described by its shape in the config schema, never by
  numbers. Converted once at load time to fixed-point accounting, so the decision path is
  pure integer arithmetic. A vendor that publishes its prices in **input-length bands** is
  recorded band for band, and each request is priced at the band it falls in — see
  *When a price depends on how much you send* below.
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
  request that asks for the mode, vadis trims tool-output payloads (build logs, search hits,
  diffs) by declared rules, records every edit in the trace and keeps the tee marker so nothing
  is silently lost. Output-side discipline and provider arbitrage beyond route choice remain
  future work. Anything that rewrites history in place — summarising, trimming by position,
  a rolling window — is deliberately out, because it breaks the upstream prefix cache and *raises* the
  bill (ADR-019). How to turn the mode on is described below under *Payload compression*.
- **Verified versus inferred**: only a measured usage difference counts as a saving, and a saving is a
  *difference between two worlds* — the request that ran and the one that did not. A local estimate is
  labelled `inferred` and can never be reported as measured; where no comparison happened, the honest
  figure is none at all ([`docs/spec.md` §7](../docs/spec.md)).
- **Response caching**: an exact-match, session-scoped *replay* of a response vadis recorded — **off by
  default**, and the one lever whose saving can never be measured (so it is excluded from every gate). The
  `verified`/`inferred` rule above is what makes that exclusion checkable; see *Response caching* below.
- **How to check the claim**: `vadis stats --config config.yaml --window 24h` to read the
  cost and cache report (the window is required, because a saving that does not state its window
  cannot be checked), and its `plan family` section for what the switches cost. `vadis replay`,
  which will recompute money over a fixed trace with the same code path, is planned and **not
  served** in v0.1 ([`docs/spec.md` §9.3](../docs/spec.md)).

## Two cache numbers: a predictor and a fact

When you look at a trace (or a `vadis stats` report) you will see two numbers that both talk
about the prefix cache. They are not rivals and they are not interchangeable:

- **`prefix.continuity` is a predictor.** Vadis computes it locally, from the shape of this
  request's conversation blocks against the previous request of the same session. It answers
  *"should* the upstream's prefix cache hit?" — 1.0 means "nothing at the front of your
  conversation changed, the cache should hold". It is a **local prediction**, computed before
  and independently of what the provider actually did, so it can be wrong: a provider may evict
  a cache for reasons vadis cannot see. A value below 1.0 on traffic you did not expect to
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

- **Nothing is ever converted.** Vadis has no exchange rate, stores none, and does not translate a price, a
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
- **Reports are per currency, never mixed.** The cost lines of `vadis stats` are printed once per currency
  present in the window, each labelled, and a window that holds two currencies shows **no** combined total:
  adding them is the one thing vadis will not do, and it will not guess which unit you meant. The counts
  (how many requests, how many switches) stay single, because a request is a request whichever account served
  it. If you want a single-currency report, route only through entries of that currency — that is a
  configuration choice, not a conversion.
- **One cap is denomination-bound.** `overflow_monthly_cap_usd` is a ceiling in **USD**; if you write it on a
  family whose metered route is priced in another currency, the config **refuses to load** instead of
  comparing a dollar ceiling with a yuan spend (spec §4.6). Leave it out, and the family still spills — the
  cap is a guardrail, not a requirement.

The precise rules live in [`docs/spec.md` §4.8](../docs/spec.md) (what each key is and is not) and
[§4.0](../docs/spec.md) (the price convention).

## When a price depends on how much you send

Some vendors do not publish one price per model. They publish one price per **band of input length**: the same
model costs one rate while your request stays under a threshold, and another rate once it crosses it. Vadis
records that as the page publishes it — band for band — and prices each request at the band that request falls
into.

What that means for you as a client:

- **One band prices the whole request, not part by part.** Crossing a threshold does not add a surcharge to the
  part that went over: it changes the rate the *entire* request is billed at, which is what the vendor's own
  table does. A prompt that grows past a threshold can therefore cost noticeably more per token from that
  request on — that is the vendor's published step, and vadis reproduces it rather than hiding it.
- **The band follows the prompt you sent, as the provider counted it** — the whole input, cached or not. It
  never depends on how much of your conversation the cache happened to serve, so the same conversation is
  priced the same way whether the cache was warm or cold. (The *rate* does not move with the cache; the cache
  still decides how much of your input is billed at the cheap hit price inside that band.)
- **Peak hours still multiply whatever band applies.** The two are separate: the band picks the rate, the time
  of day multiplies it. Enabling peak pricing cannot move you into another band, and a long prompt cannot move
  you out of a peak window.
- **A model whose page publishes no bands is written flat** — one price for every length, exactly as before.
  That is what most of the shipped example roster does, and those entries did not have to change.

Where this lives in your config: the provider entry's per-model price block, either as the flat four prices or
as a list of bands. The full shape, the load-time refusals (a band with no ceiling, two bands claiming the same
input, a band priced at zero, a multiplier written inside a band) and the boundary semantics are
[`docs/spec.md` §4.10](../docs/spec.md); the figures themselves, each with the citation of the band it came
from, are in [`providers.example.yaml`](../providers.example.yaml). One limitation worth knowing: if a page prices
its bands in a unit that cannot be compared with your prompt's token count, the entry stays **flat** rather
than guessing — a flat price you can check beats a band the vadis inferred.

### Where the price table and its citations live

The rule is that a price lives in the **roster** — the `providers:` block — and the citation lives beside it:
each model's price carries the `source` URL and capture date of the page it was transcribed from, and those
comments are provenance a reader must treat as part of the file, not decoration
([`docs/spec.md` §4.0](../docs/spec.md)). What is a contract with a second shape, and what is shipped, is worth
being precise about:

- **The roster has a file of its own (shipped).** The shipped example is a pair: the server's own settings in
  `config.example.yaml`, the roster in `providers.example.yaml`, which that config names with
  `providers_file:` and which holds every price number in this project. The price table you edit is the roster
  file, and the citations that belong to it travel with it, comments and all
  ([`docs/spec.md` §4.14](../docs/spec.md)).
- **The roster may also stay inline (equally legal).** The same block can be written straight into the config as
  `providers:`; exactly one of the two keys is written, and both written or neither is a refusal at load.
  `vadis setup` writes the pair for a new configuration and edits whichever of the two files holds the entry.

Either way the money is unchanged: the same prices, the same per-model citations, the same refusals at load,
and this chapter still carries no numbers — the figures and their sources are in the roster, one block in one
file, never two copies at once.

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
    models: [ { id: glm-5.3, ... } ]   # taken from providers.example.yaml (no price is copied here)
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
same `family` tag and write that tag in `plan_policy.family`: the vadis then treats the two routes as one
family, while each route still sends the id **its own** provider expects, and the trace still records the
string your client asked for. When the ids already agree you write nothing and the tag is the id — which is
what a config written before tags existed keeps doing. Two things the tag is not: it is not a route (a client
still writes `provider/model`, and a bare tag resolves to nothing), and it is not inferred (the vadis never
guesses that `k3` and `kimi-k3` are the same model — you state it or it is not true)
([`docs/spec.md` §4.8](../docs/spec.md)).

One optional top-level `plan_policies:` **list** then names the pair — one entry per family (the
shipped template carries it, and `vadis setup` walks it one family at a time):

```yaml
plan_policies:
  - family: <the family tag both routes' model entries carry>   # §4.8 — see below when their ids differ
    primary: <the coding-plan route>   # must be an account: coding_plan route
    overflow: <the metered route>      # must be an account: api route, distinct from primary
    on_primary_exhausted: spill        # or block
    recover: probe                     # or none
    cooldown: 15m
    # overflow_monthly_cap_usd: <usd>  # optional ceiling on metered spend
```

**One of the two spellings is written, never both**: either that list, or the single `plan_policy:`
section (one family, no list). A second plan account that no entry names is a legal roster entry — it
is simply not routed specially yet.

**When it spills.** Only when the provider itself says the plan is exhausted — an upstream
`403`. The verdict is read off the provider's own answer, which vadis reads before it decides —
including on a streaming request, where the answer's error body is read before the classification
runs, so the same `403` moves you whether or not you stream. Everything else about the design is about *not*
spilling early: the plan's token
allowance may not be published at all (a plan page often publishes a monthly price and no
token count), so the local allowance counter that vadis keeps is a **warning**, not a verdict.
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
plan's own reset point when vadis knows it) the next new session probes the plan; a success is
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
switched request's own measured usage. When the route you moved to publishes its prices in
input-length bands, that decision-time figure is computed from the model's lowest band — it is an
estimate made before the next request exists, and the band that request actually falls in is what
its own cost line shows. `GET /health` shows the current account and the probe
deadline, and `vadis stats` counts the switches and their verified cost — the two surfaces,
their fields and the rule that decides which of the two numbers a claim may rest on are
[`docs/spec.md` §9](../docs/spec.md).

**What vadis will not do.** It will not guess your plan's allowance, and it will not "fix" a
provider's quota. It reacts to what the provider says and comes back when the provider allows
it; the allowance itself stays the provider's business.

**A failover jump back into a family re-enters its policy.** The family's order above is not only
for the route your client named: when the walk reaches a `fallback` entry that is a route inside a
plan family, that route passes through the family's policy before it is attempted, so the request
is routed by the family's state — its plan tier first — rather than billed at whatever the jump
happened to land on. A request that fails over onto a family's metered route is therefore served by
that family's plan while the plan is usable; the move is recorded as a switch, and a `fallback`
entry outside every family is attempted as before.

**The boundary this section does not cross.** What vadis states is that a request **reached the
plan account on a protocol the vendor documents at the endpoint your roster names** — nothing more.
It does **not** state, and must not be read as stating, that the plan's allowance is *consumed* the
way you expect. Whether traffic that arrives through a gateway counts against a coding-plan
allowance is a **vendor policy** question: the vadis cannot observe the vendor's meter, so no claim
in this book can answer it. Read your plan's own terms — see
[Getting started](getting-started.md) — before you point it at a gateway.

## Several keys, several plans: drain them all, then spend

The section above describes one plan account and one metered account. Most people hold more than
that: several API keys, some of them **coding plans** (possibly from different vendors), some of
them **pay-per-request** accounts. A plan is cheaper — often dramatically — but it has a quota, and
running one dry means **waiting**, which is exactly what interrupts a working session. vadis is built
to hold all of them at once and use them in the right order:

1. **every key you configure is usable** — a provider entry may carry a *pool* of credentials
   (`api_keys: [ENV_A, ENV_B]`, in the order you write them) instead of the single `api_key_env`.
   A credential that comes back `401`/`429` is rotated out for the next one **of the same provider**
   before the request moves on — a quota exhaustion is *not* a bad key, so it never rotates;
2. **every plan is drained before anything is spent** — any `account: coding_plan` entry whose model
   carries the family's tag is part of that family's **plan tier**, walked from `primary` and then in
   the order your roster lists them. When one plan's quota runs out, the next plan takes over and the
   family **stays in-plan**;
3. **only then the metered accounts** — the family's `account: api` entries are ranked by their own
   published prices (`overflow_selection: cheapest`) and tried in that order, cheapest first. Ranking
   is by price alone — no invented "quality" score, and nothing vadis cannot point at a source for;
4. **and back to a plan the moment one recovers** — the same probe described above, still only at the
   start of a session: the next new session tries the plan tier's head again, and if it answers, the
   family returns to the plan and the spending stops.

**How you turn it on.** Give the providers that belong together the **same `family` tag** on their
model entries — that tag *is* the set of candidates, so adding a second plan or a second metered
account is a roster edit and nothing else. Then name the pair's anchor and the ranking mode:

```yaml
plan_policy:
  family: <the tag your plan and metered entries carry>
  primary: <the plan route the drain starts from>
  overflow: <the metered route — the anchor, and the `block`/cap subject>
  overflow_selection: cheapest    # or `declared`, today's behaviour
```

With several families (say, one plan pair per model you use), write them as a list under
`plan_policies:` — the parser and the routing have accepted the list since ADR-049, and the shipped
template carries it (`vadis setup` walks it one family at a time). One of the two keys, never both —
as with the roster's own two spellings, a config that writes both is refused at load, naming both keys.

**The order is your roster order.** Inside the plan tier every member costs the same (zero marginal
price), so there is no price to rank them by — the order is yours, expressed by where you list the
entries and by which one `primary` names. If you re-order the roster you re-order the drain, which is
worth knowing before you shuffle the file.

**How you read it.** `GET /health`'s plan section tells you **which route the family is on right now**
(`route`) and, under `cheapest`, the exact order it will walk (`metered_candidates`, with the rank key
it ordered on — check it against your roster). Each provider's pool is reported per credential name
(`keys[]`), never by value. In the trace, a move *inside* the plan tier is `plan_switch` with
`reason: plan_exhausted` — the family changed plan, still in-plan; leaving the tier for the metered
accounts is the familiar `primary_exhausted`, and coming back is `primary_recovered`. The credential
that served a request appears as `decision.key_index` — an index, never a secret.

**What it will not do.** It will not re-decide per request: a session keeps the account it started
with, and a ranking that changes mid-session (a reload, a competitor getting cheaper) moves **new**
sessions only. That rule is what keeps the prefix cache intact, and the cache is the larger saving of
the two — a router that "optimises" per request costs more than it saves. It will also not invent an
allowance or a quality score: it reads what your providers publish and your roster declares, and
nothing else. And it does not claim your plan's allowance behaves the way you hope: whether traffic
that arrives through a gateway counts against a coding-plan allowance is vendor policy, not something a
roster or a trace can answer (the boundary stated under
[Plan-first routing](#plan-first-routing-the-subscription-first-the-metered-account-as-the-spill)).

The precise rules — the tier's discovery, the state machine, the one reason word this adds, and every
load-time refusal that goes with them — are
[`docs/spec.md` §4.6/§4.6.1](../docs/spec.md) and
[`design/decisions/ADR-049`](../design/decisions/ADR-049-plan-first-fan-out-key-pool-and-cheapest-metered.md).

## Payload compression: an opt-in mode, off by default

In an agent loop, the tokens are not mostly the conversation — they are the **tool output** piling up
inside it: build logs, `grep` results, diffs, JSON returned by MCP servers. The client resends the whole
history on every turn, so those bytes are paid for again and again. Trimming them is the obvious lever,
and vadis already has the rule *data* for it ([`rules/tool_output.toml`](../rules/tool_output.toml):
strip noise and progress lines, truncate over-long lines, cap the total, and leave a marker line saying
what was dropped).

**None of it runs unless your config and the request both ask for it** — the rules are inert as
configured data until a request carries the mode header, and a request without the header is
byte-identical to what the client sent no matter what the file says. A content edit is in direct
tension with the promise the rest of this chapter rests on (the bytes reaching the provider are the
bytes the client sent, apart from two documented substitutions), so the contract was settled before
the code. What it settles:

- **It is a mode the client asks for, per request** — a request header, not a setting. A request that
  does not ask is byte-identical to what the client sent, modulo those two substitutions (vadis-owned
  fields removed, the provider's own model id written into `model`). That guarantee does **not** depend
  on your configuration: a config full of rules cannot weaken it, and no key in the file can turn the
  mode on for a request that did not ask (ADR-019).
- **Edits are declared, and only tool/environment payloads are touched** — never your words, never the
  system instruction, never the tool schemas. Every edit is recorded with the path it touched and the
  bytes before and after, so "what did vadis change" is a list you can read, not an investigation.
- **Three invariants hold**: the same content always produces the same bytes (no dependence on the turn
  number, the clock or randomness); a growing conversation *extends* the previous turn's bytes instead
  of rewriting them, which is what keeps the prefix cache alive; and a request that did not ask for the
  mode is unchanged even when matching rules are loaded.
- **An edit that drops bytes can still cost money.** The cache is keyed on the prefix, so rewriting
  something the provider had already cached is paid for once at the miss price — and that is why a rule
  is admitted only with its inline tests green, the cache regression measured, and its net gain
  measured rather than estimated (the D3 gate). Until such a pair of measurements exists, every
  per-transform figure in `vadis stats` is an estimate and must be read as one: nothing here is a
  reported saving yet.

To turn the mode on: declare the rule file in your config under `plugins:` (an entry of kind
`builtin/transform_rules` with `config.rules_file` pointing at it — see
[`config.example.yaml`](../config.example.yaml)), and have the **client** send
`X-Vadis-Transform: transform` on the requests that want it. Both halves are required: the
config decides which rules exist, the request decides whether they run, and a request without
the header is byte-for-byte your own bytes no matter what is configured. Every figure a rule
reports is an estimate until a paired on/off measurement exists, so the report gains its
measured per-transform saving only when that measurement lands — until then the rule file's
numbers are shapes, not savings.

## Response caching: exact match only, off unless you ask

A different kind of cache, and the smallest useful one. When a request is **byte-identical** to one this
session has already sent — the same request bytes, in the same session, on the same protocol, under the same
configuration and with the same transform mode — vadis can return the response bytes it recorded then instead
of calling the provider again. Change one byte, or ask from another session, and it is an ordinary request.
It is **off by default**, and turning it on takes both halves: a `plugins:` entry of kind
`builtin/response_cache` **and** `config.enabled: true` inside it — listing the plugin without that switch does
nothing, and no later config edit can turn it on behind your back. What it will **not** claim is the interesting
half: a served repeat is a **replay, not a prediction** — those bytes are what the provider returned earlier, so
vadis asserts nothing about what it would answer now — and because no call happens, nothing is measured: the
figure is labelled `inferred`, it is kept out of every rate and every sum, and no gate counts it
([`docs/spec.md` §4.17](../docs/spec.md) and §7; `design/decisions/ADR-042-exact-match-cache.md`). It pays when
a client re-sends an identical request inside one session — a retry after a timeout, a re-run of an unchanged
command. On the corpora this project measures against, no request is a byte-identical repeat of another in its
session, so treat it as a fidelity-preserving convenience rather than a cost lever: the numbers to weigh it are
not in this repository yet, and this chapter will not invent them.

## Authoritative sources

- [`providers.example.yaml`](../providers.example.yaml) — the shipped roster: the price table and the only
  place price numbers exist, each with `source`. The shipped config names it with `providers_file:`
  ([`docs/spec.md` §4.14](../docs/spec.md)); the rule — one roster, one shipped copy — is what the pair keeps.
- [`docs/spec.md` §4.0](../docs/spec.md) — the price convention (why this document holds
  no numbers, how peak windows are encoded as a multiplier).
- [`docs/spec.md` §4.10](../docs/spec.md) — banded pricing: the shape a banded entry takes,
  which band prices a request, the load-time refusals, and the citation rule per band.
- [`docs/spec.md` §7](../docs/spec.md) — the accounting convention: `verified` vs
  `inferred`, and the reporting requirements for any savings claim.
- [`docs/spec.md` §6](../docs/spec.md) — cache metrics exposed to the user
  (`cache_hit_rate`, `prefix_continuity` percentiles).
- [`docs/spec.md` §9](../docs/spec.md) — the reporting surfaces: `vadis stats`' report, each
  figure's provenance and its `verified` / `inferred` label, and what is not served yet.
- [`design/DESIGN.md` §5](../design/DESIGN.md) and [§6](../design/DESIGN.md) — the cost
  engine and the cache policy.
- [`docs/spec.md` §4.6](../docs/spec.md) — `account` and `plan_policy`: the per-key
  semantics, the defaults and the hard rules of plan-first routing.
- [`docs/spec.md` §4.6.1](../docs/spec.md) — the fan-out's own contract: a provider entry's
  credential pool, the `cheapest` metered ranking and the `plan_policies:` list, with every
  load-time refusal that goes with them.
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
