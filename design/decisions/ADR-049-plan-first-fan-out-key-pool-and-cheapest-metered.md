# ADR-049 — plan-first fan-out: the credential pool, the family list, and the cheapest metered account

- Status: accepted
- Date: 2026-10-04 (round **R66**'s contract card, `R66-0`)
- Kind: **a new capability, treated as a product highlight — and docs-only in this card.** It adds three
  additive config surfaces and one inspectable surface; it changes no product byte *here*. `R66-1`
  implements it, `R66-0b` audits this contract before `R66-1` is cut. It adds seven conformance cases
  (`CONF-91…97`), each enumerated in §9, each parked `#[ignore]`d with its own dependency until the
  implementing card lands it. It adds no gate, moves no corpus, changes no threshold, mints no
  saving/latency/cost figure, and touches no price.
- Authority: **the owner's directions of 2026-10-04**, quoted verbatim in §1.1. The allocation of
  `CONF-91…97` is that act, not a loop outcome (AGENTS constraint 9 / ADR-012).
- Related: **ADR-014** (plan-first routing — whose *session granularity* (item 1), *probe* (item 3) and
  *rule order* (item 9) this ADR preserves untouched, and whose Background constraint 1 — *a route change
  is a cache event* — is the reason §6 below exists); **ADR-011** item 7 (the credential-rotation **slot**
  — *"v0.1 has exactly one `api_key_env` per provider, so rotation is a slot whose implementation is a key
  list — an additive config key with its own spec change"*; this is that key list); **ADR-022/023** (the
  candidate walk's per-wire eligibility and its per-condition refusal); **ADR-037** (the roster as its own
  file — whose *exactly one of two keys* refusal ladder §4.14 is the precedent §4.5 below follows);
  **ADR-041** (the metrics surface this ADR extends with a per-family label set); **AGENTS.md** constraints
  **1** (the byte boundary), **2** (content determinism), **3** (the observation boundary), **4**
  (verified vs inferred), **5** (no fabricated prices), **8** (docs before code), **9** (the measurement is
  not part of the search space); spec §2, §3 (the reserved `Selector` slot), §4, §4.0, §4.2, §4.6, §4.8,
  §4.10, §6, §7, §8, §9.1, §9.2; DESIGN §12.3 (`Selector`), §12.5, §12.8, §12.10.5, §12.10.8, §12.10.9.
- Cases: `tests/conformance/tests/conf_91_credential_pool_rotates_before_fallover.rs`,
  `conf_92_key_pool_presence_and_the_secret_boundary.rs`,
  `conf_93_plan_policies_list_and_the_family_key.rs`,
  `conf_94_cheapest_metered_candidate_is_the_walk_order.rs`,
  `conf_95_ranking_total_order_and_currency_refusal.rs`,
  `conf_96_ranking_pinned_per_session.rs`,
  `conf_97_plan_tier_drained_before_metered.rs`. No existing ID is spent, reused or renumbered.
- **Line-number convention.** Every `path:line` below resolves at **this branch's HEAD** — the commit that
  carries this ADR. `R66-1`'s edits shift the numbers in the files it touches; the deltas are stated where
  they matter (§10) rather than left to be discovered.

---

## 1. The authority, and what "a highlight" changes

### 1.1 The owner's directions, verbatim and dated

**The owner's directions of 2026-10-04**, verbatim, in the order they were given:

> *"1. 将配置的 api key 全部用上 2. 优先把 coding plan 的 quota 用满，用满后，再按照配置 (价格/效果/综合等)
> 选择基于请求计费的 api key，使用其 (provider, model) 3. 能够自动监测 coding plan 的 quota 恢复了，恢复后自动从
> 请求计费的 api key 切换回可用的 coding plan"*

> *"「把配置的 key 全用上」具体指哪种？"* → *「**两者都要**」* (both: a per-provider key pool **and**
> several metered providers as rankable candidates)

> *"metered 候选的排名口径以哪个为准？"* → *「**只按价格：用配置里已有的 price 表排序（源已齐备，constraint 5
> 满足）**」* (price only — no invented quality axis)

> *"plan quota 恢复的「监测」要主动后台探测吗？"* → *「**保持现状：只在会话边界的首个请求上探一次（不改 ADR-014，
> 零额外成本）**」* (no background polling; ADR-014 item 2 stands unchanged)

> *"1. 配置面：按照你的建议进行"* (the config surface as proposed: a ranking **mode** on the policy, the
> candidate set discovered from the family tag — **not** an explicit `overflow:` list)

> *"2. 放开为列表"* (open `plan_policy` to a list — several families)

> *"这个能力是要作为亮点对待的"* (**treat this capability as a highlight**)

> *"把这个作为亮点，主要是考虑到通常会有多个 api key，且这些 api key 可能是不同的 provider 的 coding plan 或基于请求
> 计费，通常前者会比后者便宜。但 coding plan 会有 quota 限制，若超出 quota，可能要等待，会中断工作。为了有效使用这些 api
> key，可以通过 vadis 自动将请求转发到有效的 api key 上，且按照一定的策略进行转发，如优先用满 coding plan，再按照策略选择
> 某个基于请求计费的 api key。在 coding plan quota 恢复后，自动再从基于请求计费的 api key 切换回 coding plan。"*

> — **the sentence that makes §5.1's plan tier non-optional.** Several keys, each *either* a coding plan *or*
> a pay-per-request account, **often from different providers**; a plan is cheaper, but a plan's quota, once
> exceeded, means **waiting — which interrupts the work**. The gateway must therefore hold **several plan
> accounts**, drain them, and only then spend. An earlier draft of this card had a single-`primary` plan
> tier; the owner's clarification is what turned it into a tier, and the ADR records that provenance rather
> than presenting the tier as its own idea.

> *"多层 plan 的抽干顺序按什么定？"* → *「**按 roster 声明顺序（由我在配置里排先后，无新键）**」* — the plan
> tier is ordered by the roster's own declaration order, `primary` naming its head; **no new key orders it**.

> *"plan 层多重化（含 plan_state 状态机升级）放哪里？"* → *「**折进 ADR-049，同轮完成（§4.6 尚未实现，此时改冻结最
> 便宜）**」* — the tier and the generalized state machine belong to **this** ADR, not a successor: §4.6 is
> un-implemented, so naming its state model now is cheaper than migrating it later.

### 1.2 What "a highlight" means here, stated as two obligations and one refusal

A highlight is not a licence to overstate; in this repository it is the opposite. The word buys exactly
two things and forbids one:

1. **It is named and it is the product's own story.** The capability gets a name — *plan-first fan-out* —
   a lead section in `book/` (not a footnote in `operations.md`), and the spec's §4.6 chapter is extended
   rather than patched. A highlight a user cannot find is not one.
2. **It is inspectable at the moment it acts.** The owner's standing preference is *black-box the
   machinery, front-stage the verifiable audit evidence*. So the fan-out's two decisions — *which key* and
   *which metered account, in what order* — must be readable from a running process (§7), not only
   inferable from a log after the fact. This is the part that makes it a highlight rather than a config key.
3. **Refused: any quality/efficiency claim.** The owner chose *price only*. So this ADR introduces **no**
   "效果"/quality field, no score that is not a price, and no `verified` number it did not measure
   (constraint 4). The ranking's input is the roster's existing, sourced `price` table and nothing else
   (constraint 5, already satisfied). A highlight that lied about quality would be the exact failure
   constraint 4 exists to prevent.

---

## 2. The capability in one picture

```
request → family guard (§4.6, unchanged)
   │
   ├─ the PLAN TIER, drained in declaration order (§5.1)   ← several plan accounts, several providers
   │     plan A ──403 quota_exhausted──▶ plan B ──403──▶ …   (stays in-plan; reason `plan_exhausted`)
   │     │  401/429 → the next KEY of the same provider (§3); each key its own cooldown
   │     │
   │     └── every plan member exhausted ─────────────────────┐
   │                                                          ▼
   └─ the METERED TIER, ranked by price (§5.6)   ← the pay-per-request accounts
         cheapest ──retryable failure──▶ second-cheapest ──▶ … ──▶ global `fallback`
         │
         └── session boundary probe (§4.6 item 2, unchanged) ──▶ back to the PLAN TIER's head
```

**In one sentence:** `vadis` keeps your work running on the cheapest account that still has room — it drains
every coding plan you own before it spends anything, moves on when one plan's quota runs out, and returns to
the plan the moment one is usable again.

Three additions, all additive, all reachable without reshaping a frozen section:

| # | Surface | Reads | Lands |
|---|---|---|---|
| A | `api_keys:` on a provider entry | a **pool** of credential env names, in declaration order | §3 |
| B | `plan_policies:` (list) beside `plan_policy:` (one) | **several families**, one policy each | §4 |
| C | the **two tiers** — the plan tier (declaration order), then the metered tier (price order) | the **drain-then-spend** walk | §5 |

and one **generalized state machine** (§5.2), because with two tiers the family's state is no longer
`primary`/`overflow` but **the route it is currently on**.

---

## 3. A — the credential pool

**The slot this fills.** ADR-011 item 7 already reserved it: *"rotation is a **slot** whose implementation
is a key list — an additive config key with its own spec change"* (ADR-011:187). This ADR is that spec
change. Nothing about the classification changes: the classifier already produces
`RecoveryAction::RotateCredential`, and v0.1 ships it as a no-op that falls through to `FallbackProvider`
(ADR-011:188-190). R66-1 removes the no-op's "falls through" half.

**The key.** On a provider entry, beside `api_key_env` (`crates/vadis-core/src/config.rs:896`):

```yaml
  - name: zai
    api_key_env: ZAI_API_KEY          # the one-key spelling — unchanged, and still enough
    # — or —
    api_keys: [ZAI_API_KEY_A, ZAI_API_KEY_B, ZAI_API_KEY_C]
```

**Exactly one of the two is written**; both written, or neither, is a load error naming **both** keys. This
is §4.14's own refusal ladder (ADR-037 D1/D3/D4) applied to a second pair — the precedent is deliberate,
because a config file with two spellings of one fact and no rule between them is the ambiguity §4.14
already ruled against. `api_key_env` keeps working exactly as today, so **no existing config changes and
no existing case moves**.

**Semantics (normative).**

1. **Declaration order is the rotation order.** The pool is the *present* subset of the names, in the order
   written. An absent name is not a startup failure — it is one fewer credential (the `§12.10.2` stance,
   unchanged).
2. **Availability is pool-wide.** A provider is available iff **at least one** pool key is present; today's
   `present` boolean becomes per-key facts, and the provider-level fact is their disjunction (§7).
3. **Rotation is same-provider, and only on a credential-class failure.** `401`/`403`-auth and `429` — the
   classes ADR-011 item 7 assigns to the credential — advance to the **next present key** of the **same**
   provider. Only when the pool is exhausted does the walk leave the provider (`FallbackProvider`,
   ADR-011 item 7's *"a 401 rotates first and then falls over"*).
4. **`403 quota_exhausted` never rotates.** ADR-011 already treats it as a fact about the **account**
   (ADR-011:181-182, ADR-014's Background); a coding plan with three keys is still one exhausted plan. It
   moves the family (§4.6 rule 3), not the key. This is the single most important negative in the ADR, and
   `CONF-91`'s negative arm pins it.
5. **A key holds its own cooldown.** Per-credential exhaustion/demotion with a TTL chosen by the causing
   status, and a terminal-auth state that does not re-enter rotation — ADR-011 item 7's own shape. It is a
   **projection** (ADR-010), keyed `(provider, key_index)`, rebuildable from the event log; no in-memory
   credential state may exist on the serving path (ADR-009/010).
6. **One attempt per key per request.** `attempt_index` (`crates/vadis-proxy/src/forward.rs:1115`) counts
   attempts; an attempt is now a `(route, key_index)` pair, and a request never re-attempts a key it has
   already tried.
7. **The key is a header, not a byte.** The pool touches no body byte: constraint 1 is unaffected and the
   conformance prefix hash is unmoved. The **value** is read once at the revision's build
   (`crates/vadis-cli/src/lib.rs:207-222`) and never printed — the existing secret boundary, extended to N
   names. Where the trace must name the credential it names the **index**, never the value (`CONF-92`,
   `CONF-70`'s canary shape).

**A key switch is a cache event, and is priced as one.** Two keys of one vendor may or may not share an
upstream cache namespace; the vadis cannot know, and must not assume. So a within-provider key switch is
narrated and priced exactly like the account move it resembles: `error.classified.action =
"rotate_credential"` with the key index, and the displacement priced under ADR-011 item 9's convention
(inferred at decision time, verified once usage lands) when a re-prefill is incurred.

---

## 4. B — the family list

**Today.** `plan_policy` is one optional mapping and *"at most one policy in v0.1; a second family is an
additive future key (the `state:` / `retention` precedent), never a reshaped section"* (spec §4.6,
`docs/spec.md:587`).

**This ADR is that additive key.** `plan_policies:` is a **list** of the same policy objects; both
`plan_policy` (one family, today's mapping) and `plan_policies` (N families) written is a load error
naming both keys — §4.14's ladder again, one layer up. **Neither written is not an error**: it is the
no-plan configuration §4.6 itself freezes ("at most one policy in v0.1"), and it loads unchanged
(rule 4 below depends on exactly that — a plan account named by no policy is a legal, ordinary entry).

```yaml
plan_policies:
  - family: glm-5.3
    primary: zai-plan/glm-5.3
    overflow: zai/glm-5.3
    overflow_selection: cheapest
  - family: kimi-k3
    primary: kimi-plan/k3
    overflow: kimi/kimi-k3
    recover: probe
```

**Normative.**

1. **A family tag appears at most twice as a policy key — once per list entry — and the list is refused at
   load if two entries name one tag** (the state is keyed by the family tag; two policies on one tag would
   be two writers of one state row). Message names `plan_policies[i].family`.
2. **Every §4.6 rule is per family** and unchanged: probe admission, cooldown, `on_primary_exhausted`,
   `overflow_monthly_cap_usd`, the `plan_state` key, the `plan.switched` payload's `family`. The
   projection needs **no change** — it was keyed by family from the start (ADR-014 item 8).
3. **Families are independent.** A spill on family A leaves family B on its primary; a probe on A never
   moves B. `CONF-93` pins this (and pins that it is not merely the family *label* that is independent).
4. **`account` remains a provider-entry property.** Nothing here changes what an entry *is*; a plan account
   named by no policy is still the legal, ordinary entry §4.6 already allows.
5. **Reload.** A family added, removed or re-parameterized by a reload follows the existing revision rules
   (ADR-040): the new revision's families serve the next request; the store's `plan_state` rows are keyed by
   tag and survive the switch, and a family the new revision does not declare is simply not routed
   specially — its rows stay as history.

**Declined: `plan_policy` accepting both a mapping and a sequence (an untagged enum).** It is additive too
and needs no second key. It is declined because this repository's refusals *name the offending key*, and an
untagged enum degrades every one of §4.6's twelve load refusals into a shapeless "did not match any
variant" (ADR-025's lesson: a config file is a record, and a record's error must name the record). The
two-key ladder costs one more key and keeps every message binding.

---

## 5. C — the two tiers: drain every plan, then rank the metered accounts

(The owner's clarification of 2026-10-04 — §1.1's quotation about **several keys, several providers** — makes
this section the highlight's **core**: there are usually **several** API keys, each the key of *either* a
coding plan *or* a pay-per-request account, and usually from **different providers**. A plan is cheaper than a
metered account, but a plan has a quota, and exceeding it means **waiting** — which interrupts the work the
gateway exists to keep running. So the gateway must hold **several plan accounts**, drain them, and only then
spend.)

### 5.1 The plan tier (the drained tier)

> **the family's plan candidate set** = every roster route whose provider is `account: coding_plan` and whose
> model entry carries the family tag.

The walk starts at the policy's **`primary`** and then visits the tier's remaining members in **roster
declaration order**. The rationale is forced, not chosen: inside a plan the marginal price is **0** (§4.6
rule 4), so every plan member is price-equal and *price cannot order them* — the only information left is the
operator's own preference, which is exactly what declaration order (plus an explicit `primary` for the head)
expresses. **No new key orders the tier** (the owner's ruling of 2026-10-04). A provider contributes at most
one route to the tier **by construction**: §4.8 already allows at most one model entry per tag per provider
(`crates/vadis-core/src/config.rs:787-789`), so "several plans" means several provider entries — each with
its own `api_key_env` or `api_keys` pool (§3) and its own subscription.

### 5.2 The state machine, generalized

Today the family's state is one boolean (`primary` | `overflow`) with two moves. With a tier the state records
**the active route**:

| | today | this ADR |
|---|---|---|
| state key | `family` | `family` (unchanged) |
| state value | `primary` \| `overflow` | the **active route** (`provider/model`) + `since_us`; the tier is *derived* from that route's provider's `account` |
| wire words | `account: primary \| overflow` | **kept unchanged**: `primary` ⇔ the active route is in the plan tier, `overflow` ⇔ it is in the metered tier. Every existing consumer (`/health`'s `account`, `probe.deadline`, and all eleven case files that read the member) is untouched |
| new | — | `/health`'s plan section also carries **`route`** — the specific route currently active — so "which plan am I on?" is inspectable (§7) |

**The four moves and their `reason` words.** Spec §6 decides a displacement's reason by **direction** (the
destination account); with two account tiers a same-tier move appears, so exactly one word is added:

| from → to | `reason` | whose rule |
|---|---|---|
| plan → plan | **`plan_exhausted`** *(added by this ADR)* | the plan route was exhausted; the family moves to the next plan member and **stays in-plan** |
| plan → metered | `primary_exhausted` | unchanged (spec §6's existing word) |
| metered → plan | `primary_recovered` | unchanged: the only way back is a plan that proved usable |
| followed the metered tier | `primary_cooling_down` | unchanged (ADR-011 route availability) |

**The probe** is generalized by exactly one clause: it is admitted when the active route is **not** in the
plan tier, and it attempts the **head of that tier** (`primary`). A successful probe moves the family to that
route and records `primary_recovered`; a failed probe leaves the family where it was and the walk continues
into the metered tier exactly as today — a probe is still **one** attempt on a free account and never a
per-request retry (ADR-014 item 3; §6 rule 1 of this ADR).

**And the rule that keeps a drained plan drained: the guard passes the *active* route, not a fixed
`primary`.** Today `PlanMove::Pass` returns `policy.primary`; with a tier it returns the family's **active
route** whenever that route is in the plan tier. Without this clause the tier would thrash — the very request
after a plan moved A→B would be sent back to A, which is the per-request flip ADR-014 constraint 1 forbids —
so the clause is not an optimization but the tier's coherence condition: **a plan that 403'd is retired for
the family until the whole tier is exhausted and a probe re-admits the head.** (`Pass` with no state row
still yields `policy.primary`, so a family that has never switched is byte-identical to today.)

`on_primary_exhausted: block` now means **refuse while the whole plan tier is exhausted** — the operator's
"fail rather than spend" over every plan, not only the first. The key's name, its values and every existing
config are unchanged, and `overflow_monthly_cap_usd` keeps its subject (the metered tier's spend). `primary`
keeps its other two jobs: the tier's head, and the anchor the `block` refusal's message names.

### 5.3 The metered tier, ranked by price

**Today.** A family on `overflow` spills to **one** named route (spec §4.6 `overflow`), and the only other
mechanism is the global `fallback` list (`docs/spec.md:386`) — a **static, ordered** list that "is a
**failure** mechanism, not an economic preference" (ADR-014's Background).

**This ADR adds the economic preference the owner asked for, and buys it with no new candidate syntax.**

### 5.4 The candidate set is the family tag — not a new list

§4.8's family tag already *is* a set of routes: *"At most one model entry per provider entry may carry a
given tag"* (`crates/vadis-core/src/config.rs:787-789`). So a family can already span several providers, and

> **the family's metered candidate set = every roster route whose provider is `account: api` and whose model
> entry carries the family tag.**

Adding a second metered provider is then a **roster edit and nothing else** — one model entry carrying the
existing tag. That is the owner's chosen config surface (*"按照你的建议进行"*), and it is why the alternative
below is declined.

### 5.5 The knob, and the default

One new key inside a policy entry:

| Key | Type | Default | Semantics |
|---|---|---|---|
| `overflow_selection` | `declared` \| `cheapest` | `declared` | `declared`: the family's `overflow` route is what a spill attempts — today's behaviour, byte-identical. `cheapest`: the metered candidate set of §5.4 is walked in the ranking of §5.6 (the plan tier, §5.1, is drained first in both modes). |

`declared` is the default, so **every existing config keeps its exact behaviour** and no existing case
moves. `overflow` stays **required** in both modes: it is the `block` anchor, the `overflow_monthly_cap_usd`
subject, and the family's canonical metered name in the trace and `/health`.

### 5.6 The ranking (normative)

1. **Key.** Ascending `price.input_miss`, then ascending `price.output`, then **roster declaration order**
   as the final tie-break. The total order is therefore always defined and a config never has an ambiguous
   ranking.
2. **Why `input_miss` first.** A route change's cost is the re-prefill — an **input** event — which is the
   same quantity `plan_switch.switch_cost_nano` prices (ADR-014 item 5, spec §6). Ranking on the term that
   is actually paid on a switch keeps the knob honest; `output` breaks ties because it is the same request's
   other real charge.
3. **Single currency, or the config is refused.** Every candidate's provider entry must declare the same
   `currency` (§4.8). A mixed-currency family is a load error naming the family and the currencies found —
   because §4.8 and constraint 5 forbid comparing or adding two currencies, and a ranking *is* a comparison.
   (The shipped roster already carries USD and CNY entries; this is not hypothetical.)
4. **Banded models (`price.tiers`, §4.10).** Ranked by their **first band** — the lowest ceiling's prices,
   the one band every entry has — which is already the convention §6's own infeasibility figures use, and
   which invents no estimate. The chosen band is stated in the trace (§6) so the reading is not mistakable
   for a band-faithful quote.
5. **The ranking is a pure function of (stable config, roster)** — no turn number, no wall clock, no RNG
   (constraint 2). Two evaluations on one revision give one order.

### 5.7 What a spill does

The walk is `[the plan tier, in declaration order] → [the metered tier, in rank order] → global fallback`,
the family's own candidates keeping their existing precedence over global `fallback` (spec §4.2). Per-wire
eligibility (ADR-022) and the per-condition refusal (ADR-023) are unchanged: an ineligible candidate is
*skipped* in the keyless class, in chain order, with its own reason in `skipped[]` — **the tiering changes
the order, not the eligibility rules**.

### 5.8 `overflow_monthly_cap_usd` under `cheapest`

The cap's subject becomes **the family's whole metered spend** — summed over every route in the candidate
set, not the `overflow` route alone — because the family's metered spend is exactly what the cap names
(*"the family's **metered** spend"*, spec §4.6). Two consequences follow, and both are refusals:

- every candidate's provider currency must be **USD** (the cap's own name; `declared` mode keeps its current
  single-route subject and its current rule); a non-USD candidate is a load error naming
  `plan_policy.overflow_monthly_cap_usd` and the currency found;
- the spend is measured from the log (`Query::OverflowSpend`, `crates/vadis-proxy/src/forward.rs:1763`),
  extended from one route to the set — a SUM over `cost.computed` rows, so no counter can disagree with the
  log (DESIGN §12.10.8's own stance).

### 5.9 Declined: an explicit `overflow: [route, route, …]` list

More control over order, and it would let an operator pin a preference price cannot express. Declined
because spec §4.6 is explicit that a second family is *"never a reshaped section"* and §4.8 already
provides the set — an explicit list would give the same fact two spellings (the tag's set and the list) and
two places to drift, which is the ambiguity §4.14 and §5.4 above exist to remove.

---

## 6. Determinism, cache, and the one trade this ADR refuses to make

ADR-014's Background constraint 1 is the load-bearing sentence: *a policy that re-decides per request makes
the route flip within one session and pays a re-prefill every time it does* — trading the first-order lever
(prefix stability) for the second-order one (model choice). A ranking capability is the single most likely
way to reintroduce that trade, so it is closed explicitly:

1. **The ranking is evaluated at the decision point, and the session is pinned to the result.** A session
   that has spilled keeps its chosen candidate; the existing sticky binding (§6 `sessions`, DESIGN §6) is
   the pin — **no new state is introduced**. A later reload that re-ranks, or a competitor that becomes
   cheaper, does **not** move a live session. `CONF-96` pins both halves (the old session stays, a new
   session takes the new ranking).
2. **Determinism is a function of (content, stable config)**, per constraint 2: the same revision and the
   same session state give the same route. No turn number, no clock, no RNG anywhere in §5.6.
3. **Every route change is priced** (§3's last paragraph for the key; ADR-014 item 5 for the account), and
   an unchanged route writes nothing — so the capability's cost is visible in the same places the existing
   displacement cost is.
4. **Rotation does not run ahead of need.** Keys are tried in order, at most one attempt each, and only
   after a credential-class failure. A request never pays a rotate it did not need, so the pool cannot
   become a per-request roulette. (This is where a "use every key" implementation usually breaks the cache
   contract; v0.1's answer is *use every key it must, in order, and nothing more*.)

---

## 7. The inspectable surface (the highlight's own evidence)

The capability must be verifiable from a running process — the owner's black-box/audit preference, and
AGENTS constraint 3's stance that the serving path's only output channel is the trace and the log. Two
surfaces grow, both **reads of loaded config plus projections**:

**(a) `/health`'s `providers[]` — the pool, per key.** Today one boolean
(`crates/vadis-proxy/src/health.rs:87-104`, and the `ProviderKeyFacts` struct at `:56-62`):

```
"providers": [ { "name": "zai", "api_key_env": "ZAI_API_KEY",
                 "keys": [ { "env": "ZAI_API_KEY_A", "present": true,  "cooling_down": false },
                           { "env": "ZAI_API_KEY_B", "present": false, "cooling_down": null } ],
                 "api_key_present": true, "available": true, "region": "intl", "currency": "USD" } ]
```

`api_key_env` and `api_key_present` keep their current values (the one-key spelling's own facts, now the
pool's first name and the pool's disjunction) so **no existing reader moves**; `keys[]` is additive. Names
may be reported; **values never are** (spec §4.7's own sentence, unchanged).

**(b) `/health`'s `plan` member — the tiers, per family.** The section gains the two facts the highlight
exists to make checkable: `route` (which route the family is on — §5.2) and, for a policy with
`overflow_selection: cheapest`, the resolved order it will actually walk:

```
"metered_candidates": [ { "route": "zai-cn/glm-5.3", "currency": "CNY",
                          "rank_key": { "input_miss": 0.008, "output": 0.028 }, "rank": 0 },
                        { "route": "zai/glm-5.3",    "currency": "CNY", … "rank": 1 } ]
```

`rank_key` names the exact pair §5.6 ordered on, so the order is **checkable by hand against the roster** —
the audit-evidence property this ADR's §1.2 obligation 2 demands. The member is **absent** under
`declared` (nothing was ranked; inventing an order for a mode that has none would be the "a state nobody
can see" error in reverse — §9.1's own existing rule for `plan.configured: false`, `docs/spec.md:2403`).

**(c) The shape of `plan`.** With `plan_policy` written it stays **today's object** — every existing
`/health` reader and all eleven case files that read it (`conf_41/44/71/72/73/74/75/76/77/78/82`) are
untouched. With `plan_policies` written it is a **list of the same objects**. The shape follows the key that
was written, which is unambiguous because exactly one of the two is (§4).

**(d) The metrics surface.** The five plan series already carry a `family` label
(`crates/vadis-cli/src/metrics.rs:256-320`); with several families they are emitted **once per family**. The
guard at `:256` (`if let Some(fam)`) becomes a loop over the declared families. No series is added, renamed
or re-labelled; `ADR-041`'s contract is unmoved.

**(e) The trace.** `result.plan_switch` (`docs/spec.md:2048-2055`) gains two members, present only under
`cheapest`:

- `candidates: ["<route>", …]` — the ranking that was resolved, in order (the same list `/health` shows);
- `chosen: "<route>"` — which of them the walk settled on.

and `decision` gains `key_index` (the credential that served, integer, `null` when the provider holds one
key or when the pool's first name served) — the audit half of §3's "name the index, never the value".
Both are additive; `plan_switch` stays present-and-null as always when no displacement happened.

---

## 8. Refusals this ADR adds (all load-time, all naming their own key)

| Combination | Fails as | Message names |
|---|---|---|
| both `api_key_env` and `api_keys` written | load error | both keys |
| neither written | load error | both keys |
| `api_keys` with a duplicate or empty name | load error | `providers[i].api_keys[j]` |
| both `plan_policy` and `plan_policies` written | load error | both keys |
| neither written | **loads** — the no-plan configuration (§4.6's "at most one policy"; rule 4 above). Only a written explicit `plan_policies: null` is refused, by `plan_policies:` itself | `plan_policies:` |
| two `plan_policies[i]` naming one family tag | load error | `plan_policies[i].family` |
| `overflow_selection` is neither `declared` nor `cheapest` | load error | `plan_policies[i].overflow_selection` |
| `cheapest` and the candidate set's currencies differ | load error | the family + the currencies found |
| `cheapest` + `overflow_monthly_cap_usd` and any candidate is not USD | load error | `plan_policies[i].overflow_monthly_cap_usd` + the currency |
| `cheapest` and the candidate set is empty (only `primary`'s tag exists) | load error | `plan_policies[i].overflow` (the family names no metered route) |

The last row closes a real hole: `family` already requires *an* `overflow` route (§4.6's existing check), so
the set is never empty in practice — the row states that `cheapest` adds no new way to be empty rather than
leaving it to be discovered.

**The plan tier adds no new refusal.** It is discovered by exactly the two conditions §4.6's existing check
already enforces on `primary` — a roster route of an `account: coding_plan` provider whose model entry
carries the tag (§4.6's table, rows 1 and 7) — applied to the set rather than to one route. So a config whose
plan tier is not a singleton is legal today (its second plan member is simply an entry no policy routed
specially), and `cheapest` is not what makes it safe: the tier's members satisfy the same checks
individually, and `primary`'s membership is the non-empty guarantee.

### 8.1 One rule changed in place, and the words it adds

§4.6's `primary` was *"the subscription route"*; it is now *"the subscription route the plan tier's walk
starts from"*. The key, its type, its required-ness and its validation are unchanged, so this is a
**clarification** of a frozen row, recorded here because a reader of the old sentence would otherwise read
`primary` as the family's only plan. Likewise `plan_state`'s value generalizes from a two-valued word to the
**active route** (§5.2) while its wire words (`primary`/`overflow`) stay — so every existing reader — and the
`plan.switched` `reason` vocabulary gains exactly one word, `plan_exhausted` (§5.2). Three edits to frozen
contract text, each named, each additive in effect.

---

## 9. The cases (`CONF-91…97`) — the allocation

Allocated by the owner's act of §1.1 (AGENTS constraint 9 / ADR-012: the gate side is the owner's). Each
lands with the implementation it witnesses and is parked `#[ignore = "CONF-NN: depends on <item>"]` until
then (CONF-20…25's precedent, `design/DESIGN.md:1106-1114`). No existing assertion is touched; the
register's occupancy paragraph is amended in the same commit (§10).

| ID | Subject | The assertion, in one line |
|---|---|---|
| `CONF-91` | §3 rules 3/4/6·the pool rotates, and `quota_exhausted` does not | two-key provider: key#1 `401` → key#2 serves, each key used once, `error.classified.action = "rotate_credential"` with the index, `failover_from` null (the provider was not left); **and** a `403 quota_exhausted` moves the account (§4.6) with **no** rotation, so the two axes cannot be confused |
| `CONF-92` | §3 rule 7 + §7(a)·per-key presence and the secret boundary | `/health` reports `keys[]` per entry with the one-key fields unchanged; a half-present pool is available; **no canary byte** of any key value appears on stdout, stderr, `/health`, the trace or the events (CONF-70's shape, at N keys) |
| `CONF-93` | §4·the list, and independence | two families in one config each spill and probe **independently**; `/health`'s `plan` is a list of two; `plan.switched.family` names the right one; plus the refusal arms (one tag twice; both keys written) |
| `CONF-94` | §5.4·the ranking **is** the walk order | three metered providers at three prices: the cheapest is attempted first, the second-cheapest after its failure, and the declaration order is deliberately **not** the price order so a pass cannot be an accident; `skipped[]` and the trace carry the resolved ranking |
| `CONF-95` | §5.3·the total order and the currency refusal | `input_miss` ties broken by `output`, then roster order, each arm constructed to disagree with the others; a mixed-currency family refused at load naming family + currencies; a `price.tiers` model ranked by its first band; two evaluations on one revision identical (the purity arm) |
| `CONF-96` | §6 rule 1·the pin, and its limit | a session spills to the cheaper candidate; a reload makes a competitor cheaper; the **same** session's next turn stays on its pinned route (no second re-prefill, ledger reconciles) while a **new** session takes the new ranking |
| `CONF-97` | §5.1/§5.2·the plan tier drained before any spend, and the state machine across it | **two plan accounts, one family, drained in declaration order, then metered, then back.** Two `account: coding_plan` providers carry one family tag (declared so that the roster order *is* the intended preference): the first plan mock 403s, the walk moves to the **second plan** — the family **stays in-plan** (its `plan.switched.reason` is `plan_exhausted`, `account` still `primary`, the metered mock receives **nothing**) — and only when the second plan 403s too does the walk reach the metered tier (`primary_exhausted`, `account: overflow`). Then the recovery: after the cooldown a **new** session's boundary probe reaches the **tier's head** and, once it answers 200, an `overflow → plan` move is recorded (`primary_recovered`) and the metered mock stops receiving. Each mock's own request log is the evidence at every step, and `/health`'s `plan.route` names the plan actually active throughout. Depends on: the plan-tier discovery and the walk (`vadis-proxy/src/forward.rs`, `vadis-core/src/plan.rs`), the generalized `plan_state` projection (`vadis-store`), the `plan_exhausted` reason and `/health`'s `route` |

---

## 10. What this ADR does not do

- It does **not** write the code, the config keys, the spec/DESIGN amendments in their final form, or the
  book section. Everything frozen here is implemented by **R66-1** and audited by **R66-0b**.
- It does **not** touch `config.example.yaml` or `providers.example.yaml`. Both must keep parsing with the
  **shipped** parser (`deny_unknown_fields`), so `api_keys`, `plan_policies` and `overflow_selection` may
  appear there only in the commit that teaches the parser them. **R66-1** makes that edit with the code.
- It does **not** add a quality/efficiency axis (§1.2 refusal), a background prober (§1.1's answer), an
  explicit `overflow:` list (§5.9), an explicit plan-order key (**§5.1** — the tier's order is roster
  declaration order, the owner's ruling), an untagged `plan_policy` (§4), or any per-request re-decision (§6).
- It does **not** introduce a third tier, a weighted score, a per-request plan choice, or a "try the next
  plan on a *failed* (as opposed to *quota-exhausted*) request" rule: the plan tier moves exactly the way
  ADR-011's provider demotion already moves the chain, and the tier's *order* is the only thing this ADR
  adds to it.
- It changes **no** price, no gate, no corpus, no threshold, no L1 envelope and no other case's assertion.
  The five existing plan metrics keep their names, labels and derivations (§7(d)).
- It does not define the `Selector` plugin surface. DESIGN's `P3 resolution / surface Selector` stays
  **blocked @ L4** (`design/DESIGN.md:4225`) — this capability is landed in the **core guard**, where
  `PlanFirstRule` already lives (`crates/vadis-core/src/plan.rs`), exactly as §4.6 landed it. A future round
  may migrate the *policy* to the plugin surface, and this ADR neither pre-empts nor blocks that.

---

## 11. Consequences, alternatives, reversibility, and the register

| Decision | Alternatives | Gain / sacrifice | Reversible? |
|---|---|---|---|
| **A pool key (`api_keys`), exactly-one-of with `api_key_env`** | make `api_key_env` accept a list (reshapes a frozen key); a separate `credentials:` section | Gain: zero existing config or case moves; §4.14's ladder reused rather than reinvented. Sacrifice: one more key in the file | **Yes** — additive; a config using it fails to load on an older binary, which is the declared direction (no compatibility shim, ADR-048's stance) |
| **`plan_policies:` list beside the `plan_policy:` mapping** | untagged enum under one key; hard-switch the key to a list | Gain: every refusal still names its key; old configs untouched. Sacrifice: two spellings for one fact, with a rule between them | **Yes** (additive) |
| **The tag *is* the candidate set; ranking by `overflow_selection: cheapest`** | an explicit `overflow: [route, …]` list; a score field | Gain: an operator adds a metered provider by adding a roster entry, and the ordering is derived from the sourced price table alone — no new number to maintain, no fabrication risk | **Yes** (a mode value; `declared` reproduces today exactly) |
| **Price-only ranking (`input_miss`, `output`, roster order)** | a hand-written priority; an offline-computed quality score | Gain: constraint 4/5 are satisfied by construction. Sacrifice: a genuinely better-but-pricier model is not chosen — which is the owner's own ruling | **Yes** (an additive key if a quality axis is ever wanted, its own ADR) |
| **The plan tier drained in declaration order, `primary` as its head** | a price ranking (impossible — in-plan marginal price is 0); an explicit order key | Gain: "drain every plan before spending" needs no new key, and several plan accounts of several vendors compose by §4.8's existing tag. Sacrifice: the order is a *configuration* fact (roster order), so re-ordering the roster re-orders the drain — stated, not hidden | **Yes** (additive; `primary` alone reproduces today) |
| **`plan_state` generalized to the active route, wire words kept** | keep the boolean and model the tier outside the state | Gain: "which plan am I on?" is a projection read, not an inference, and every existing reader is untouched. Sacrifice: one field's meaning widens — named in §8.1 so a reader is not surprised | **Yes** (the wire words are unchanged, so a reversion is a projection rebuild) |
| **Session-pinned, no re-ranking mid-session** | per-request ranking | Gain: preserves ADR-014 constraint 1's first-order lever. Sacrifice: a session does not benefit from a price that improves mid-flight | **Yes** |

**Register this card opens** (non-blocking):

| id | item | owner | due |
|---|---|---|---|
| `R66-0-F1` | `overflow_selection: cheapest` does not model a candidate's **`context`** against the request's size — a cheaper route with a smaller context window could be ranked first and then fail upstream. Ranking on price alone is the owner's ruling; the interaction is recorded, not silently handled. | owner (if a context pre-filter is wanted) | open |
| `R66-0-F2` | The credential cooldown's `(provider, key_index)` projection is a **new table**; ADR-011 item 7's "per-credential exhaustion with a TTL" is prose there and gets its first schema here. | R66-1 | open |
| `R66-0-F3` | `CONF-95`'s `price.tiers` arm ranks on the first band (§5.6 rule 4). If a band-aware ranking is ever wanted it needs a request-size estimate the vadis does not have (GAP-Q14/Q20) — recorded so the simplification is not mistaken for a measurement. | R66-1 (note) | n/a |
| `R66-0-N1` | Cost: this card is docs-only, offline, `$0.00` — no provider dialled, no credential read, no network read at all. | — (note) | n/a |
| `R66-0-N2` | The §12.8 heading and its occupancy paragraph are repaired in the same commit as the seven rows (§10) — a heading whose last row is `CONF-97` while the title says less is the drift that section forbids (R50's precedent, `design/DESIGN.md:1500-1511`). | R66-1 | open |
| `R66-0-F4` | The plan tier's order is **roster declaration order** (§5.1), so an operator who reorders the roster silently reorders the drain. This is stated (not hidden), and `primary` pins the head — but a future "explicit plan order" key is the natural follow-up if reordering proves error-prone in practice. | owner (if wanted) | open |

**What this ADR does not change.** The byte boundary (constraint 1 — the pool adds a header, not a byte),
the observation boundary (constraint 3 — §7's additions are reads), the accounting labels (constraint 4 —
no new figure, and nothing here may be counted as a saving), the price policy (constraint 5 — the ranking
*reads* the sourced table and writes no number), and ADR-014's three frozen rules: session granularity
(item 1), the session-boundary probe (item 2), and the guard's rule order (item 9).

---

## Dated note — 2026-10-06 (R68-0; **F-12** of R66's close-out register)

**This note is appended because this ADR is append-only: not one line above it is edited.** It repairs the
*wording* of one sentence and points at the authority rather than restating it.

**§7(e)'s `key_index` clause names two null classes where the code has one.** §7(e) reads *"the credential
that served, integer, `null` when the provider holds one key **or when the pool's first name served**"*. Only
the first half is true. The rule the code implements is one class: `decision.key_index` is `null` **iff** the
entry writes the single-key spelling (`api_key_env`), and it is the credential's **integer index into
`api_keys`** — `0` for the pool's first name — whenever the entry writes the pool. The code sites are
`crates/vadis-proxy/src/forward.rs:1536` (`key_index: multi_key.then_some(key_cursor as u32)`) and
`crates/vadis-proxy/src/stream_forward.rs:206` (*"`decision.key_index` is null on the single-credential
spelling"*); `CONF-91` and `docs/spec.md`'s `decision` field row agree with them, so this ADR's sentence is
the outlier — written before the index's base was frozen. Read §7(e)'s clause as: *`null` when the entry
holds one credential; otherwise the index, in `api_keys` declaration order, of the name that served — `0`
for the first.*

**What this note does not do.** It re-opens no decision: §3 (the pool), §6 (the trace members) and §9 (the
cases) are untouched, no key is added, and no assertion is edited (AGENTS constraint 9 / ADR-012 — this is a
wording repair, not a measurement, and it moves no gate, corpus, threshold or L1-envelope value). It closes
the item R66's close-out carried as **F-12** against the code at `5d78f7b`; `CONF-91` stays the witness.
