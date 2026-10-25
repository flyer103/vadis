# ADR-062 — the block that broke the prefix, and the money it cost, labelled `inferred`

- Status: **accepted (2026-10-26), contract only.** It extends `ADR-057`'s step ledger with **two row
  columns**, **one summary block**, and **one label ruling** — and it repairs the wording that ruling owes.
  It writes **no** product byte, allocates **no** `CONF` id, declares **no** price, and moves **no** gate,
  threshold, corpus or L1 envelope (AGENTS constraint 9). The code half is specified here (§5, §6) and
  landed by its own card; the five wording sites it must move are frozen in §5 so that change cannot
  "fix" a red by loosening an assertion.
- Kind: **an extension of a served read-out, not a new surface.** `vadis steps` already prints every fact
  this ADR needs (`kept`, `at`, `blocks`, `new_tok`, `in`, `cached`); what it does not do is **name the
  token weight of what the break kept and what it billed again**, and put a session-shaped number on it
  with the label the accounting discipline requires.
- Authority: the owner's card of 2026-10-26 (`W1`), whose evidence is the week audit: over **4,432
  requests**, `Σ miss` **12.63 M** tokens against **6.30 M** of session-final context — **6.33 M tokens
  (50 % of all miss volume) were re-prefill, not new content**, ≈ **$0.95** at the metered route's own
  rate (15 % of the bill), from **107** mid-session prefix breaks (2.5 % of 4,342 in-session transitions;
  **97** diverging at an `input_item` block, **10** at a `tool`) plus **15** same-session cold re-prefills
  above 20 k. *Those figures are the audit's, quoted as the requirement's provenance — this ADR
  re-measures nothing, and no figure of the audit appears in the product (constraint 6).*
- Amends: `docs/spec.md` **§9.4.2** (the block index's ordering sentence — a stale claim, §9 F2),
  **§9.4.3** (two appended columns and their labels), **§9.4.5** (three summary lines),
  **§9.4.6** (the never-list), and a **new §9.4.8** (the whole contract of this ADR);
  `design/DESIGN.md` **§12.9**'s register; `book/observability-and-accounting.md` (the `steps` section:
  the re-bill paragraph, and two sentences that still print the wrong label word, §9 F1).
  `design/decisions/ADR-057-the-step-ledger.md` is **append-only and is not rewritten** — its §2.2
  ordering paragraph and its §7 factor-of-four row outlived their measurement and are registered here
  (§9 F2) rather than edited.
- Related: **ADR-057** (the ledger, its five class words, its three labels — extended, never redefined),
  **ADR-058** (one word, one meaning: `measured` / `verified` / `inferred`, and the composition rule this
  ADR applies), **ADR-011 item 9** and **ADR-014 item 5** (`reprefill_tokens` / `switch_cost_nano`: the
  *inferred money from measured tokens at a declared price* precedent, adopted unchanged and **not**
  overridden), **ADR-056 §2.4** (the three-bucket partition, untouched), **GAP-Q14** (block tokens are a
  proportional attribution, therefore `inferred`), **GAP-Q17** (a plan's allowance is not a price),
  **ADR-042** (a cache-replay row's money is not an upstream charge), **ADR-005** (hashes, never the
  body), **AGENTS constraints 4 and 5** (no unverified savings, no fabricated prices).

---

## 0. The ruling in one paragraph

The ledger must be able to answer *"which block broke this session's prefix, and what did the re-prefill
cost me"* without a second tool and without a second label vocabulary. So: **at a break, the row names
the block (its index and kind, column 11, already printed) and adds the two token weights that make the
break legible — `kept_tok`, the prefix the break did not touch, and `rebill_tok`, the tokens this step
billed again (columns 18–19, both `inferred`)**; the summary gains **one** number, the session's **re-billed
tokens** (`summary.rebill.tokens`), defined as the measured cache-miss volume above the session's one-pass
floor, priced at **each row's own measured miss rate** (never a price table this surface reads), and
labelled **`inferred`** — with a named list of what it must never be called. A **cold start** cannot be a
re-bill (`rebill(first) = 0` by construction), so the two are told apart by columns the ledger already
prints; and a session with no break reports **zero**, not an empty field.

---

## 1. Why

### 1.1 What the ledger already prints, and what that leaves unanswerable

`ADR-057` §2.3's seventeen columns already carry the *structure* of a break: `kept` (the block count of
the kept prefix), `at` (`#<index>:<kind>` of the first diverging block), `blocks` (the signed delta) and
`new_tok` (the inferred token weight of everything after the divergence). What they do not carry is the
**weight of the part that survived**, and — because a ledger row is one observation — any statement at
all about what the break cost *relative to not breaking*. The three questions a reader of the audit
actually asked are:

