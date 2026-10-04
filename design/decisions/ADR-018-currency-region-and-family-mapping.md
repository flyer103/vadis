# ADR-018 — currency, the region field, and the route family tag (money is never mixed, a deployment is an entry, a family is a tag)

- Status: accepted
- Date: 2026-09-21
- Related: **ADR-006** (integer fixed-point accounting — **amended here**: the fixed point keeps its scale, the *unit* becomes data), **ADR-014** (plan-first routing — **generalized here**: its "one model id served by two accounts" assumption becomes "one family tag served by two routes"), ADR-005 (the trace is the analysis truth and is read without the config), ADR-009/ADR-010 (one local store; the log is the truth), ADR-011 (one classifier), ADR-012 (the measurement is not part of the search space); spec §2 (the byte boundary and the outbound `model`), §3 (routes and aliases), §4 + §4.0 (the config schema and the price convention), §4.6 (plan-first routing), §6 (the observation contract), §7 (the accounting convention), §8 (errors), §9.1 / §9.2 (the reporting surfaces); DESIGN §12.4 (the cost engine), §12.5 (config types), §12.6 (DecisionRecord), §12.8 (the case registry), §12.9 (the gap register), §12.10.2 (config landing + load validation)

## Background

Two changes were chosen by the owner during R6 and deliberately deferred to a docs-first round, because both
change the **shape** of the config rather than its values:

1. **The CN price tables are published in CNY.** `config.example.yaml` carries, to this day, only commented
   placeholders for the mainland endpoints and the note "the CN entries' price tables land with the currency
   (CNY) round — CNY prices are never FX-converted into this file's USD basis". Until this ADR, the roster
   had no way to say *what unit* a price is in, so those tables could not be transcribed at all.
2. **A vendor's coding-plan endpoint and its metered platform do not serve the same model id.** `plan_policy.family`
   was defined (ADR-014 item 4) as "the model id **both** routes carry", which makes exactly the pairing the
   owner asked for unexpressible: the Kimi coding endpoint serves `k3` / `k3-256k` / `kimi-for-coding` /
   `kimi-for-coding-highspeed`, the Kimi platform serves `kimi-k3` / `kimi-k2.7-code` / `kimi-k2.7-code-highspeed`.

Three facts make the naive answers wrong, and they are the reason this is an ADR rather than three config keys:

- **A currency is not a formatting choice.** The whole ledger is `NanoUsd` (ADR-006): one accounting unit, one
  rounding point, integer arithmetic end to end, and the arithmetic is only isomorphic because there is a single
  unit. A second currency that is folded in at any exchange rate — even a published one, even once — destroys
  that property *and* violates AGENTS constraint 5, which is about numbers nobody can read off an official page.
  So the second currency has to be carried as data, not converted.
- **The trace is read without the config** (ADR-005: `vadis stats` / `vadis replay` / the analysis loop). A trace field
  that says only `184000 nano` is ambiguous the moment a CNY route exists, and a consumer that guesses USD is
  silently wrong in exactly the way this repository forbids.
- **The CN plans do not publish token allowances.** `GLM Coding Plan` publishes **credits** (Lite 2,000 per 5
  hours / 10,000 per week; Pro 12,000 / 60,000; Max 28,000 / 140,000) with a deduction formula whose
  coefficients are published (GLM-5.3: input 6.9, cached 1.7, output 24, per 10,000 credits), and `Kimi Code`
  publishes **usage windows** (a 5-hour rolling window plus a monthly total; the older schema had a weekly
  window as well). v0.1's `quota` is `{models, window: monthly, tokens, reset_day, over_quota}` — a token count
  on a monthly window. Neither CN plan's published allowance is a token count, so a `tokens:` value there would
  be **fabricated** (GAP-Q1's class of error, one level worse: Q1's placeholder at least had a denominator).

### The official sources read for this ADR

Every figure below was read from the vendor's own page on **2026-09-21 (CST)**; none is estimated, remembered
or converted. (The three pages reached from `platform.moonshot.cn` / `platform.moonshot.ai` answer on the
`kimi.com` / `kimi.ai` host today — a 301 the operator measured in R6; the content is the same document set.)

