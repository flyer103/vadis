# ADR-063 — the failover order is a rule: healthy before cooling, plan before metered, cheap before dear

- Status: **accepted, contract only** (R75-0, 2026-11-03). The rule is frozen here; the code that makes it
  real lands in a **separate** change (`R2`, the implementing card against this ADR), and the proof that it is
  load-bearing in a third (`R3`). Until `R2` lands, the chain walks in the order it walks today — which is
  exactly the order the incident in §1.1 walked.
- Date: 2026-11-03
- Kind: **a contract change with no product byte.** It amends `docs/spec.md` (§4.2's chain, one clause of
  §4.6, one clause of §4.20.5, §6's `result` group), `design/DESIGN.md` (§12.8's failover-chain note,
  §12.10.5 row 8 and the "money-bearing rows carry their unit" paragraph, §12.9's register), and takes one
  dated note each on `ADR-014`, `ADR-049` and `ADR-056`. It adds **no config key**, **no price**, **no rate
  table**, **no code**, and **no conformance case** — §9 says why the last of those is a deliberate absence
  and what witnesses the rule instead.
- Authority: **the owner's card of 2026-11-03** (`R1`, this card), whose evidence is one production failover
  of **2026-10-22 15:12 CST** (§1.1); and, behind it, the owner's standing ruling of 2026-10-04 quoted in
  `ADR-049` §1.1 — *metered candidates are ranked by **price alone**, from the config's own sourced table,
  with no invented quality axis*. This ADR extends that ruling from the family's metered tier to **the chain
  the walk holds**, which is the surface the incident used.
- Related: **ADR-049** §5.1 (the plan tier's declaration order — **preserved, and now derived**), §5.5 (the
  `overflow_selection` knob), §5.6 (the price ranking — **generalized**, not replaced), §5.7 (the walk), §6
  (the session pin), §7(b)/(e) (`/health`'s `metered_candidates`, the trace's `candidates`/`chosen` — the
  record this ADR mirrors); **ADR-011** items 3 (what a class forbids), 4 (demotion scope), 5 (the provider's
  own clock), 6 (the not-billed retry), 8 (one event, mirrored in the trace), 9 (a failover prices the cache
  it breaks); **ADR-056** §2.5 (the route-scoped, advisory `stream_silence` row — **its preference becomes a
  rank**, and its advisory character is preserved); **ADR-014**'s Background constraint 1 (a route change is
  a cache event) and its sentence that the chain is *"a failure mechanism, not an economic preference"*
  (**the label this ADR narrows** — every clause behind it stands; the dated note on that ADR says which); **ADR-022/023** (per-wire eligibility and the refusal — untouched);
  **ADR-024** (a pre-attempt skip narrates `failover_from` and writes no row — untouched); **ADR-051** §2.4
  (the guard re-enters at every commitment point — **the guard is above the order**); **ADR-018 / spec §4.8**
  (a currency is never compared with another); **ADR-060 D3** (the same-route retry runs *before* the walk —
  untouched); **ADR-005** (the trace is the analysis channel — why §6.4 mirrors the record); **AGENTS.md**
  constraints **1** (no relayed byte), **2** (determinism), **3** (the observation boundary), **4**
  (verified vs inferred), **5** (no fabricated prices), **8** (docs before code), **9** (the measurement is
  not part of the search space — hence §9's empty allocation).
- Cases: **none.** This card allocates no `CONF` id (AGENTS constraint 9: the gate side is the owner's, and
  allocating an ID is the owner's act, not a contract card's). §9 names the four assertions `R3` must make and,
  for each, what must turn it **red**.

---

## 1. Why

### 1.1 The incident, as its own record states it

One failover fired in a week of production, 2026-10-22 15:12 CST: a `connect_failure` on the `deepseek`
route moved the request to `kimi-cn/kimi-k3`. The chain worked — the client got served — and the request
cost roughly **twenty-five times the week's average request**, on an input price roughly **two orders of
magnitude above the route it left**. Those two ratios are the card's reading of that request's own cost
record; they are **not** prices, and no price, rate or table is restated in this document (constraint 5 —
the roster's tables are sourced and dated there, and that is the only authority for them).

The failure is therefore not a failure of the chain's *mechanism*. It is a failure of the chain's **order**,
and of the fact that **nothing in the record says why that destination and not the other one**. The chain
held three candidates; the walk took the one that happened to be written second.

**Would the rule have changed that hop?** On the deployment's own chain, the two candidates that follow the
head are both denominated in the same non-USD currency, and the declared order of those two is **not** their
price order — the entry written third declares a cheaper input price than the one written second, and
`input_miss` is the term a switch is actually paid on (ADR-049 §5.6 rule 2). So if that third entry was
healthy and declared the inbound protocol, the order this ADR rules would have attempted it first, and it
would have attempted it *by rule* rather than by luck of the declaration. Whether it was in fact eligible is
not asserted here — it is exactly what the new record makes answerable (§6), and `R3` builds the 10-22 shape
as a fixture to answer it. (The comparison above is stated as a relation and deliberately quotes no price:
the roster is the only authority for those numbers, constraint 5.)

### 1.2 What today's record answers, and the fourth question it cannot

`failover.triggered` today carries `attempt_index`, `reason` (the error class), `from`, `to`,
`reprefill_tokens` and `switch_cost_nano` (§6.1). With the trace's `result.failover_from` and
`result.plan_switch` beside it, the record answers three questions:

| # | Question | Answered by |
|---|---|---|
| 1 | what left, and what arrived? | `failover.triggered.from` / `.to`, `result.failover_from` |
| 2 | why did it leave? | `error.classified`'s `reason` + `matched` (ADR-011 item 8) |
| 3 | what did the move cost in cache? | `reprefill_tokens` + `switch_cost_nano` (ADR-011 item 9, both `inferred`) |

The incident is not a question from that list. It is the **fourth** question — *why that destination, and
what would have been the alternative?* — and no member of any medium answers it: the payload names the
candidate the walk chose and is silent about every candidate it did not, about their relative price, and
about whether the ones it skipped were skipped or merely unlucky in the declaration. An operator reading the
row afterwards cannot tell *"the chain had no cheaper healthy candidate"* from *"the chain had one, and the
order put it second"*. Those are different defects with different fixes, and the record conflates them.

### 1.3 The three contracts the ruling has to reconcile, and the sentence each contributes

| Contract | The sentence that binds this ADR | What it forbids |
|---|---|---|
| **ADR-049 §5.1** | *"The walk starts at the policy's `primary` and then visits the tier's remaining members in **roster declaration order** … inside a plan the marginal price is **0**, so every plan member is price-equal and price cannot order them."* | Re-ordering the plan tier **by its declared price table**. A plan's table is what the vendor would charge *outside* the plan; inside it the marginal price is 0 (§4.6 rule 4), so a price-ranked plan tier would be reading the wrong column. |
| **ADR-011** | *"Only quota/billing exhaustion is a fact about the account; every other failure is a fact about the attempt **or the route**"* (item 4's own summary), and item 3's classes that must not move at all | Treating a class that does not walk as if it produced a walk, and treating a *route*-scoped fact as a *provider*-scoped one. |
| **ADR-056 §2.5** | *"the walk **prefers** an eligible candidate whose route is not silence-cooling, and leaves a cooling one only when such a candidate exists … it **never refuses a request**, never appears in `skipped[]`, and never produces `no_available_route`."* | Making a `stream_silence` row binding — including by the back door of an ineligibility list that a refusal is derived from. |

The three do not conflict; they are three answers to three different questions (tier order, what may be
retried or skipped, what "healthy" means). The order that satisfies all three at once is stated in §2.

---

## 2. The ruling

> **The failover candidate order is a rule, not a knob — healthy before cooling, plan before metered, cheap
> before dear, and where the configuration cannot be compared (two currencies, or no price declared) the
> operator's declared order decides; the walk takes the head of that order, and `failover.triggered` records
> the order, the exclusions and the declared price that placed it.**

Concretely, and normatively: **the walk holds a list; the list is produced by `R2`'s one owner as a stable
sort of the declared list under the key below; position 0 is never moved by the sort (it is the resolved route);
the walk attempts the list in order, applying eligibility as it goes.** "Cheapest first" is the *name* of the
ruling, not a claim that every position is price-decided — each position's `basis` in the record says how it
was placed (§6.2).

### 2.1 The rank key

For each candidate the walk may attempt, the key is **lexicographic and ascending**. "The declared list" below
means the candidate list as §4.6/§4.2 declare it — the resolved route, the plan tier's remaining members in
declaration order, the family's metered walk, then the `fallback` entries — i.e. the order the walk holds
before this ADR orders it:

| # | Key | Values | Where it comes from | Which contract owns it |
|---|---|---|---|---|
| 1 | `silence` | `0` not silence-cooling · `1` silence-cooling | the route-scoped `stream_silence` row, read at the decision instant | ADR-056 §2.5 — **its preference, stated as a rank** |
| 2 | `tier` | `0` plan (marginal price 0) · `1` metered | the candidate's provider's `account:` | ADR-049 §5.1 — **its "plans drain first", derived** |
| 3 | `currency` *(metered only)* | the index of the **first appearance of that currency in the declared list** | the provider entry's `currency:` (§4.8) | spec §4.8 / ADR-018 — a currency names a **group**, never an amount |
| 4 | `input_miss` *(metered only)* | the declared per-1K miss price; **no declared `input_miss` sorts after every ranked candidate in its currency group** | the model entry's `price` block (a `price.tiers` model: its **first band**, §5.6 rule 4) | ADR-049 §5.6 rules 1 and 4 |
| 5 | `output` *(metered only)* | the declared per-1K output price; absent = after every ranked one | the same block | ADR-049 §5.6 rule 1 |
| 6 | `declared` | the candidate's index in **the declared list** — which is the roster/tier/`fallback` declaration order, `primary` at the plan tier's head | the declared list's construction (`ADR-049` §5.1/§5.7) | ADR-049 §5.1 (the tier), §5.6 rule 1 (the final tie-break) |

Three consequences follow immediately, and each is the reason a key above is ordered where it is:

1. **On a plan tier the key is a no-op**, because terms 3–5 are skipped for plan candidates and term 6 *is*
   §5.1's order (`primary` head, then declaration). The rule therefore **derives** §5.1 instead of
   overriding it: a plan route is first because its marginal price is 0, and the members tie because they are
   all 0, so the only information left is the operator's own preference — which is exactly §5.1's argument.
   Nothing about a plan's declared table enters the sort.
2. **On a single-currency metered set the key reduces to `input_miss`, `output`, declaration order** — verbatim
   §5.6 rule 1. The currency member can never vary on a `cheapest` family set, because §5.6 rule 3 refuses a
   mixed-currency set at load (§8). So §5.6's ranking is this rule's metered instance, and no second ranking
   exists in the product.
3. **`declared` is the pre-rule order**, so a list whose metered candidates are all price-tied or all
   unrankable sorts *identically* to the way the declared list would be walked — the compatibility property
   §5.2 relies on, and the reason `R3`'s single-candidate case can assert "identical to before this branch"
   rather than "identical except".

**One arm of the key is unreachable through a validated config, and it is kept anyway — as a totality
guarantee, not as a state to hunt for.** `R2`'s card asks for the case of *a route with no declared price*.
It cannot occur: `ModelCfg.price` is required, the flat shape and every band must state all four prices or the
load is refused (`quad_to_table`, `config.rs:1679`; spec §4.0 / §4.10 rule 6), and an **empty** band list is
refused too — *"a price block with no band states no price"* (`config.rs:1563`). So every route the walk can
hold has a declared `input_miss`, and `metered_candidates`' own `INFINITY` arms are documented as *"unreachable
through a validated config — answered, never unwrapped"* (`plan.rs:206-212`). Terms 4–5 nevertheless define a
value for the case so that the key stays **total** (a partial key is an unordered pair, i.e. a walk whose
order depends on the sort implementation), and the record's `declared_no_price` basis names it. What this
changes for `R2`/`R3`: the assertion is a **unit** assertion over the pure order function fed an unpriceable
candidate — never an integration rig that has to find a config the loader would refuse. Registered as
`R75-0-F7`, because a card that hunts for this configuration would otherwise spend a round discovering the
loader already forbids it.

### 2.2 The four boundaries the rule does not cross

| Boundary | Rule | Why |
|---|---|---|
| **The head** | The request's **resolved route** stays at position 0. The sort runs over positions `1..` only. | Re-ordering the head is not failover, it is selection — a per-request re-decision that destroys the prefix cache and pays a re-prefill on every request (ADR-014 constraint 1), and it would override a route the client named. The incident's head was correct; its **tail** was not. ADR-056's advisory pass may still abandon a cooling head (§3.3) — that is a fact about the route's recent behaviour, not an economic preference about which model to use. |
| **The guard** | A candidate the family guard re-routes (spec §4.2's re-entry, ADR-051 §2.4) is placed by **the guard's answer**, never by this order. | The guard is the single source of the route a family's request goes to; an order that overruled it would reintroduce the flip ADR-014 forbids, from the other side. |
| **The same-route retry** | ADR-060 D3's retry runs **before** the walk; it is not a candidate and never appears in the order. | A retry on the same route is not a move (it writes no `failover.triggered` row and no `failover_from`); listing it as a position would make one request's two media of recovery read as one list. |
| **The pins** | A session that has spilled keeps its binding, and a family keeps its active route, exactly as ADR-049 §6 rule 1 and ADR-014 item 1 provide. The order decides the *next decision*, never a live binding. | The rule changes an order, not a state machine. A reload that re-ranks reaches new decisions only. |

### 2.3 What "healthy" means, in one sentence

A candidate is **healthy** when nothing in the cooldown projection says its route is currently unavailable
and nothing says it recently stalled — the first is *binding* (it removes the candidate), the second is
*advisory* (it moves the candidate to the back of the order and no further). §3 states both reads and why
they must stay two.

---

## 3. Eligibility, the cooldown, and the order over the *eligible* set

The card's third question, answered as a sequence rather than as one sentence, because the two halves belong
to two different owners and a reader who merges them will implement a binding advisory row.

### 3.1 The three kinds of cooldown row that exist, and the two questions asked of them

| Row | Scope | Reason word | Asked as | Effect on the order | Contract |
|---|---|---|---|---|---|
| a provider-wide demotion (`QuotaExhausted`, or a `Retry-After` on it) | `provider` | the class's own word | *is this provider refused now?* — `Query::Cooldown { provider, model: None }`, `availability::provider_in_cooldown` | **binding**: every route of that provider is `excluded`, reason `demoted`, with the row named | ADR-011 item 4 |
| a route-scoped row written by a **class** | `route` | any word **except** `stream_silence` | *is this route refused now?* — the union read `Query::Cooldown { provider, model: Some(..) }` | **binding**: that route is `excluded`, reason `demoted` | ADR-011 item 5 — **the read is in place, the writer is not** (`R75-0-F1`) |
| a route-scoped `stream_silence` row | `route` | `stream_silence` | *did this route stall recently?* — `availability::route_silence_cooling` | **advisory**: the candidate keeps its place in the list and moves to the back of the order; it is **never** `excluded` | ADR-056 §2.5 |

The predicate the ruling fixes is therefore **reason-aware and scope-aware, in one expression**: a route is
unavailable iff a live row blocks it **and** that row is either provider-wide or route-scoped with a reason
other than `stream_silence`. That is one small function beside `route_silence_cooling`
(`crates/vadis-proxy/src/availability.rs:63`), and it is what makes ADR-011 item 5's unimplemented half a
*writer*'s problem rather than a *reader*'s: the day a class writes a route-scoped row, the order already
respects it, with no change here.

### 3.2 The sequence: eligibility first, then the order — over what is left

1. **The failure's class decides whether there is a walk at all** (`ErrorClass::fails_over()`). A class that
   forbids a route change (`ContentPolicyBlocked`'s abort, `ContextOverflow`'s compress,
   `PayloadTooLarge`'s same-route retry — ADR-011 item 3) produces **no order, no move and no row**: its
   record is `error.classified`'s `action`, which already exists. This is what the card calls a *class
   mismatch*, and it is deliberately **not** a per-candidate exclusion (§3.4).
2. **The eligible set is computed** by removing, from the list, every candidate that (a) belongs to a
   provider already attempted in this request (ADR-011 item 4's in-request form, `attempted`), (b) is
   unavailable by §3.1's predicate (`demoted`), (c) has no roster entry (`unknown_provider`), (d) has no
   transport or key (`keyless`), or (e) does not declare the inbound protocol (`wire_mismatch`, ADR-051).
   The vocabulary is the walk's existing one (`forward.rs:3131-3134`) plus `attempted`; no new reason word
   is invented for a fact the walk did not have.
3. **The order is computed over that set** with §2.1's key. The advisory silence rank is part of the key
   (row 1), not part of the filter, which is why a lone cooling route is still served: the sort cannot remove
   a candidate, only move it.
4. **The walk attempts the head**, and the record names the order, the exclusions and the chosen candidate
   (§6).

### 3.3 Why the advisory row must remain a *position* and never an exclusion

ADR-056 §2.5.3 refuses a binding reading of `stream_silence` because a binding row can **end a chain**, and a
one-route deployment whose upstream hiccups twice would answer `502` for a minute rather than attempt the
route that was answering. Expressing the same rule as term 1 of the key preserves that property **exactly**
— a sort cannot produce `no_available_route`, because it never removes an element — while making the
preference and the order *one* mechanism instead of two (today the preference is a second pass in
`next_candidate`, `forward.rs:2781`). The record keeps the distinction visible: a cooling candidate appears
in `candidates[]` with its `advisory` member and its `until_us`, and **never** in `excluded[]`.

### 3.4 The class axis is a whole-walk fact, so the record has no per-candidate member for it

A reader of the card will look for "class mismatch" beside "cooldown" and "no price". It belongs one level up,
and saying so is more useful than inventing a member that can never be written:

- a class that **does not walk** produces no order and no `failover.triggered` row (step 1 above); its record
  is `error.classified`'s `reason` + `action` + `matched` — the existing, and the more precise, evidence;
- a class that **demotes the account** (`QuotaExhausted`) is visible per candidate as `demoted`, with the
  row's own `scope` and `reason` — which is where a reader needs it, because it is *which* candidates it
  removed that the incident question asks about;
- a class that **demotes a route** (ADR-011 item 5) is the same `demoted`, with `scope: "route"`.

So the exclusion vocabulary is closed over the walk's real skips, and no member states a fact the walk did
not have. That is also why a candidate with **no declared price is not in `excluded[]`** at all — were such a
candidate reachable (§2.1: it is not, the loader refuses that shape): it would be *attemptable* and merely
*unrankable*, and listing it as an exclusion would make an eligible route look refused — the same error, one
list over, that §3.3 is about.

---

## 4. What happens when the cheapest healthy candidate is a plan route

Asked by the card, and the answer is the rule's own derivation rather than an exception:

**It is chosen, and it is chosen *because* it is cheapest; no special case is needed and ADR-049 §5.1 is not
touched.** A plan route's marginal price is 0 by configuration (§4.6 rule 4: the quota is already paid for),
so term 2 puts every plan candidate ahead of every metered one, and term 3–5 — the price terms — never see a
plan's declared table at all. Within the tier they tie at 0 and term 6 orders them: `primary` at the head,
then declaration order. That is §5.1, sentence for sentence.

Three things this rules out, stated because each is a live temptation:

- **Ranking a plan tier by its declared table.** The table is the vendor's out-of-plan price; inside the plan
  the marginal price is 0, so a table-ranked tier is a category error, and it would break §5.1's coherence
  rule (a plan that 403'd is retired for the family until the tier is exhausted — ADR-049 §5.2).
- **A `plan: true` flag on candidates to force the tier first.** The zero marginal price *is* the flag, and
  it is the config's own statement, not a second declaration of the same fact (§4.14's "one fact, one
  spelling", which ADR-049 §5.9 already applies to the metered set).
- **Assuming the plan tier is the only place a plan route can appear.** A plan route named in the global
  `fallback` list is reachable by the walk, and under term 2 it now walks **before** the metered candidates —
  which is the owner's own standing instruction (*"drain every coding plan you own before you spend"*).
  Registered as a consequence in §10 (`R75-0-F3`), because it is a behaviour change a reader should meet in
  the register rather than discover.

---

## 5. A rule, not a knob — and what happens to the knob that already exists

### 5.1 Why the knob loses

The card offers the alternative honestly (*"roster order by default, `cheapest` opt-in"*), so it is answered
honestly. Four arguments, in ascending order of weight:

1. **The default *is* the defect.** A `cheapest`-opt-in key means the shipped behaviour stays the order that
   produced the incident, and every deployment that does not read this ADR keeps it. The chain "worked" in
   the incident; what failed was a default nobody had ruled. A key whose default is the known-bad order is a
   fix that has to be applied by hand, per deployment, forever.
2. **A knob nobody flips is not a fix** — the card's own sentence, and it is the accurate one. The repository
   has one piece of evidence about whether an opt-in ordering key gets flipped, and it is that the key it
   already has for a *narrower* version of this question is unset on the deployment that produced the
   incident: `overflow_selection` has existed since R66, and that deployment's `plan_policy:` writes
   `family`, `primary`, `overflow`, `on_primary_exhausted` and `recover` — no `overflow_selection:`. The key
   governs the family's metered *set*, not the chain, so this is not proof about a chain key; it is the
   nearest evidence there is, and it says the operator did not go looking for an ordering mode. A second
   opt-in would be read by the same operator, at the same moment, with the same result.
3. **Two knobs interact four ways, two of them incoherent.** A chain-order key beside
   `overflow_selection` produces: `declared`+declared-chain (today), `declared`+price-chain, `cheapest`+price-chain
   (double-ranked, harmless), `cheapest`+declared-chain (a family ranked by price whose tail is not — the
   shape that looks audited and is not). The product would need a rule for the fourth combination, which is
   the rule this ADR writes, minus the honesty.
4. **The owner already ruled the principle.** ADR-049 §1.1's 2026-10-04 authority is *"price only: rank with
   the price table already in the config"*, and §1.2's refusal is any non-price axis. A declared chain
   order that is not a price order is precisely a second, unstated axis — the thing that ruling refuses. The
   knob would make the second axis legal and optional; the rule removes it.

### 5.2 What happens to `overflow_selection`, and the one promise this ADR amends

`overflow_selection` **stays, unchanged, and it is not repurposed**. Its two values choose the metered
candidate **set** a family offers:

| Value | The set | The order of that set |
|---|---|---|
| `declared` (default) | the policy's `overflow` route — one route (ADR-049 §5.5) | nothing to order: a singleton |
| `cheapest` | the family's whole tag set (ADR-049 §5.4) | §5.6's ranking — which is this ADR's metered key (§2.1 consequence 2) |

So the mode decides **which routes a family may spill to**, and this ADR decides **the order of whatever list
the walk holds**. Under `declared` the mode's own list is a singleton, so the rule is vacuous inside it — and
the rule bites on the chain's tail, which both modes share. That is the single sentence ADR-049 §5.5's
*"`declared`: today's behaviour, byte-identical"* loses, and it is named as an amendment rather than left to
be discovered: **`declared` still offers exactly one metered route, byte-identically; the chain's own tail is
no longer walked in declaration order.** ADR-049 takes a dated note recording exactly that sentence and no
other (its own file, appended — ADR-049 is append-only).

### 5.3 What the operator gains that a knob would not have given

With the rule, the operator gains **two** things and needs to flip nothing:

1. **The order is the economic one by default**, so a chain nobody audits does not spend an order of magnitude
   more than it must on its second hop.
2. **The order is *explainable* by default** — §6's record names every position's basis, so the next incident
   is a lookup rather than an investigation. This is the half that a knob would also have bought, and it is
   the half that matters if the rule is ever wrong: a rule that chooses can be wrong (two currencies the
   operator declared in this order, a cooling route that was in fact healthy, a class that does not walk),
   and the record is how the operator sees *which* of those decided a hop. A knob would have bought this too
   — and would have bought nothing else.

---

## 6. The record: what `failover.triggered` must carry

The card's second question. The design principle is one sentence: **the row must let a reader reconstruct the
decision without the config file, the live process, or the trace of another request** — because the incident
was diagnosed from a single record eleven days after the fact, and the config it ran on is not in the store.

### 6.1 The members, existing and new

| Member | Type | Present | Meaning | Written from |
|---|---|---|---|---|
| `attempt_index` | integer | always | which attempt this row succeeds (ADR-011 item 6) | the walk's own counter |
| `reason` | string | always | the error class that moved the request (§8's vocabulary) | `Classification.class` |
| `from` | `provider/model` | always | the route abandoned | the failed candidate |
| `to` | `provider/model` | always | the route the walk moves onto — **the head of `order.candidates`** | the order's first entry |
| `reprefill_tokens` | integer \| null | always (null when no ledger) | the session's prefix tokens; `inferred` (ADR-011 item 9) | the `cache_ledger` projection |
| `switch_cost_nano` | integer \| null | always (null when no ledger) | `reprefill_tokens` at the destination's declared miss price; `inferred` | the pure cost function |
| **`currency`** | `"USD"` \| `"CNY"` … | **always** | **the unit `switch_cost_nano` is denominated in** — the destination's (§4.8) | the destination's provider entry |
| **`from_price`** | object \| null | always | the declared price of the route in `from` (null when that entry declares none) | the roster's `price` block |
| **`order`** | object | **always on a failover row** | §6.2 | the pure order function |

`from_price` earns its member from the incident's own wording: the four questions a reader asks of a
mis-ordered hop are *what did it leave, what did it take, what does each cost, and was there a cheaper
healthy one?* — and the first three are answerable only if both sides of the comparison are in the row, since
the config revision that priced it is not (the trace carries `config_digest`, but the digest names a file, it
does not contain one).

### 6.2 The `order` object

```
"order": {
  "rule":     "cheapest_healthy_first",     // the ruling's own name (frozen token, §2)
  "candidates": [                            // in walk order — the head is `to`; eligibility already applied
    { "route": "<provider>/<model>",
      "tier": "plan" | "metered",
      "currency": "USD" | "CNY",
      "basis": "plan" | "price" | "declared_currency" | "declared_no_price",
      "price": { "currency": …, "input_miss": <as declared | null>, "output": <as declared | null>,
                 "band": null | { "up_to": <tokens> }, "peak_multiplier": <as declared> } | null,
      "advisory": null | { "reason": "stream_silence", "until_us": <integer> } },
    …
  ],
  "excluded": [                              // in list order; binding facts only — never an advisory one
    { "route": "<provider>/<model>",
      "reason": "demoted" | "keyless" | "wire_mismatch" | "unknown_provider" | "attempted",
      "cooldown": null | { "scope": "provider" | "route", "reason": <the row's word>, "until_us": <integer> } },
    …
  ],
  "chosen":   "<provider>/<model>"           // == the payload's `to`, one value written twice (§6.3)
}
```

**Every number in it is the configuration's, read at decision time — none is computed, estimated or
converted.** `input_miss` / `output` are the entry's own as-written values (a `price.tiers` model reports its
**first band** and says so in `band`, §5.6 rule 4 — the reading is therefore not mistakable for a
band-faithful quote); a value the config does not declare is `null`, never a stand-in; `peak_multiplier` is
the entry's declared multiplier, recorded because a base price compared in a peak window is not the price the
request paid; and **no exchange rate appears anywhere**, because no cross-currency comparison is ever made and
none is recorded (§5.6 rule 3's prohibition, extended to the record).

`basis` is the one member that answers *why is it here*: `plan` = placed by term 2 with §5.1's declaration
order, `price` = placed by terms 4–5, `declared_currency` = price was not compared across a currency
boundary, `declared_no_price` = the entry declares no `input_miss` — **the defensive arm of §2.1, unreachable
through a validated config and present so the key is total and the vocabulary is closed**: it is emitted only
if the code's own defensive arm fires, and `R2`'s unit test is its witness, never a roster. **A reader can
therefore re-derive the whole order by hand from the roster** — the same audit property ADR-049 §7(b) demands
of `/health`'s `metered_candidates` and §7(e) of `plan_switch.candidates`.

### 6.3 The three invariants a reader may assert

1. `order.chosen == to`, and `order.chosen` is `order.candidates[0].route` — the walk takes the head, so the
   two members are **one value written by one expression**; a disagreement is a defect, and `R3` asserts the
   equality rather than trusting the writer. (An ADR-051 §2.4 guard **redirection** may still move the
   *attempt* off that head after the row is written; that move is `plan_switch`'s record, exactly as today,
   and it does not change this row.)
2. `order.currency` — the `currency` of the chosen candidate — equals the payload's `currency`. §4.8 requires
   a money member to carry its unit at its own level; the object also carries it so that a reader holding only
   the object is not left with an unlabelled `switch_cost_nano`. The duplication is deliberate and is one
   value, not two facts.
3. No route appears in both `candidates` and `excluded`, and no `stream_silence` row appears in `excluded`.
   The second is ADR-056 §2.5.3's advisory rule, expressed where a reader can check it.

### 6.4 The trace mirror, and why the event alone is not enough

`result.failover_order` carries **the same object** (§6.2) plus `from_price` and `currency`, present-and-null
on every record that no failover moved (the same stance as `failover_from`, so the key set never changes
shape). Two reasons it must exist, both of them contract, not convenience:

1. **ADR-005's channel.** The trace is *the* product → analysis-loop channel; the event log is state truth and
   the analysis loop does not read it. The incident in §1.1 was read out of a trace. A rule whose evidence
   exists only in the store is invisible to the medium the operator actually reads.
2. **ADR-011 item 8's doctrine.** *"Every classification writes one event and mirrors it in the trace"* — and
   ADR-049 §7(e) already mirrors the sibling decision (`plan_switch.candidates` + `chosen`) into the trace for
   the same reason. The failover's order is the fourth question of §1.2; the plan switch's order is the third
   question's sibling; the two records should not answer their questions in different media.

Additive by construction: an added key never moves `TRACE_SCHEMA_VERSION` (`crates/vadis-core/src/trace.rs`,
the `config_digest` / `cache` precedent), so the trace stays **2**.

### 6.5 The event schema moves 2 → 3, and the reason is §4.8's

`EVENT_SCHEMA_VERSION` (`crates/vadis-core/src/store.rs:22`) moves **2 → 3**, for exactly the reason ADR-018
gave for 1 → 2: a `failover.triggered` row now carries **its own unit**, and a reader that knows only v2 would
read a CNY `switch_cost_nano` into a USD sum. A v2 row stays readable and stays **unlabelled**, so the upcast
rule is one sentence: **`currency` absent on a `failover.triggered` row means the row predates this contract,
and its `switch_cost_nano` must not be summed with a v3 row's** (ADR-009 item 7's upcast stance, unchanged in
shape — old rows are never rewritten, readers upcast).

### 6.6 What the row deliberately does not carry

- **A number the config does not state, and no new inferred figure.** No token estimate, no exchange rate, no
  default substituted for an undeclared value. The two money members that already exist keep their `inferred`
  labels (ADR-011 item 9), and nothing this ADR adds is priced by the gateway at all.
- **A candidate the walk never offered.** The order lists the list the walk held; a route no policy, tier or
  chain entry offered is not in it, and its absence is the config's statement, not the record's omission.
- **A per-candidate class verdict.** §3.4: the class's record is `error.classified`, one level up.
- **A recomputed eligibility for a *pre-attempt* skip.** A row is written only when the request **moves**
  (spec §4.2's narration predicate; ADR-024). The skips that happened before the failure keep their existing
  narration (`result.failover_from`, and `details.skipped[]` on a refusal). This ADR does not turn every
  cooling route into a row: a row per request that did not move would be the "a state nobody can see" error
  inverted, and ADR-056 §2.5.3 forbids the shape for the advisory rows in particular.

---

## 7. Determinism, cache, and the pins

1. **The order is a pure function of (stable config, the cooldown projection, one clock read)** — constraint
   2. Terms 2 and 6 read the config; terms 3–5 read the config; term 1 reads the cooldown projection at the
   instant the caller already takes for eligibility (`now_us()`), and nothing else. No turn number, no RNG, no
   second clock: two evaluations on one revision with one projection state give one order.
2. **A route change is still a cache event, and the rule adds no new ones.** It changes *which* route a move
   lands on, not whether a move is paid; ADR-011 item 9's pricing and `switch_cost_nano` are untouched.
3. **The pins stand** (§2.2): the session binding, the family's active route and the reload semantics are
   ADR-049 §6 rule 1's and ADR-014 item 1's. A reload that reorders the roster reaches new decisions only —
   which is also why the record's `basis` matters: after a reload, the row from before it is the only evidence
   of what the old order was.
4. **`R3`'s determinism limb** falls out of (1): the same fixture run twice must yield a byte-identical order
   object modulo the timestamps, and `R2`'s body already asks for exactly that.

---

## 8. Refusals this ADR adds: none, and that is a decision

- **No new config key** ⇒ no new load error, no `vadis setup` askable-key decision, no `deny_unknown_fields`
  interaction (§4.14's ladder untouched).
- **A route with no declared `input_miss` is not a config the loader accepts.** Every model entry states all
  four prices (§4.0, §4.10 rule 6) and an empty band list is refused, so §2.1's `declared_no_price` term is a
  **totality guarantee** rather than a roster state: it defines a position so the key is total, and the record
  names it if the defensive arm ever fires. No new refusal is added by this ADR, and none is removed: the
  loader refuses this shape already, and a rule must not refuse a shape the loader permits.
- **`overflow_selection` keeps both values and its default** (§5.2). §5.6 rule 3's mixed-currency refusal for
  a `cheapest` **family set** stands untouched: the set is *discovered* from a tag the operator did not write,
  so a mixed set is an accident the loader must refuse; a chain is an *authored* list whose currency boundary
  is a declaration the operator made, so it is grouped, never refused (term 3). The distinction is ADR-049
  §5.4's own (tag = set vs explicit list = declined), applied in the direction that does not loosen a frozen
  refusal.

---

## 9. The proof, and the deliberate absence of a conformance case

**No `CONF` id is allocated.** AGENTS constraint 9 puts the gate side — the case definitions, the corpus, the
assertions — outside the mutable scope; an ID is the owner's act (ADR-012), and this card's `Never` says so.
The rule is proved instead by `R3`, which is `R2`'s sibling card and asserts, on rigs of its own:

| # | Assertion | Red when |
|---|---|---|
| 1 | two healthy metered candidates with different declared prices: the dearer-declared first is **not** attempted first | the sort is reverted to `declared` order — the named test must go red (R3 records the panic) |
| 2 | the cheapest candidate is under a **binding** cooldown: it is skipped and `excluded[]` names the cooldown and its `{scope, reason, until_us}` | the eligibility filter is removed — the test must go red |
| 3 | an **unpriceable** candidate — fed to the pure order function directly, because no validated config can contain one (§2.1) — sorts last in its currency group with `basis: declared_no_price` and `price: null` | the price read substitutes a stand-in (a `0`, a default, a conversion) or the key stops being total |
| 4 | one candidate only: the order object and the walk are **identical to before this branch** (term 6 preserves the declared list) | the sort touches position 0, or the tie-break is not the pre-rule index |

Why not conformance: a case would be a **contract on the measurement** (constraint 9) allocated by this card,
and the rule is fully expressible as unit + integration assertions over a pure function (`vadis-core`'s order
key) and a two-route rig. If the owner later wants the rule frozen at the gate, that is an allocation with its
own act — the register carries it (`R75-0-F4`).

---

## 10. Register this card opens

| id | item | owner | due |
|---|---|---|---|
| `R75-0-F1` | **ADR-011 item 5's route-scoped half has a reader and no writer.** `Demotion.provider_wide` is `true` on every path (`error_class.rs:289-299`), so a plain `429` (`RateLimit`) demotes nothing at all today, and only `QuotaExhausted` writes a row — provider-wide. §3.1's predicate already answers the route-scoped case correctly, so the day a class writes a route row the order respects it; **that writer is not this round's** and is not silently implied by this contract. | owner (a class-scope round) | open |
| `R75-0-F2` | **The order is blind to a candidate's `context` window.** A cheaper route with a smaller window can rank first and then fail upstream — inherited verbatim from `R66-0-F1`, and the rule makes it *more* likely to bite (price now decides more positions). A context pre-filter needs a request-size estimate the vadis does not have (`GAP-Q14/Q20`); recorded, not handled. | owner (if wanted) | open |
| `R75-0-F3` | **A plan route named in the global `fallback` list now walks before the metered candidates** (§4). Economically right (marginal 0) and consistent with §5.1's principle, but it is a behaviour change on a legal configuration — stated so a reader meets it here. | owner (note) | n/a |
| `R75-0-F4` | The four assertions of §9 are **not** conformance cases; if the rule is to be frozen at the gate, that allocation is the owner's act (constraint 9). | owner | open |
| `R75-0-F5` | The row grows by three members (`currency`, `from_price`, `order`) and `EVENT_SCHEMA_VERSION` moves 2 → 3. A reader of a v2 row must not sum its `switch_cost_nano` with a v3 row's (§6.5). | `R2` | open |
| `R75-0-F6` | The **book** sentence and `config.example.yaml`'s `fallback` comment are `R2`'s (constraint 8: the behaviour lands there, so the user-facing text lands there — ADR-056 §6.3's precedent). This card ships no user-facing claim about a behaviour not yet served. | `R2` | open |
| `R75-0-F7` | **"A route with no declared price" is not a config state** — `ModelCfg.price` is required and both shapes must state all four prices (`config.rs:1679`), an empty band list is refused (`config.rs:1563`, spec §4.10 rule 6). §2.1's `declared_no_price` arm is therefore a **totality guarantee** with a unit-test witness, not a roster the rigs can build. Registered because `R2`'s and `R3`'s cards both name the case and could otherwise spend a round looking for a configuration the loader has already refused. | `R2` (unit test only) | open |
| `R75-0-N1` | Cost: this card is docs-only, offline, `$0.00` — no provider dialled, no credential read, no network read at all. | — (note) | n/a |

---

## 11. Consequences, alternatives, reversibility

| Decision | Alternatives | Gain / sacrifice | Reversible? |
|---|---|---|---|
| **The order is a rule; no new key** | a `fallback_selection: declared \| cheapest` key, default `declared` | Gain: the default becomes the economic order; one ordering principle in the product instead of two; the record explains by default. Sacrifice: an operator who wrote the chain for a **non-price** reason (reachability, a compliance-bound vendor, a deliberate preference) loses that ordering — mitigated only partially by the currency grouping and by the record naming the order. | **Yes** — a later additive key (`fallback_selection: declared`) restores today's order with one key and no schema change; the record's `basis` vocabulary already distinguishes the two orders |
| **Term 1 = the advisory silence preference, promoted from a second pass to a rank** | leave `next_candidate`'s two-pass find and add the price sort *after* it | Gain: one mechanism, and the advisory rule provably cannot refuse (a sort cannot remove). Sacrifice: the preference now lives in the key, so a reader must know ADR-056 to read term 1 — mitigated by the record's `advisory` member and §3.3's citation. | **Yes** (a rank's position is one line) |
| **Currency groups ordered by first declaration, never converted** | refuse a mixed-currency chain at load (impossible in practice: the deployment's own chain leaves a USD route for CNY ones, and a refusal would have made the incident's configuration unloadable rather than mis-ordered); apply an exchange rate (forbidden: constraint 5) | Gain: the rule applies to the chain that produced the incident, without inventing a comparison the repository forbids. Sacrifice: a cheap route on the *other* side of a currency boundary is not preferred over a dear one on this side — the operator's currency priority decides, which is the only non-invented evidence there is. | **Yes** |
| **`from_price` and per-candidate `price` in the row** | record only the chosen route's price (the card's literal ask) | Gain: the incident's own question (*133× the one it left*) is answerable from the row alone, forever. Sacrifice: two more members on a rare row, and a `price` block inside every candidate of the order. | **Yes** (members are additive; removing one is a version bump, not a redesign) |
| **The trace mirror** | the event only | Gain: the analysis channel can see the order (ADR-005), and the two siblings (`plan_switch`, `failover_order`) live in the same medium. Sacrifice: ~one extra object on the rare records that moved. | **Yes** (a trace member is additive and versionless) |
| **Mirror, not a second row** | a separate `failover.order` event | Gain: one move, one row (ADR-010's "one event per fact"). Sacrifice: the payload is bigger than a writer's minimum. | **Yes** |

**Not decided here, and named so a reader does not infer it:** any quality/efficiency axis (ADR-049 §1.2's
refusal stands); a per-request re-decision of the head (§2.2); a third tier; an explicit ordered chain key
(the rule makes the chain's *membership* the operator's instrument and its *order* the rule's); and any
conformance allocation (§9).

---

## 12. What `R2` owes, and where the code sites are

Named so the implementing card starts from this ADR rather than from a search. Every path resolves at this
branch's HEAD.

| # | Site | What lands |
|---|---|---|
| 1 | `crates/vadis-core/src/plan.rs` (beside `plan_tier:304`, `metered_walk:283`, `rank_metered:246`) | **one pure owner** of the order: a function taking the roster, the policies and the two cooldown predicates' facts and returning the ordered list with each entry's `basis` and price key. Both media call it — the two construction sites below are today's duplicates of the same order and must end up calling one function (`availability.rs`'s doc comment is the precedent: *the second implementations are deleted, not wrapped*) |
| 2 | `crates/vadis-proxy/src/forward.rs:1391` (buffered chain) | call the owner; keep position 0 = the resolved route |
| 3 | `crates/vadis-proxy/src/stream_forward.rs:872` (streamed chain) | the same call, not a second copy |
| 4 | `crates/vadis-proxy/src/availability.rs:35` / `:63` | the reason-aware binding predicate of §3.1, beside the two reads it composes |
| 5 | `crates/vadis-proxy/src/forward.rs:3005` (`record_failover`) | the three new payload members (§6.1) from the same order value the walk used — never a re-derivation |
| 6 | `crates/vadis-core/src/trace.rs:350` (`ResultRec`) | `failover_order`, present-and-null (§6.4) |
| 7 | `crates/vadis-core/src/store.rs:22` | `EVENT_SCHEMA_VERSION` 2 → 3, its doc comment naming this ADR (§6.5) |
| 8 | `book/` + `config.example.yaml`'s `fallback` comment | the user-facing sentence (§10 `R75-0-F6`) — *the chain is walked cheapest healthy first, plans before metered routes, and a chain whose entries cannot be compared keeps the order you wrote* |

No test in `tests/conformance/` is added, edited or deleted by `R2` (constraint 9). Four gates
(`fmt`, `clippy -D warnings`, `build`, `test --workspace`) run verbatim, as `AGENTS.md` states them.