| The question | What `ADR-057` answers today | What is missing |
|---|---|---|
| which block broke the prefix? | `at #<index>:<kind>` | nothing — this is already ruled and printed |
| how big was the prefix that survived? | `kept` — a **block count** | the tokens, and they are the ones that get re-billed when the provider's cache does not cover them |
| what did the break re-send, and what does that cost? | `new_tok` — the *tail's* inferred weight, i.e. new content **plus** the re-sent tail, undivided | the **re-billed** part, per step and per session, with a label and a price |

The third row is the whole card. `new_tok` cannot answer it: on an `append` row `new_tok` is genuinely new
content (the client grew), and on a `rewrite` row the same column contains both the edit and the entire
tail the client re-sent — one number, two causes. The audit's 50 %-of-miss-volume finding is exactly the
difference between those two causes, and the ledger currently cannot express it.

### 1.2 The arithmetic the audit had to do by hand

The audit's shape is one subtraction over a session: `Σ miss` minus the session's final context. It is
correct, and it is not expressible on this surface for two reasons: **(a)** the ledger prints no `miss`
column at all (it prints `in` and `cached`, so the reader subtracts per row), and **(b)** a session-shaped
number with no field name and no label is the thing `ADR-057` §1.4 refused to ship as "a script in
someone's dotfiles" — the labels are the product.

So the number moves into the summary, with the two operands printed beside it so that a reader can check
the subtraction on the page, the way `§9.4.5` already prints `steps = succeeded + failed + stream_incomplete`.

### 1.3 What the audit's second finding adds: the row *shape*

The audit separates **107 mid-session prefix breaks** from **15 same-session cold re-prefills above 20 k**.
Those are two different rows in this ledger and today they wear the same face: both print `kept 0`, and
`at #0:<kind>` for the cold ones. A reader can work out that `kept 0` on a non-`first` row means "nothing
of the previous prompt survived", but the surface never says it, and the session's **own** first request —
which is a cold start too, and which the client cannot avoid — prints the same `kept 0`. §2.5 rules the
three shapes apart in the columns that already exist, and adds the arithmetic reason the first of them
can never be priced.

### 1.4 Why this is a contract and not a footnote to `ADR-057`

Two reasons, both of which the repository has already paid for once.

- **The label is the product, and this surface's label is currently the wrong word.** `ADR-058` ruled that
  `verified` names *a difference between two worlds* and that a single observation is `measured`; spec
  §9.4.3 has said `measured` since. The **shipped** command still prints, on every run, *"in/cached/out and
  the block facts are **verified**"* and *"cache … (… input tokens, **verified** — the provider's own
  counters)"*, and the book's `steps` section says the same in two sentences, one of which calls the class
  word *"a name over a verified relation"*. A unit test pins the header sentence, so the wrong word is
  *asserted*, not merely present. This is registered as **F1** and repaired in §5/§9: a surface that is
  about to print an `inferred` figure three lines below a `verified` one is a surface where the reader will
  conclude the wrong half is the weaker number. **The one word this ADR adds must not be added into that
  drift.**
- **The block index means something, and the contract says the wrong thing about it.** Spec §9.4.2 and
  `ADR-057` §2.2 both say the index is *"vadis's own enumeration order of the client's bytes, **not** the
  provider's template order"*. The code has enumerated in **template** order since the 2026-09-20 Plan A
  decision (`crates/vadis-core/src/prefix.rs:73-86`, pinned by `CONF-31`, stated correctly in spec §6).
  `at #7:input_item` is therefore a **template-order** ordinal — the same order the provider's own prompt
  uses — and the sentence a reader is pointed at says it is byte order. This ADR's whole first rule is an
  *index plus a kind*, so the sentence is corrected in place (spec §9.4.2) and registered as **F2**
  (with `ADR-057`'s append-only consequence named there).

---

## 2. The ruling

### 2.1 Attribution: the two columns a break needs (18 `kept_tok`, 19 `rebill_tok`)

Appended to `§9.4.3`'s table, so **no existing column number moves** (`CONF-118` pins column 16's typing;
`CONF-112`/`CONF-113`/`CONF-116` name columns 2-3, 5-8 and 11):

| # | Key | Type | Source | Label |
|---|---|---|---|---|
| 18 | `kept_tok` | integer | `Σ prefix.blocks[i < lcp].tokens` — the inferred size of the prefix the previous step also carried, i.e. the part of the prompt **the break did not touch** | **inferred** (GAP-Q14) |
| 19 | `rebill_tok` | integer \| null | §2.2's step term: `max(0, (in − cached) − max(0, in − peak(n−1)))`; **null** when the row carries no usage | **inferred** |

`kept_tok` is the complement of `new_tok` over the same block list (`kept_tok + new_tok` is the block
sum the proportional attribution distributes over `usage.input_total`), and it is **never null**: it is
computed from the blocks, like `new_tok`, so a `usage_missing` row still prints it. `0` on a row whose
`lcp == 0` is a genuine empty sum — the same measured zero `kept 0` prints beside it (`§9.4.2`), not the
`—` absence.