| Region | Page | What it is cited for |
|---|---|---|
| CN | `https://open.bigmodel.cn/pricing` | GLM-5.3: context 1M, input ¥8/M, output ¥28/M, cache hit ¥2/M, cache storage "限时免费"; GLM-5.3-Flash: context 1M, input ¥0.8/M (a limited-time 50 % promotion lists ¥0.4/M), output ¥2.8/M, cache hit ¥0.23/M |
| CN | `https://open.bigmodel.cn/glm-coding` | Coding Plan tiers: Lite ¥94.4/month (list ¥118), Pro ¥430.4 (¥538), Max ¥862.4 (¥1078); quarterly × 0.8, yearly × 0.7 |
| CN | `https://docs.bigmodel.cn/cn/coding-plan/overview` | the plan's allowance is **credits**: Lite 2,000/5 h + 10,000/week, Pro 12,000/60,000, Max 28,000/140,000; the deduction formula and coefficients (GLM-5.3: input 6.9, cached 1.7, output 24 per 10,000); peak = Mon–Fri 14:00–18:00 UTC+8 at 1×, off-peak 0.5× |
| CN | `https://docs.bigmodel.cn/cn/coding-plan/quick-start` | the plan's own base URLs: Anthropic `https://open.bigmodel.cn/api/anthropic`, OpenAI chat `https://open.bigmodel.cn/api/coding/paas/v4`, OpenAI responses `https://open.bigmodel.cn/api/v1`; "GLM Coding Plan 仅限在官方支持的指定工具与产品环境中使用" |
| CN | `https://platform.kimi.com/docs/pricing/chat` | kimi-k3: unit 1M tokens, cache write ¥20.00 (TTL 5 min) / ¥40.00 (TTL 1 h), cached input ¥2.00, input ¥20.00, output ¥100.00, context 1,048,576; kimi-k2.7-code ¥1.30 / ¥6.50 / ¥27.00, 262,144; kimi-k2.7-code-highspeed ¥2.60 / ¥13.00 / ¥54.00, 262,144; kimi-k2.6 ¥1.10 / ¥6.50 / ¥27.00 |
| CN | `https://www.kimi.com/code/docs/` (+ `/kimi-code/membership.html`, `/kimi-code/models.html`) | Kimi Code base URLs (CN: `https://api.kimi.com/coding/v1`, `https://api.kimi.com/coding/`); its four model ids; the subscription is a membership benefit with a 5-hour rolling window and a monthly total, **no token allowance published**; third-party tool integration is documented |
| CN | `https://platform.kimi.com/docs/guide/product-plans.md` | "Kimi API 开放平台是按量计费模式、无订阅制方案，与 Kimi 会员、Kimi Code 等产品不同" and "Kimi Code API 与本平台提供的 API 服务相互独立" |
| intl | `https://docs.z.ai/guides/overview/pricing` (already cited by the shipped entries) | the international GLM prices the roster already carries (USD) |
| intl | `https://platform.kimi.ai/docs/pricing/chat` | kimi-k3: cache write **$3.00 (TTL 5 min) / $6.00 (TTL 1 h)**, cached input $0.30, input $3.00, output $15.00; kimi-k2.7-code $0.19 / $0.95 / $4.00; kimi-k2.7-code-highspeed $0.38 / $1.90 / $8.00; kimi-k2.6 $0.16 / $0.95 / $4.00 |

## Decision

### 1. `currency` is a provider-entry property, and no rate is ever applied

`providers[i].currency`: **`USD` | `CNY`** (ISO-4217, uppercase), **absent ⇒ `USD`**. It is the unit of every
`price` tier in that entry's model table, and of nothing else.

- **Why the entry, not the model and not the tier.** The four tiers of one model are one invoice line from one
  account, so they cannot disagree with each other: a per-tier unit would be a field whose only legal value is
  the one its siblings have. And the price list belongs to the **account** — the same argument that already put
  `account`, `base_url`, `wire_api` and `api_key_env` on the provider entry (spec §4.6): the same model id
  (`kimi-k3`) is billed in CNY through `api.kimi.com` and in USD through `api.kimi.ai`, and only the entry knows
  which one it is.
- **No conversion, anywhere.** vadis applies no exchange rate, stores no rate, and never converts one
  currency's figure into another's. A CNY page is transcribed as CNY; a request served by a CNY entry is
  accounted and reported in CNY. An estimated rate is a number nobody published — the same defect class as an
  estimated price (AGENTS constraint 5).
- **`currency` is not derived from `region`.** A CN-region entry billed in USD is a legitimate configuration,
  and deriving one field from the other would manufacture a fact the pages do not state. Two defaults
  (`region: intl`, `currency: USD`) are written, and neither implies the other.
- **The source rule becomes regional.** §4.0's `source` requirement is now read per region: a `cn` entry cites
  the CN official page, an `intl` entry the international page. One region's table is never the evidence for
  another region's price.

### 2. Money carries its currency through the whole path, and mixed arithmetic does not compile

- `Currency { Usd, Cny }` (serialized `"USD"` / `"CNY"`) and `Money { nano: Nano, currency: Currency }`.
- The raw fixed-point amount keeps its **scale** (1e-9 of the major unit) and loses its **name**: `NanoUsd(u64)`
  is renamed **`Nano(u64)`**. ADR-006's representation decision (integers, one rounding point, saturating
  overflow, no `f64` on the money path) stands unchanged; only item 1's "1 NanoUsd = 1e-9 **USD**" becomes
  "1 nano = 1e-9 of the currency carried beside it". A type whose name asserts a unit it no longer holds is
  exactly the small lie this repository writes ADRs to avoid.
- `PriceTable` and `CostBreakdown` gain `currency: Currency`, copied from the entry at load time: the pure
  cost function stays integer-arithmetic and its output is self-describing.
- **There is no `impl Add for Money` across currencies and no `Sum` impl at all**: aggregation is spelled as a
  per-currency map (`BTreeMap<Currency, Money>`), and the only `Add` that exists is `Money::checked_add`, which
  returns the mismatch as an error rather than a value. A future code path that wants to add two currencies has
  to name the currencies it is adding — it cannot do it by accident.
- **The one structural scalar is kept single-currency by a load error.** `plan_policy.overflow_monthly_cap_usd`
  is denominated in USD by its name, and it is compared against the family's metered spend. A policy that sets
  it while the `overflow` route's provider is of another currency is a **load error** naming
  `plan_policy.overflow_monthly_cap_usd`. Comparing a USD ceiling with a CNY spend is precisely the silent
  mixing this ADR forbids, and the honest alternatives (re-denominating the cap by a rate, or comparing them
  anyway) are both worse than refusing to start.
- **The store's two money-bearing payloads carry the unit too.** `cost.computed` (a set of `*_nano` integers)
  and `plan.switched` (`switch_cost_nano`) are the same class of record as the trace's cost group, and the
  store is read on its own — the `OverflowSpend` projection sums `total_nano` over one route. Both gain a
  `"currency"` field, and `EVENT_SCHEMA_VERSION` moves 1 → 2 with `TRACE_SCHEMA_VERSION` (DESIGN §12.10.5):
  leaving a second, older place where an amount has no unit would reintroduce exactly the ambiguity item 1
  removes. The projection stays single-currency by construction — it is route-scoped, and its one comparison
  is the USD cap of item 2 (DESIGN §12.10.8).

### 3. `region` is a provider-entry field, not a name suffix

`providers[i].region`: **`cn` | `intl`**, **absent ⇒ `intl`**. It declares which regional deployment of the
vendor the entry's endpoint and key belong to. It is **not** a routing dimension (a client writes
`provider/model`, and the deployment behind the name is invisible to the request) and **not** an accounting
dimension (that is `currency`). It is surfaced: `/health`'s provider list carries each entry's `region` and
`currency` beside its name, key variable and availability, so "which deployment am I actually spending on" is
answerable from a reporting surface rather than from a comment.

The vadis does **not** check region against the host in `base_url`: vendors own their host lists, a built-in
table of them would rot between releases, and a wrong-yet-declared region is a documentation error, not a
routing one. The check that matters is the `source` rule (item 1), which a reviewer reads on the entry itself.