**Print order.** The text table prints columns 1–16, then **18, 19**, then 17 `flags`: `flags` is the
variable-length cell and must stay last (a right-aligned number after a ragged tail is unreadable). The
numbering is the `--json` key order and the amendment log's reference; the *names* are what both forms are
read by, and §2.6 keeps the values identical.

### 2.2 The definition, exactly

Notation, per session, over the rows that carry usage (`usage_missing` false and both counters present),
in the reducer's order `(ts, trace-file order)`:

- `in(n)` = `usage.input_total`, `cached(n)` = `usage.input_cached`, and
  `miss(n) = in(n) − cached(n)` (saturating) — **the token count the `input_miss` bucket prices** (§6's
  own sentence; this ADR defines no token count).
- `peak(n−1) = max { in(k) : k < n, k carries usage }`, with `peak(0) = 0`.
- `growth(n) = max(0, in(n) − peak(n−1))` — the part of this step that is **its own growth**.
- `rebill(n) = max(0, miss(n) − growth(n))` — the part of this step's miss that is **above its growth**.
- `surplus(n) = max(0, growth(n) − miss(n))` — the part of this step's growth the cache covered anyway.

Then, and this is the sentence the surface is read by:

> **`summary.rebill.tokens` — the session's *re-billed tokens* — is the measured cache-miss volume of the
> session's steps above the session's one-pass floor, counted step by step and floored at zero on each
> step: `Σ_n max(0, (in(n) − cached(n)) − max(0, in(n) − peak(n−1)))`.**

Two identities follow, and the report prints the second for audit:

| Identity | Why it holds |
|---|---|
| `Σ growth(n) = floor`, where `floor = max_n in(n)` | the positive increments of a running maximum telescope |
| **`rebilled = Σmiss − floor + surplus`** | `rebill(n) − surplus(n) = miss(n) − growth(n)`, summed |

`Σmiss`, `floor` and `surplus` are all printed beside the number, so the reader can check
`84120 = 212000 − 132000 + 4120` on the page — the reason §9.4.5 already prints `steps` next to the three
buckets.

**Three consequences, each of which is a rule:**