**The naming convention is documentation, not a load rule, and `moonshot` → `kimi` is a one-time correction
made now.** The names are operator-chosen: vadis validates a name for uniqueness, not for shape, because a
naming rule that is not checked is a rule that lies. The convention above exists so that a name cannot
contradict the key variable beside it, which is exactly the state R6-3 left the shipped example in (its
entries are named `moonshot` / `moonshot-plan` while their `api_key_env` reads `KIMI_API_KEY` /
`KIMI_CODING_API_KEY`). The correction is cheap **only now**, before v0.1 ships: a provider name is
client-facing — it is part of every route string a client writes, of every alias, of `fallback` entries and
of `quota.models` references — and it is what already-written traces record in `decision.provider`. Once
configs and traces exist in the wild, renaming a provider is a breaking change to both, which is the reason
`region` is a field rather than a name suffix: the facet that had to be added was added additively, and no
future facet needs to repeat this correction.

**Rejected: region as part of the provider name (`zai-cn`).** The provider name is the client-facing route
grammar and the trace's `decision.provider`; encoding a facet into it would invalidate every existing route
string, alias, `fallback` entry and `quota.models` reference, and would change what already-written traces
mean (a rename is not additive for the observation record). It also forces the reader to *parse* a name to
recover a fact the file can state, and a naming convention is unenforceable where a typed field is validated.
The exchange is that names stay operator-chosen and the convention below is documentation, not a check.

**The naming convention (documentation, not a load rule):** `<vendor>[-<region>][-<account>]`, where
`<vendor>` is the product name the key variable uses (`DEEPSEEK_*` → `deepseek`, `ZAI_*` → `zai`, `KIMI_*` →
`kimi` — R6-3 fixed the key names to the product names, and a provider name that disagrees with its own
`api_key_env` is the mismatch an operator sees on the first `/health` read), `<region>` is omitted for `intl`
and `-cn` for `cn`, and `<account>` is omitted for `account: api` and `-plan` for `account: coding_plan`. The
shipped example therefore reads `deepseek`, `zai`, `zai-plan`, `kimi`, `kimi-plan`, `zai-cn`, `zai-cn-plan`,
`kimi-cn`, `kimi-cn-plan` (see "the example" below).

### 4. The family is a **model-entry tag**: `models[].family`

`models[i].family`: optional non-empty string; **absent ⇒ the model's own `id`** — which is exactly ADR-014's
current rule, so every existing config and every existing conformance case behaves identically.
`plan_policy.family` (§4.6) matches this tag.

- **Model level, not route level.** The statement being made is about *models* ("this entry's model is the
  same family as that one"), not about routes, and it belongs next to the native id it maps, where the pair is
  readable in one place. A route-level map (`family → [routes]`) would be a second top-level section
  duplicating the roster — two places to keep in sync, with its own illegal-combination table.
- **It is a name, not an address.** A client never writes a tag: it writes `provider/model` or an alias (§3),
  and a bare tag resolves to nothing (`404 unknown_model`). The tag never appears on the wire and never reaches
  a provider.
- **§2's two mutations are unchanged.** The outbound `model` is still the resolved route's own native id
  (`k3-256k` for one route, `kimi-k2.7-code` for the other), and `decision.requested_model` is still the
  client's own string verbatim. The tag moves neither: it exists so that a policy can name a pair the ids
  cannot.
- **The equivalence is asserted, not verified.** Nothing checks that two routes' models really are the same
  model — the vadis cannot know, and the operator can. It is deliberately **explicit**: no rule ever infers a
  family from ids that look alike (prefix, suffix, substring, case), because an inferred equivalence would be a
  claim about a provider's catalogue that no page makes.
- **Load rules:** a tag resolves to at most one model entry per provider entry (a duplicate is a load error
  naming `providers[i].models[j].family`); an empty tag is a load error; `primary` and `overflow` must each
  resolve to a model entry carrying `plan_policy.family`; the family's coverage rule inside the primary
  provider's `quota` is restated in terms of **the model id the primary route resolves to** (it is
  `quota.models` that is being satisfied, and `quota.models` names ids).
- A tagged entry that no policy names is **legal and inert** — the same standing as a plan account no policy
  routes today.

### 5. The trace says what its money is denominated in

- `cost.currency` joins the cost group (§6): the currency of the route whose price table priced the record.
  One request is priced by one table, so a record has one currency, and `cost.currency` is by construction the
  entry `decision.provider` names.
- `result.plan_switch.cost_currency` denominates `switch_cost_nano`, which is priced by the **destination**
  route's table and therefore keeps the destination's denomination even in the rare case where that attempt
  fails and the chain serves a route of another currency. It is present whenever `plan_switch` is not null.