1. **The session's own cold start can never be a re-bill.** `rebill(1) = miss(1) − growth(1) =
   (in(1) − cached(1)) − in(1) = −cached(1) ≤ 0 → 0`. It is arithmetic, not a special case: the first step's
   growth *is* its whole prompt, so nothing existed for it to re-send. §2.5 leans on this.
2. **The clamp is arithmetic, not concealment — and it can only raise the printed number.** `rebill_tok`
   is a count of tokens billed twice, which is non-negative by construction; a pre-clamp negative means
   this step's miss fell *short* of its growth (the provider's cache covered tokens this session had not
   itself sent — a shared system prompt cached by another session, or an overlap between two requests).
   Those tokens go to `surplus`, which is added back in the identity, so the report never loses them. This
   is deliberately **not** `§9.4.3`'s `gap_ms` rule: a `gap_ms` is a measured difference whose sign is
   information and is printed as measured; a `rebill` has no negative population to print.
3. **The floor is the session's largest prompt, not its final one.** They coincide exactly for the
   ordinary append-only session (its last request is its biggest) — which is the shape the audit's corpus
   is dominated by, and on which `rebilled = Σmiss − final` with `surplus = 0`. Where they differ, the
   peak is the only defensible floor: a session that shortened after its peak already paid for that
   content once, and a *final*-context floor would book the compaction's replacement text as a re-bill.
   §7 keeps that alternative and its cost.

### 2.3 The money, and exactly where the price comes from

Per row, in that row's own currency:

```
rebill_nano(n) = floor( rebill_tok(n) × cost.input_miss(n) / miss(n) )     — only when miss(n) > 0
```

- **The price is the row's own measured miss rate.** `cost.input_miss(n)` is the money the cost engine
  already charged this row at miss; divided by the token count that same bucket priced, it is
  **`p_miss` as the config declared it and as the engine applied it** — the band (`§4.10`), the peak
  multiplier, the currency, all included, none re-read. This surface **reads no price table, declares no
  price and derives none from a page** (constraint 5): it has the money and the token count in the record
  it already loaded. `--config` is still required, for the reasons §9.4.1 gives (`plan` label, trace dir).
- **`miss(n) == 0` is never divided.** Such a row has `rebill_tok(n) == 0` by §2.2 (a miss of zero is
  below any growth), so the guard and the arithmetic agree.
- **A plan row is counted, never priced.** Its rebill tokens land in `plan_tokens`, and no currency line
  receives a zero (§4.6 rule 4, GAP-Q17, `§9.4.5`'s existing `plan rows` rule).
- **A row whose rebill tokens cannot be priced contributes tokens and no money, and says so.** That is a
  row with `cost.input_miss == 0` while `miss > 0` and not a plan account — a cache replay (ADR-042,
  whose money is not an upstream charge) or a record whose cost group is one. Counted in `unpriced_rows`.
- **Summation never crosses a currency** (§4.8): one money figure per currency, exactly as `§9.4.5`'s
  `money` line already does.
- **The rounding rule is stated, not silent:** integer NanoUsd (ADR-006), floored **per row**; the residue
  is at most one nano per row and is never presented as a figure.
- **It is never added to a measured money line.** `Σcost.total` and `Σcost.input_miss` are the money the
  provider billed; this figure prices *tokens that were billed under another request's counters*. Adding
  them would double-count the bill — the same defect `§6`/`§9.2` already refuse for `switch_cost_nano`.

### 2.4 The label, and what the number must never be called

The figure is **`inferred`**. The rule it follows is `ADR-058`'s composition rule applied one step further:

> **A restatement of measured figures stays `measured`; a figure whose computation asserts a fact the
> provider never recorded is `inferred`.**
> `Σmiss` and `floor` are restatements (both are the provider's own counters, added and maximised). The
> re-bill asserts that these particular tokens were **billed a second time** — a claim no record carries,
> resting on a *model* of what a one-pass session would have paid (the floor). So it is `inferred`, and
> so is the money computed from it (`ADR-011` item 9 / `ADR-014` item 5's precedent, adopted unchanged).

**The number must not be called any of these, and the surface must print none of these words:**

| Never | Why | Where the word would come from |
|---|---|---|
| **`verified`** | `verified` is a difference between two worlds, minted only by a paired control (`ADR-059`) or the replay ladder (`ADR-031` D4.2). There is no second world here: the ledger holds one trace. | `ADR-057`'s pre-`ADR-058` draft, and the shipped header (§9 F1) |
| **a saving** — `tokens saved`, `avoidable`, `wasted`, `recoverable` | it is money already spent, and it is not recoverable by any action this surface can recommend. The ledger cannot see *why* the client re-sent: an edit, a compaction, a rewind, a cache expiry and a route move are one number here. | the temptation §7 and `§9.4.6` exist to refuse |
| **a measured difference** | its inputs are measured; the difference is not. `ADR-058` §2: a difference of two `measured` figures is not `verified` **by itself** — and a difference taken against a modelled floor is not `measured` either. | the audit's own phrasing, which this surface must not inherit |
| **`reprefill_tokens`** | that word is `ADR-014` item 5's: *the session's prefix token count* one account switch re-sends, estimated at decision time for a cache-namespace change, carried in `result.plan_switch` (and `§9.2`'s `switch re-prefill (inferred)` line). The ledger's figure is an aggregate over a session's **measured** miss. Two quantities, two names — **the opposite of drift**. | the vocabulary that already exists and must not be overloaded |
| **a cost the gateway charged** | the provider billed `Σcost.input_miss`; this is a decomposition of *that* paid volume, not a second bill. | `§9.2`'s money lines |

### 2.5 The four row shapes, and how a reader tells them apart

Rule: **a re-bill is a *row* fact (`rebill_tok > 0`), a break is a *block* fact (`at` present). They are
independent columns, and that independence is the diagnosis.** No sixth class word is added — `§9.4.2`'s
five words stay closed, because a cold re-prefill *is* a `rewrite` (the relation is the fact; the word is
its name) and `first` already names the session's own start.

| Row shape | `class` | `kept` | `kept_tok` | `at` | `rebill_tok` | why the arithmetic says so |
|---|---|---|---|---|---|---|
| **the session's own cold start** — nothing was ever sent before | `first` | `0` | `0` | `—` | **`0`** | `growth(1) = in(1)`, so `rebill(1) = −cached(1) → 0` (§2.2 consequence 1). Unavoidable, and priced nowhere. |
| **a same-session cold re-prefill** — the previous prompt was not extended, it was replaced | `rewrite` (or `repeat`) | `0` | `0` | `#0:<kind>` (on a `repeat`: `—`) | **≈ `in`** — the whole prompt | `peak(n−1) ≈ in(n)`, so `growth ≈ 0` and the miss is all re-bill. This is the audit's "15 cold re-prefills above 20 k". |
| **a mid-prefix break** — something early changed, the tail was re-sent | `rewrite` | `k > 0` | `Σ_{i<k} tokens > 0` | `#k:<kind>` | the tail above `growth` | `growth = in(n) − peak(n−1)`; the rest of the miss is the re-sent tail. This is the audit's 107 (97 `input_item`, 10 `tool`). |
| **no break, and a re-bill anyway** — the prefix did not change; the provider's cache did not cover it | `append` / `repeat` / `shorten` | full | `≈ in` | `—` | `> 0` | the block relation says the prompt grew (or repeated) exactly as before; the miss says the cache missed. The row's **flags** name the one in-record cause: `failover=<route>` or `plan=<from>→<to>` is a new upstream cache namespace (`ADR-011` item 9, `ADR-014` item 5). No flag ⇒ an expiry or an upstream instance the record cannot see, and the surface says nothing about it. |