- **`DecisionRecord.schema_version` moves 1 → 2.** DESIGN §12.6's exemption is for *additive optional* fields
  whose absence has a meaning (`requested_model: null`, `plan_switch: null`); this is not that case: the new
  field changes how an existing field is **read**, and a consumer that ignores it will sum CNY into USD. The
  version is the only signal a consumer gets before it does. And the older vintage stays unambiguous, because a
  v1 record is **USD by definition**: no non-USD route was configurable when it was written. A window may
  therefore hold both vintages and be read correctly.

### 6. The reporting surfaces report money per currency — and a mixed window has no total

`vadis stats` prints every **money** line once per currency present in the window, each labelled, and prints a
line naming the currencies it saw. **Counts** (`requests`, `switches`, `switches without usage`, `usage
missing`, `unknown outcome requests`) and **ratios** (`hit rate`, `continuity p50`) are currency-free and keep
aggregating across the whole window. A mixed-currency window is **not an error**: a report is produced
(exit 0), and the one thing it does not produce is a combined money total.

`--json`: when exactly one currency is present, today's scalar keys are kept and a `"currency"` string is
added; when several are present, the scalar keys are **absent** and the figures appear under a per-currency
map. A consumer that assumes one total therefore fails loudly instead of summing silently.

### 7. The CN plans carry no `quota` in v0.1, and the reason is registered

Neither CN plan publishes a token allowance (`GLM Coding Plan`: credits on a 5-hour + weekly window; `Kimi
Code`: usage windows, monthly total). spec §4.6 already says a plan whose allowance is not published is still
a plan, and `quota` is optional — so the CN plan entries carry **no** `quota`, and the published allowance,
the coefficients and the window facts live in the entry's comment with their source URL and read date.
Registering a token number would be fabrication; extending `quota` to credits and multiple windows is a
different change with its own blast radius (registered as **GAP-Q17**, DESIGN §12.9).

## Rejected alternatives

| Alternative | Why not |
|---|---|
| Fold the CNY table into USD with an exchange rate (one rate, one place, "good enough") | Forbidden twice over: the rate is a number nobody published (AGENTS constraint 5), and the ledger's single-unit property (ADR-006) is what makes replay bit-identical — a rate makes the report depend on when it was fetched (AGENTS constraint 2, content determinism) |
| Keep CNY figures in a field named `usd` (or leave the unit implicit and document it) | The defect this ADR exists to remove: a reader (human or the analysis loop) cannot tell a CNY figure from a USD one, and nothing fails when they are added |
| `currency` on the model entry, or on each tier | See item 1: an entry's models are one invoice line, and a per-tier unit has no legal disagreement |
| Derive `currency` from `region` | Manufactures a fact from a nearby one; a CN endpoint billed in USD is legal |
| Keep the name `NanoUsd` and carry the currency out of band (a struct field only at the aggregation boundary) | The type would assert USD while holding CNY, and every intermediate would need an unwritten invariant to be safe |
| Forbid a family whose two routes are of different currencies | Kills the pairing that motivates the whole change (a CN plan with an international metered spill); the ambiguity it is meant to prevent does not exist, because money is never added across currencies and each figure states its own |
| Re-denominate `overflow_monthly_cap_usd` into the overflow route's currency | Its name would have to lie or change (a rename breaks every config that writes it); a load error keeps the name honest and the comparison single-currency |
| `region` as a name suffix (`zai-cn`) | See item 3: not additive for the route grammar or for already-written traces |
| A top-level `families:` section (family → routes), or extending `aliases:` to carry a family | Aliases are client-facing (they appear in `decision.selection_source`); a family is a routing-account concept the client must not be able to name. A separate section duplicates the roster and needs its own combination table. The model entry can state it where the id it maps lives |
| Infer a family from similar ids (normalise `k3` ↔ `kimi-k3`, strip vendor prefixes) | The vadis would assert an equivalence about a provider's catalogue that no page states — the fabricated-fact class again; and it would silently change behaviour when a vendor adds a rename |
| Extend `quota` to credits + multiple windows to express the CN plans | Real, but a different change: the credit model has a coefficient table, two window kinds and a peak/off-peak multiplier, and none of it is needed for the ledger to be correct (the local counter is a warning, spec §4.6 rule 3). Registered as GAP-Q17 rather than smuggled in |
| Check `region` against a built-in table of vendor hosts | The table rots, the vendors own it, and the failure it prevents (a commented region that is wrong) is a documentation error |