`shorten` is **not** a break: it prints `at` (the first **dropped** block — there is no divergence) and its
keep is the whole new list, so its re-bill is normally `0`. The existing column keeps that behaviour
unchanged; this ADR only stops the reader from mistaking "the first dropped block" for "the block that
broke".

### 2.6 The summary block, in both forms

New `summary` member, `rebill` — nine keys, **every one printed in the text form too** (`§9.4.1`'s rule:
a field never appears in one form and not the other):

```json
"rebill": {
  "tokens": 84120,
  "money_nano": { "USD": 1261800 },
  "plan_tokens": 0,
  "rows": 3,
  "miss_tokens": 212000,
  "floor_tokens": 132000,
  "surplus_tokens": 4120,
  "excluded_rows": 0,
  "unpriced_rows": 0
}
```

| Key | What it is | Label |
|---|---|---|
| `tokens` | §2.2's definition — the number this ADR exists for | **inferred** |
| `money_nano` | one amount per currency, §2.3; `{}` when no currency receives one | **inferred** |
| `plan_tokens` | re-billed tokens whose row is on a `coding_plan` account — counted, never a zero in a currency line | **inferred** |
| `rows` | how many rows carry `rebill_tok > 0` | count |
| `miss_tokens` | `Σ (in − cached)` over the usage-carrying rows — the identity's first operand | measured |
| `floor_tokens` | `max(in)` over the same rows — the session's largest prompt, the one-pass floor | measured |
| `surplus_tokens` | `Σ surplus(n)` — the clamp's correction, §2.2 | **inferred** |
| `excluded_rows` | rows without usage, excluded from every figure above and **counted** (the `cache` ratio's `excluded` rule) | count |
| `unpriced_rows` | rows with re-billed tokens that no currency prices (§2.3) | count |

The text form prints three lines, in this order, after the existing `top steps` line:

```
re-billed        84120 tokens (inferred)  = Σ miss 212000 − floor 132000 + surplus 4120   (3 rows re-billed; 0 rows without usage)
re-billed money  1261800 nano USD (inferred; each row's own measured miss rate — no price table is read here)
re-billed plan   0 tokens (marginal 0 — allowance headroom, not a zero price)
```

The `re-billed plan` line is printed **only when `plan_tokens > 0`** (the `money` line's own `plan rows`
rule); the other two are always printed.

**Two groupings, both restatements of `rebill_tok`, both added because the audit's two findings are
groupings:**

| Line | Content | Rule |
|---|---|---|
| `by break kind` | for each of `prefix::BlockKind`'s four words — `system`, `tool`, `message`, `input_item`, always all four, `0` when never — the count of **`rewrite`** rows whose `at` names that kind, and the sum of their `rebill_tok` | it is the audit's *97 `input_item` / 10 `tool`* split, with tokens; a `rewrite` whose block carries no `kind` (never written by the trace) has `at: null` and lands in no bucket — an absence, never a guess |
| `top rebilled` | the five largest `rebill_tok`, as `#step (class, at, N tok)`, largest first, ties by step order; `—` when none, and never padded with zero rows | the answer to the surface's own question — *which step cost me the money* — where `top steps` answers *where did the prompt grow* |

The summary's `by class` block gains no column: every class figure is already a restatement of the rows
above, and a per-class re-bill is `by break kind`'s job by the more useful key (a break's cost is a
property of *which block* broke, not of which of `§9.4.2`'s five names the relation took).

### 2.7 Zeros, absences, `--last`, and a session that spans accounts

| Case | Contract |
|---|---|
| **a session with no break** (and no cache miss) | `rebill.tokens: 0`, `rows: 0`, `money_nano: {}`, `plan_tokens: 0`, `surplus_tokens: 0`, while `miss_tokens`/`floor_tokens` carry their measured values and `miss_tokens − floor_tokens == 0`. The text prints `re-billed 0 tokens (inferred; nothing was billed twice)`. **A zero, never `null`, never an absent key.** |
| **a session where no row carries usage** | `tokens: null`, `rows: 0`, `money_nano: {}`, `excluded_rows: steps`. Text: `re-billed — (no step carries usage)`. This is the one case where the key is null, and it is §7's absence rule, not a zero — the same distinction `§9.4.3` makes for `in`/`cached`/`out`/`money`. |
| **some rows without usage** | they are excluded from every figure and **counted** in `excluded_rows`; the number stands on the rows that measured. |
| **`--last <n>`** | no `rebill` figure moves: the block is computed over the session's steps, exactly as `steps`, `money` and `by class` are (§9.4.1's `--last` row). A `showing` member is present exactly as it already is. |
| **a session across two routes, accounts or currencies** | tokens are account-agnostic and not split; money is per currency and never summed across; plan rows are counted in `plan_tokens`; `unpriced_rows` states what no currency priced. A per-route split of the token figure is **refused**: the floor is a session fact, and splitting it would need an allocation rule no measurement supports. |
| **a reused session key** | `§9.4.5`'s existing rule — no boundary is invented, so a session key reused after a day reports one figure with a large `gap_ms` in the middle as the reader's evidence. |