## Consequences

- `docs/spec.md`: §4's schema gains `region`, `currency` and `models[].family`; §4.0 gains the currency and
  cache-write-tier conventions and the regional `source` rule; **§4.8 is new** (the three keys and the
  "one amount, one currency" invariant); §4.6's `family` / `primary` / `overflow` / `overflow_monthly_cap_usd`
  rows and three rows of its illegal-combination table are updated; §6's cost group gains `cost.currency` and
  `plan_switch.cost_currency`; §9.2's report, provenance table and conventions gain the per-currency rule.
- `design/DESIGN.md`: §12.4 (the currency-tagged money types, the `NanoUsd` → `Nano` rename, the no-mixed-add
  rule), §12.5 (`ProviderCfg.region` / `.currency`, `ModelCfg.family`, three parsing rules), §12.6
  (`cost.currency`, `PlanSwitchRec.cost_currency`, `TRACE_SCHEMA_VERSION` 1 → 2 and why this is not the
  optional-field exemption), §12.8 (**CONF-46, CONF-47, CONF-48, CONF-49** — allocated here, files land with the
  implementing change), §12.9 (**GAP-Q17**), §12.10.2 (the load-validation rows, the `/health` provider list),
  §12.10.5 (the `cost.computed` / `plan.switched` payloads gain the unit; `EVENT_SCHEMA_VERSION` moves 1 → 2 with
  the trace version), §12.10.8 (the `OverflowSpend` projection's sum is route-scoped, hence single-currency).
- **The rename is mechanical but wide**: `NanoUsd` appears in `vadis-core` (`cost.rs`, `breakeven.rs`,
  `plan.rs`, `quota.rs`, `store.rs`, `trace.rs`, `peak.rs`), `vadis-store` (`lib.rs`, `trace_sink.rs`) and
  `vadis-cli` (`stats.rs`), plus the conformance helpers that build fixtures. No behaviour changes with it;
  the currency fields are the behaviour.
- **`config.example.yaml`** (the coder card's write-set, not this ADR's): the region comments become fields,
  the CN entries land with their CNY tables transcribed from the pages above (each with URL + read date, the
  official /1M value quoted at the end of the line as the existing convention does), the `moonshot*` names
  follow the naming convention, and the Kimi pair carries the family tags. `plan_policy` itself does **not**
  move: a docs-first round must not silently re-point the shipped example's economic policy.
- **CONF-46…49** (DESIGN §12.8): 46 — a CNY-priced route's record and report carry CNY, and a mixed window is
  reported per currency with no combined total; 47 — the family tag pairs two different native ids, the
  outbound `model` stays each route's own native id and `requested_model` the client's verbatim (F3 unchanged),
  and a client cannot address a tag; 48 — `region` is a declared field with a display consequence and no
  routing consequence; 49 — the USD cap on a non-USD overflow route is a load error naming the key.
- **GAP-Q17**: the CN plans' allowances are credits / usage windows, not token counts; v0.1's `quota` cannot
  express them, so the plan entries carry none and the local counter is simply absent for them (the upstream
  remains the only authority, which is already rule 3).
- **What this ADR does not do**: no cross-currency summary, ever; no rate plumbing; no credit accounting; no
  change to §2's byte boundary, to the routing grammar, to `aliases`, to `fallback`, or to any error code.
- **Effect on the existing conformance cases.** No existing case file is edited by this round, and no existing
  case changes what it asserts: **CONF-40** (an in-plan request is 0 in every bucket, an overflow request is
  priced at the model's real five-tier price) is unchanged — the only difference is that the record now states
  *which currency* those zeros and prices are in, and for every USD entry in the shipped example that field is
  `"USD"`; the `cost_*` field assertions gain one field and move no value. The four new cases (CONF-46…49) are
  the ones that assert the new behaviour, and they land with the implementing change (DESIGN §12.8).

## Honest boundaries and verification owed

- **The tag is an unverified assertion** (item 4). Two routes tagged together may in fact be different models;
  the vadis honors the operator's statement, and the failure mode is a team that mis-pairs two ids and gets a
  policy routing between non-equivalent models. It is not detectable at this layer, and it is written down
  rather than guessed at.
- **`region` is declared, not observed.** Nothing in the vadis confirms that a `cn` entry's endpoint really is
  the mainland deployment of that vendor; the field is as honest as the operator who wrote it.
- **The CN plans' exhaustion signal is unobserved.** Plan-first routing moves an account on an upstream `403`
  classified `quota_exhausted` (ADR-011 / spec §4.6 rule 3). Neither CN plan page states which status its
  coding endpoint returns when a membership window is exhausted, and no response has been observed here (no
  key in this environment). Until a real `403` (or whatever it is) has been seen, a CN plan family's spill is
  designed-but-unwitnessed: if the endpoint answers `429` instead, ADR-011's classification — not this ADR — is
  what needs revisiting. This is the one item a live probe closes, and it is the first thing to check before an
  operator trusts a CN plan as a family's `primary`.
- **A CN coding plan may not be legal to use through a router at all.** `GLM Coding Plan`'s own documentation
  says the allowance is only usable "在官方支持的指定工具与产品环境中" and that calling the standard API from a
  self-built application, site, bot or SaaS product does not consume the plan's allowance (`docs.bigmodel.cn/cn/coding-plan/overview`
  and `/glm-coding`, read 2026-09-21). A gateway is a self-built application, so an operator pointing a CN GLM
  plan's key at vadis must check that vendor's terms first — the capability described here is not a licence.
  Kimi's documentation is the opposite case: it documents handing the subscription's API key to third-party
  tools (`www.kimi.com/code/docs/`), which is what a gateway is. Both facts are recorded because they are
  facts; neither is a recommendation.
- **A finding in the shipped example, not fixed here.** `config.example.yaml`'s `moonshot/kimi-k3` entry
  records `cache_write: 0.0` with the note "official page lists no cache-write billing item". The
  international page read for this ADR (`platform.kimi.ai/docs/pricing/chat`) **does** price cache writes per
  TTL tier ($3.00 / 1M at the 5-minute default, $6.00 at 1 hour) for the K3 series. Either the page changed or
  the note was written against a different section; the entry needs a re-read by the owner and a corrected
  figure (or a corrected note). The CN entry inherits the same shape (¥20.00 / 1M at 5 minutes). `config.example.yaml`
  is outside this ADR's write-set, so this is reported rather than edited.
- **The CN GLM `context` is not settled by this ADR either.** The CN pricing page states a 1M context for
  GLM-5.3; the shipped international entry carries `context: 200k` with an unresolved TODO. Two regions may
  legitimately differ, so one region's page is not used to correct the other here.

## Reversibility

| Decision | Reversible? |
|---|---|
| `currency` (absent ⇒ USD), `region` (absent ⇒ intl), `models[].family` (absent ⇒ the id) | **Yes.** All three are additive with the old behaviour as their default: deleting them restores today's configs and semantics exactly |
| The money type's currency tag and the `NanoUsd` → `Nano` rename | **Yes, mechanically** (a rename plus container fields), but only before a second currency exists in the wild; after that, going back means dropping the distinction the reports now make |
| Per-currency reporting and the absent scalar in a mixed window | **Yes** — the reporting surface, not the ledger |
| `DecisionRecord.schema_version` 1 → 2 and the two currency fields | **No.** Records are append-only and never rewritten (ADR-005/ADR-010); the field can be ignored by a future reader, but a record written without it cannot be recovered, and a v2 record must keep saying what its money is |
| `EVENT_SCHEMA_VERSION` 1 → 2 and the two payload fields | **No**, for the same reason (the event log is append-only); the change is one field per payload, and a v1 row stays readable as USD |
| The load error on a USD cap over a non-USD overflow route | **Yes** — it is a check, not a data shape |

## Publication note (2026-10-03, R62-2)

The `R<n>` labels and finding ids in this document name iterations of the project's own
private analysis loop — a loop that is not part of this repository, so no label here is
resolvable by a reader of it; they are kept as the provenance of the decision. This
publication pass removed only the dead-pointer class: every reference into that loop's
working tree (its file paths and round-record names, its state record, charter, execution
model and replay contract, its scripts and module names, and the kanban card ids), each
replaced by the neutral phrase the sentence needs. Nothing else moved — no figure,
threshold, `§`/`ADR`/`CONF` id, code sample or contract sentence.