### 2.8 What does not move

No new command, flag, exit code or record field. No change to `§9.4.2`'s five class words, `§9.4.4`'s flag
vocabulary, `§9.4.5`'s existing lines, the three-bucket partition (`ADR-056` §2.4), the money rules, the
`plan` label, or `§9.4.6`'s refusals (which only gain §2.4's never-list). No `schema_version` move, no
metric, no store read, no event-log read, no upstream call, no config key, no `vadis setup` row, no price,
no `CONF` id.

---

## 3. Blast radius, in both directions

**Gained.** The two questions the audit answered with an out-of-tree script become two fields: *which
block broke this session's prefix* (`at`, already there, now with the weight of what survived and what was
billed again beside it) and *how much of this session's miss was re-prefill* (`rebill.tokens`, with its
operands, its per-currency price and its label). Also gained, on the same two columns: a `repeat` row that
re-billed because the cache expired is now **visible as a re-bill with no break** — a failure mode no
existing surface can show, and one that `class == repeat` alone reads as "free".

**Paid.** Two columns on a seventeen-column row (the text table gets wider), one nine-key summary object,
two groupings, and **five wording sites in the shipped command plus one unit-test assertion** that this
ADR freezes and its implementing card moves (§5). Nothing is paid in the serving path: the surface is a
reader, and the arithmetic is over records already loaded.

**Unchanged, deliberately.** Every forwarded byte; every trace field; the five class words; the flag
vocabulary; the three buckets; `stats`, `/metrics`, `/health`, `/state/events`; every gate, threshold,
corpus and L1-envelope value.

---

## 4. The refusals this ADR adds

`§9.4.6` gains one bullet, and it is the never-list of §2.4 in the section's own voice:

> - A **`verified`** figure, a **saving**, a "tokens re-savable", or any reading of `rebill` as money that
>   could have been avoided: the re-bill is a decomposition of miss volume already paid, computed against a
>   modelled one-pass floor with no control, and it is `inferred` (§9.4.8). Nor may it be called
>   `reprefill_tokens` — that is `result.plan_switch`'s decision-time estimate for a cache-namespace change,
>   a different quantity.

And, from §2.5, one sentence stating what the surface will not attempt: **it does not decide whether a break
was justified.** A client that edited a message and a client whose cache expired produce the same
`rebill_tok`; the difference is not on the wire and the ledger does not invent it.

---

## 5. The amendment list (frozen the moment this ADR is accepted)

Two groups. **Group A is the shipped command's wording** — five sites plus one assertion, all in
`crates/vadis-cli/src/steps.rs`, all one word. `ADR-058` ruled the word on 2026-10-18 and the correction
reached spec §9.4.3 and `ADR-057`; it did not reach the implementation, which landed afterwards. **The
implementing card of this ADR moves them in the same change that adds §2's figures** — a surface may not
print `verified` on one line and `inferred` on the next. Until that card lands, the command's printed
header contradicts spec §9.4.3 and the book, and that window is registered (F1's row, §9).

| # | Site | Edit | Why |
|---|---|---|---|
| **A1** | `steps.rs:665` (the text header's third line) | `in/cached/out and the block facts are verified` → `… are measured`; `class names the verified block relation` → `… the measured block relation`; and the line gains one clause: `kept_tok and rebill_tok are inferred (a proportional attribution, and a miss volume above the session's one-pass floor)` | the header is printed on every run; it is the surface's own statement of its labels |
| **A2** | `steps.rs:791` (the `cache` summary line) | `… input tokens, verified — the provider's own counters …` → `… input tokens, measured — …` | a ratio of one world's counters is `measured` (§9.4.5) |
| **A3** | `steps.rs:23-27` (the module doc), `:74`, `:77` (the `blocks`/`kept` field docs) | `**verified**` → `**measured**` at each | the code's own docs are the next reader's first source |
| **A4** | `steps.rs:1118` (`assert!(text.contains("class names the verified block relation"))`) | the assertion moves with A1's new sentence — **tightened, never loosened**: it must still contain the class-relation caveat, with the correct word | the wrong word is currently *pinned by a test*; a fix that deletes the assertion instead of moving it would lose the pin |

**Group B is what the same card adds** (the implementation of §2, listed so nothing is improvised):

| # | Site | Edit |
|---|---|---|
| **B1** | `StepRow` + `reduce` | the two fields, §2.2's arithmetic (`peak` as a running maximum over usage-carrying rows, `surplus`, the clamp), and `kept_tok` from the block list |
| **B2** | `Report` | `rebill`'s nine figures, `by_break_kind`, `top_rebilled` |
| **B3** | `render_text` | §2.6's three lines and the two groupings, §2.5's shapes legible in the existing columns |
| **B4** | `report_json` | the `rebill` object and the two groupings, same values under the same names |
| **B5** | the module's unit tests | §2.2's three consequences as assertions: `rebill(first) == 0` on a grown session; a mid-session cold re-prefill reports ≈ `in`; `rebilled == miss − floor + surplus` on a session with a clamped row; a no-break session reports exactly `0` |

**No conformance case is allocated by this ADR, and no `CONF` id is spent.** When one is authored it takes
the next free id — `DESIGN` §12.8's heading reads `CONF-01…CONF-120`, so the first free is **`CONF-121`** —
and it must be authored over a trace a real `serve` wrote (`ADR-057` §4's rig rule), not a hand-written
fixture. **No gate definition, threshold, corpus or L1-envelope value moves** (AGENTS constraint 9).

---

## 6. Landing plan

| Change | Files | Content |
|---|---|---|
| **docs (this card)** | `docs/spec.md` (§9.4.2, §9.4.3, §9.4.5, §9.4.6, new §9.4.8), `design/DESIGN.md` (§12.9's register), `book/observability-and-accounting.md` | §2 of this ADR, in the contract's own voice; §9's F1's book half |
| **code (its own card)** | `crates/vadis-cli/src/steps.rs` only | §5's groups A and B. The reducer stays a pure function `[DecisionRecord] → Report`: **no clock, no RNG, no environment** (constraint 2), one pass, and the session held in memory as it already is |

**Where the arithmetic lives.** In the CLI, beside its only consumer, exactly as `ADR-057` §6.1 ruled for
the classification: `vadis-core` gains nothing, because a core home would invite a `DecisionRecord` field —
a schema change with a version question. `--json` remains the extension point.

**Docs before code** (constraint 8): the spec section and the book paragraph land in this card, before the
command prints anything new. The book takes the re-bill paragraph and the two `measured` corrections, and
**no price number** (single-source price policy): it says *"at your config's own miss price"* and links
§9.4.8.

---

## 7. Alternatives considered

| Alternative | Why rejected |
|---|---|
| **the audit's own shape verbatim**: `rebilled = Σmiss − session-final context` | wrong as a *floor*. A session that shortens after its peak already paid for that content; a final-context floor books the compaction's replacement text as a re-bill (worked example in §2.2 consequence 3). The peak coincides with the final context on the append-only session the audit measured, so nothing about the audit's finding is weakened — the definition is simply right where the audit's was silent. |
| a **sixth class word** (`cold`, `coldstart`) | the class names a *relation* (`§9.4.2`), and a same-session cold re-prefill's relation is `rewrite` with `lcp == 0` — a word already covers it, `kept 0` + `#0:<kind>` already prove it, and a sixth word would be the vocabulary drift `§9.4.4` refuses for `retry`. §2.5 answers rule 3 with the columns that exist. |
| one **`miss`** column and let the reader subtract | the audit's arithmetic was done by hand *because* it had to be; and a number with no field, no label and no named operands is the "script in dotfiles" `ADR-057` §1.4 refused. The subtraction is printed, not implied. |
| price the re-bill from the **config's price table** (`p_miss(route) × tokens`, `ADR-014` item 5's formula) | it needs a band selection, a peak decision and a currency guess that the record has **already made** — and re-making them is the cost engine's second implementation, which the card forbids and `ADR-041` refuses on principle (one definition per figure). `cost.input_miss ÷ miss` *is* that price, as applied. The precedent's *label* is adopted; its *arithmetic* is not needed because this surface has the money already. |
| an `inferred` **money-only** figure (no tokens) | it hides the unit the audit reasoned in, and a lone nano figure cannot be checked against anything a reader can see. Tokens first, money from them, operands printed. |
| call it **`reprefill_tokens`** and reuse `ADR-014`'s word | it already means *the session's prefix token count one switch re-sends* — a decision-time estimate for a namespace change. That name on a measured-miss aggregate would make the trace's own field and this summary read as one quantity. §2.4 refuses it by name. |
| label it **`measured`** ("its inputs are measured") | `ADR-058` §2's composition rule is the answer: a difference of `measured` figures is not `verified`, and a difference against a modelled floor is not `measured` — the assertion that these tokens were billed twice is not in any record. |
| fold the figure into **`stats`**'s window report | `stats` sums a window and must not carry a per-session decomposition; `ADR-057` §2.1 already refused that merge for the ledger, and this adds no reason to reopen it. |
| **no row columns**, summary only | the audit's per-step question (*which of the 107 breaks cost what*) would need a second pass over the trace by hand — the exact work this surface exists to delete. |

---

## 8. Register

### 8.1 IDs spent

| ID | What it is |
|---|---|
| **`ADR-062`** | this file |
| **spec `§9.4.8`** | the re-billed prefix's contract (definition, price source, label, never-list, the four row shapes, the summary block) |
| **spec `§9.4.2`, `§9.4.3`, `§9.4.5`, `§9.4.6`** | amended in place (one sentence; two appended columns; three lines plus two groupings; one refusal bullet) |
| **`CONF`** | **none** — this ADR allocates no case (the owner's card: *no conformance case*). The first free id stays `CONF-121` for the card that writes one. |
| **no price, no gate, no threshold, no corpus, no envelope value** | constraint 9 |

### 8.2 What this ADR adds to the documents

- `docs/spec.md`: **§9.4.8** (new), and §9.4.2 / §9.4.3 / §9.4.5 / §9.4.6 amended.
- `design/DESIGN.md`: one **register paragraph** in §12.9.
- `book/observability-and-accounting.md`: the `steps` section gains the re-bill paragraph; two sentences
  that print `verified` for a single observation are corrected to `measured` (F1's book half).
- **Nothing else.** No `crates/`, no `tests/`, no `config.example.yaml`, no `README.md`, no `rules/`.

---

## 9. Findings this ADR registers

| # | Class | Site | Effect | Owner | Due |
|---|---|---|---|---|---|
| **F1** | **wrong-label, served** | `crates/vadis-cli/src/steps.rs:665` (the text header, printed on every run), `:791` (the `cache` summary line), `:23-27`/`:74`/`:77` (the module and field docs), and `:1118` (a unit test that **asserts** the wrong word); `book/observability-and-accounting.md:186` (`a name over a verified relation`) and `:204` (`cached` … `(**verified**)`) | the shipped ledger calls single observations `verified` — the exact inference `ADR-058` ruled against and AGENTS constraint 4 forbids — while spec §9.4.3 has said `measured` since 2026-10-18. The correction landed in `ADR-057` and the spec but not in the implementation, which was written afterwards; the book then copied the implementation. **The book is corrected here** (§6); the code's five sites and the assertion are frozen in §5 A1–A4 for this ADR's implementing card, and the window is stated so nobody reads the header as the contract. | this ADR's implementing card | with the surface |
| **F2** | **stale-contract** | spec **§9.4.2** and `ADR-057` **§2.2**/§7: *"the block index is vadis's own enumeration order of the client's bytes, **not** the provider's template order"*, with a *"factor of four"* disagreement quoted as the reason the cache column is not block-derived | **the ordering claim is false and has been since 2026-09-20**: `extract_prefix_blocks` enumerates in **template** order (system → `tools` → `messages`/`input` items), pinned by **`CONF-31`** (`tests/conformance/tests/conf_31_prefix_template_order.rs`) and stated correctly in spec **§6**. So `at #7:input_item` is a template-order ordinal — the same order the provider's prompt uses — and the sentence a reader is pointed at says the opposite. The *rule* it supports survives (a block ordinal is not the byte offset a provider's cache keys on), but the sentence and the quoted 4× figure do not: the 4× was measured under the **pre-Plan-A byte-order** enumeration and is not a current figure. **spec §9.4.2 is corrected in place by this ADR**; `ADR-057` is append-only and keeps its text, which is why this row exists. The correction moves no class, no column, no count and no case (`CONF-31` already pins the true order). | this ADR (spec half done here) | done |
| **F3** | **registered-limit** | `rebill.tokens`'s definition (§2.2) | the number is exact arithmetic over measured counters and a modelled floor, but it **cannot separate** re-sent content from content a compaction replaced with new text: a session that shortens and re-summarises contributes its summary's miss to the figure. `inferred` and §2.4's never-list are what keep that from being read as waste. Not repairable without content, which this gateway does not record (`ADR-005`). | — (the label is the mitigation) | by design |

---

## 10. What a future reader must not conclude

- **Not** *"`rebill.tokens` is money vadis could have saved."* It is a decomposition of miss volume the
  provider already billed, labelled `inferred`, priced at each row's own measured miss rate, and it is
  never added to any measured money line.
- **Not** *"the re-bill is `verified`, so it is stronger than the measured cost beside it."* Backwards: the
  measured columns are one observation each; the re-bill is the derived one, and `verified` appears nowhere
  on this surface (`ADR-058`, `ADR-059`).
- **Not** *"a break was the client's fault."* The ledger names the block that diverged and the tokens that
  were billed again; it does not know whether the client edited, rewound or compacted, and it prints no
  word for intent (`ADR-057` §2.2's rule, unchanged).
- **Not** *"a `repeat` row is free."* A repeat whose `cached` is 0 re-billed the whole prompt — a cache
  expiry or a route move — and that is now a visible row shape (§2.5's fourth).
- **Not** *"the session's first request is a re-prefill."* It cannot be: `rebill(1) = 0` by construction,
  and the cold start is priced nowhere on this surface.
- **Not** *"the block index is a body byte offset."* It is a template-order ordinal (F2), which is why the
  cache column is still the provider's own counter.
- **Not** *"this ADR changed the ledger's class words or its five buckets."* No class, no flag, no bucket,
  no money rule and no exit code moved.
