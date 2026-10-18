# ADR-058 — `measured` is not `verified`: one word per scope, so that no single observation can be read as a saving

- Status: **accepted (2026-10-18), text only.** It fixes a vocabulary that three frozen documents use for two
  different quantities, and it amends the sentences that use it. It writes **no product byte**, allocates **no
  `CONF` id** and moves **no gate, threshold, corpus or L1 envelope** (AGENTS constraint 9). The one thing it
  cannot do — relabel the *served* report, whose printed word is `verified` — is named as owed work in §5 and
  in the register of `design/DESIGN.md` §12.9.
- Kind: **a vocabulary ruling across `docs/spec.md` §6/§7/§9.2/§9.4, `ADR-031` D4, `ADR-057` §1–§2, and
  `book/`.** Nothing here is a new surface, a new field or a new metric; every edited sentence already existed
  and now says which of two scopes it meant.
- Authority: the owner's card of 2026-10-18 (*"one word, one meaning"*), which named the collision, offered
  two rulings and required a checkable inventory. This ADR takes the recommended one (§2) and records what
  the other would have cost (§3).
- Amends: `docs/spec.md` §7 (the definition table and one scope paragraph), §6 (four sentences), §9.2 (one
  note under the provenance table), §9.4 (six label cells and one sentence); `design/decisions/ADR-031-*.md`
  (an appended scope note; D4's body is not rewritten); `design/decisions/ADR-057-the-step-ledger.md` (§1.2,
  §1.4, §2.2, §2.3, §2.4, §2.5, §7 — **in place**, because that ADR was never ruled, and its own status block
  now says so); `design/DESIGN.md` (§12.9's register and P8's row); `book/observability-and-accounting.md`
  (the label table, its heading and five sentences), `book/faq.md`, `book/introduction.md`,
  `book/cost-and-caching.md`.
- Related: `AGENTS.md` constraint 4 (**read, never reworded** — §5), constraint 8 (the book moves with the
  ruling); **ADR-031** D4 (the stricter half of the collision), **ADR-057** (the defective half), **ADR-041**
  (one definition per figure, the argument this ADR repeats for one word), **GAP-Q14** (block tokens are
  `inferred`), **ADR-019** (the transform ledger's `net = saved − added`).

---

## 1. The collision, quoted

| Document | What it says | The scope it actually means |
|---|---|---|
| `docs/spec.md` §7 | `verified` = *"the **measured delta** from upstream `usage`: a control turn with the transform on/off within the same session, or an attributable change in `cached_tokens`"*, and **only it** may enter a gate or be reported externally | a **difference between two worlds** |
| `ADR-031` D4.2 | *"`verified` **is not mintable by an analysis**. Only the replay harness's ladder mints it"* | the same, stricter |
| `ADR-057` §2.5 | `verified` = *"a figure the provider itself counted, or a fact of the bytes (`usage.*`, block hashes, `lcp`)"* | a **single observation**, no counterfactual |

The third is the defect, and it is not a wording slip: a per-step ledger has no counterfactual, so it cannot
mint a delta. Calling its counters `verified` invites exactly the inference constraint 4 forbids — *"this step
saved N tokens because `cached` is high"* — and the first round that wants a headline number has only to read a
ledger row and print it. The same defect is already on the served surface, unnoticed because no two documents
sat side by side: `vadis stats` prints `cost (verified, USD)` and `/metrics` emits
`provenance="verified"` for the cost tiers, the two cache counters, the hit rate and the switch cost
(`crates/vadis-cli/src/metrics.rs:176,189,196,207,286`) — **and `provenance="measured"` for `overhead p99`
(`:360`)**. Two single observations, two different words, one surface, no counterfactual in sight.

## 2. The ruling: two scopes, and the vocabulary closed at five words

### 2.1 One word, one meaning

| Word | What it names | May it be reported as a saving? | Where it is printed |
|---|---|---|---|
| `measured` | **one observation**: a number the provider itself reported for this request (`usage.*`), a fact of the bytes vadis forwarded (block hashes, `lcp`, `ts`, byte lengths), or the price arithmetic over those two (`cost.*`, a hit ratio) — every input reported or published, none estimated | **never** | `vadis stats`, `/metrics`, a ledger row, the book |
| `verified` | **the delta between two worlds**: the same content with the transform on and off inside one session, or the replay ladder recomputing the counterfactual with the same code (`ADR-031` D4.2) | **only it**, and only with the pair's provenance attached | the savings line; nothing else |
| `inferred` | a local estimate with no control | never | unchanged (`spec §7`) |
| `count` / `derived` | a count, or a name derived from other fields (`step`, `class`, `flags`) | never (not a quantity) | `vadis stats`, §9.4 |

The rule a reviewer can grep for, in both directions:

> **`measured` is one observation, never a saving; `verified` is the pair, and nothing else is a saving.**

### 2.2 The composition rule — why `verified ≠ measured − measured`

A delta between two observations is `verified` **only when the pair is a designed counterfactual**: the same
content, one treatment removed, the two readings attributable to that removal. Two adjacent requests of a
session are two observations, and their difference confounds the content change with the treatment; a replay
that recomputes the counterfactual with the same code is a pair, and a control turn is a pair. Without that
rule the split would be cosmetic: it would relabel the ledger and leave "measured minus measured" free to be
presented as `verified` by the first total that wants one.

### 2.3 What each document now says, and how to check it

| Document | The sentence the ruling added | Check |
|---|---|---|
| `docs/spec.md` §7 | the three-row table, plus *"**`measured` is one observation, never a saving; `verified` is the pair, and nothing else is a saving**"* and the composition rule under it | `grep -n 'measured. is one observation' docs/spec.md` |
| `ADR-031` D4 | an appended **Scope note (ADR-058, 2026-10-18)**: D4's `verified` is the delta scope, unchanged and strengthened; D4.1's *"no measured evidence"* means no `verified` delta, because one `measured` observation is not evidence for a lever | `grep -n 'Scope note (ADR-058' design/decisions/ADR-031-*.md` |
| `ADR-057` §2.3/§2.5 | the counter columns are `measured`; the surface prints **no** `verified` figure, and says so | `grep -n 'no .verified. figure on this surface' design/decisions/ADR-057-*.md` |
| `docs/spec.md` §9.4 | the same six cells, relabelled in the *contracted, not served* text | `grep -n 'measured' docs/spec.md` at §9.4.3 |
| `docs/spec.md` §9.2 | a note under the provenance table naming the three legacy rows and the reading rule | `grep -n 'legacy word' docs/spec.md` |
| `design/DESIGN.md` §12.9 | the dated register line | `grep -n 'ADR-058' design/DESIGN.md` |
| `book/observability-and-accounting.md` | the label table gains `measured` as its first row; the worked paragraphs say *becomes `measured`*, not *becomes verified* | `grep -n 'measured' book/observability-and-accounting.md` |

## 3. What a reader loses by the other option

The alternative was **one word, one scope**: keep `verified` for deltas only, and make `ADR-057` say its
columns are *unlabelled observations* with an explicit sentence that nothing on that surface is a saving.

It is cheaper by exactly one code change, and it pays three times over:

1. **The word would be missing where the product needs it.** `/metrics` already publishes
   `provenance="measured"` for `overhead p99`. Option 2 either leaves that value meaning what `verified` means
   everywhere else — two words for one quantity, the collision mirrored — or it removes a value that consumers
   can already select, which is a served-contract change and therefore *more* work, not less.
2. **A prohibition is weaker than a vocabulary.** *"No figure here is a saving"* is a sentence a reader must
   remember while reading a column; `measured` is a word that says it at every row, and a reviewer can grep
   for a mislabelled figure. Abstractions that depend on being remembered are the ones that fail first.
3. **The distinction the ledger needs has no name.** A ledger prints the provider's count, a local estimate
   and a difference in the same table. Option 2 gives two of those three a word and calls the third an
   "unlabelled observation" — which is a name no surface can print and no test can assert.

What option 1 costs, stated plainly: one more word in the book and in §7, a rename inside a *proposed* ADR,
and — because `vadis stats` prints its label as text and `/metrics` publishes it as a label value — a
**deferred** code relabel (§5) during which `spec §9.2`'s printed word is legacy and says so.

## 4. The inventory — every label occurrence, and what happened to it

The census has three numbers, because the word has two lives. Measured over
`docs/spec.md design/DESIGN.md design/decisions/ADR-031*.md design/decisions/ADR-057*.md AGENTS.md book/`,
at the parent commit `fdc24fe` and at this change:

| Census | Before | After | What it counts |
|---|---|---|---|
| `grep -rn 'verified'` (lines) | **123** | **131** | every occurrence: ordinary English, mentions and applications alike |
| `` grep -ro '\*\*verified\*\*' `` (**label cells** — the word applied to a figure, in a table) | **20** | **4** | **the defect's own count**: the three §9.2 legacy rows plus the one delta metric |
| `` grep -ro '`verified`\|\*\*verified\*\*' `` (occurrences / lines) | 71 / 69 | 79 / 76 | the word *as a term*: applications **and** the new definitional sentences that name it |

The applied count is the one that says whether the defect is gone: **20 → 4**, and the four are exactly the
deferred rows below plus `verified savings tokens` (the one figure the word is for). The term count rises for
the obvious reason and it is not a relapse: the ruling has to name the word it governs, so `ADR-031`'s scope
note, `ADR-057`'s status correction, `spec §7`'s scope sentence and `DESIGN §12.9`'s register line all use it
in a sentence about the word rather than as a label on a figure. `ADR-058` itself is outside the glob above
and uses it on nearly every page, on purpose.

The other distinction is a rule, not a convenience: **the word is governed where it is a label** (backticked
or in a label cell). Ordinary English — *"verified against codex-cli 0.137.0"*, *"smoke-verified"*, *"three
facts, each verified"*, *"unverified"* — names a check somebody performed, not an accounting scope, and is left
alone; rewriting it would be a second, larger change with no contract behind it.

| # | Site | Use before | Disposition |
|---|---|---|---|
| 1 | `docs/spec.md` §7 table + prose | the definition itself | **amended** — the three-row table, the scope sentence, the composition rule |
| 2 | `docs/spec.md` §6 `cache_hit_rate` ("is therefore **verified**") | hit rate | **amended** → `measured` |
| 3 | `docs/spec.md` §6 switch cost ("become `verified` through … measured usage") | a cost | **amended** → `measured`; the in-plan counterfactual sentence keeps its arithmetic and loses the word |
| 4 | `docs/spec.md` §6 "a gate or an external saving claim adjudicates on the **verified** number only" | the delta rule | **amended** — now says *the `verified` delta*, and that a `measured` figure is never substituted for one |
| 5 | `docs/spec.md` §6 `verified_savings_tokens` (the §6 metric) | a delta metric | **unchanged** — it is a real `verified` metric, and it is 0 until a pair exists |
| 6 | `docs/spec.md` §9.2 provenance table: `cost`, `hit rate`, `switch cost` rows and the printed sample lines | three measured figures under the legacy word | **annotated, not relabelled** — §5's deferred card; a note under the table gives the reading rule |
| 7 | `docs/spec.md` §4.16 `provenance` value list (`verified`, `inferred`, `measured`, `count`) | the value set | **unchanged** — after this ADR the set is correct as written, and one member stops being a coincidence |
| 8 | `docs/spec.md` §9.4.2 ("a name over a **verified** relation"), §9.4.3 columns 5–7, 9–11 | six observations and one name | **amended** → `measured`; §9.4 was *contracted, not served*, so nothing printed moves |
| 9 | `design/decisions/ADR-031` D4 and D4.1/D4.2/D4.3 | the delta scope, correctly | **appended scope note only** (§2.3); D4's text is untouched (append-only) |
| 10 | `design/decisions/ADR-057` §1.2, §1.4, §2.2, §2.3, §2.4, §2.5, §7 alternatives | seven places that read the ledger's counters as `verified` | **renamed in place** → `measured`, with the status block saying the draft was never ruled at the moment of the correction |
| 11 | `design/DESIGN.md` P8's row ("every figure carries `verified`/`inferred`") | the label set | **amended** — the set is three words, and the row says which one a saving may rest on |
| 12 | `design/DESIGN.md` §12.9 | the register | **appended** — this ADR's dated line, naming the deferred card |
| 13 | `design/DESIGN.md` elsewhere (its remaining `verified` occurrences, e.g. §12.4, §12.10.6, §12.11, §12.22's prohibition lists) | rules of the form *"gates read `verified` only"*, *"no `verified` figure on this path"* | **unchanged** — every one of them states a rule about the **saving** scope, which this ADR does not move; §2.1 restates it rather than reversing it |
| 14 | `book/observability-and-accounting.md` | the label table, its heading, and five sentences | **amended** — `measured` is the table's first row; the *becomes verified* sentence becomes *becomes `measured`* |
| 15 | `book/faq.md`, `book/introduction.md` (*"a measured (`verified`) difference"*) | the collision, in the reader's words | **amended** — the two words are now two different things, and the sentence says so |
| 16 | `book/cost-and-caching.md` | "becomes *verified* in the shadow of …", "their verified cost" | **amended** → `measured` |
| 17 | `book/connecting-clients.md`, `book/getting-started.md`, `book/roadmap.md` | ordinary English (*verified against codex-cli 0.137.0*, *smoke-verified*, *verified cost and latency* as a quality bar) | **unchanged** — not the label |
| 18 | `AGENTS.md` constraint 4 | the constitution | **untouched** — see §5; it needs no change |

## 5. What this ADR does not decide, and what it owes

- **Constraint 4 is read, not rewritten.** *"Every transform reports its token delta and labels it `verified`
  (measured usage difference) or `inferred` (local estimate). Only `verified` numbers may be counted as
  savings."* The parenthetical *is* the delta scope; `measured` names the single observation that the
  difference is taken **over**, so the constitution says exactly what this ADR says and the card's stop
  condition (a ruling that would need constraint 4 changed) does not fire.
- **The served word is deferred, deliberately.** `vadis stats` prints `cost (verified, <currency>)` and
  `switch cost (verified, <currency>)`, and `/metrics` publishes `provenance="verified"` from six call sites,
  all of them but one (`:246`, the savings series) `measured` figures. A label that a consumer can select and a
  report prints is a served contract (`spec §9.2`, §4.16) and its change needs a code change plus whatever
  `CONF-87` asserts — so it is a **follow-up card**, not this text-only change. Until it lands, `spec §9.2`
  carries the reading rule in-band.
- **Update (2026-10-18, later the same day) — the deferred item landed, by owner ruling.** The follow-up card
  found `CONF-87` pinning the literal `"verified"` at fourteen sites (the `expected_provenance` map plus
  eleven `value_of` assertions), so per this ADR's own stop condition it stopped and reported. The owner
  authorized the amendment narrowly: the five measured series → `"measured"` at exactly those fourteen sites,
  the savings arm (`:90`, `:1246`) keeps `"verified"`, the three `--json` `"label"` sites that rode along
  (`stats.rs:655/:664/:922`) move too and are pinned in the same commit, field *names*
  (`switch_cost_verified_nano`, `verified_savings_tokens`) stay — identifiers, not printed words — and no
  predicate, series, threshold or N-vs-10N arm is touched. The case was not weakened: every sample still
  demands exactly one provenance label and the delta series is still distinguished.
- **No code identifier is renamed.** `verified_savings_tokens` (`crates/vadis-core/src/trace.rs:437`) keeps its
  name: it counts deltas, which is what the word means.
- **Nothing else moves.** No gate definition, threshold, corpus, L1 envelope or conformance case; no trace
  field, no `schema_version`, no config key; no product byte.

## 6. What a future reader must not conclude

- **Not** *"`measured` is a weaker `verified`"* — it is a different quantity: an observation, not a difference.
  A `measured` figure is the *input* to a delta and is never the delta.
- **Not** *"two measured numbers make a verified one"* — §2.2: the pair has to be designed.
- **Not** *"the ledger got quieter"* — nothing was removed; three counters kept their values and gained the
  word that says what they are.
- **Not** *"cost is unlabelled now"* — cost is `measured`, which is a label, and §9.2's legacy printed word is
  the only place still saying otherwise.
- **Not** *"ADR-057 was amended against a ruling"* — it was **proposed**, and its status block records both the
  correction and the fact that it had not been ruled when it was made.

## 7. Register

| ID | What it is |
|---|---|
| **`ADR-058`** | this file |
| — | **no `CONF` id, no `GAP-Q` id, no round number**: the census is a grep, not a gate, and the served relabel is the follow-up card's to witness |
| **owed** | the served relabel: `crates/vadis-cli/src/metrics.rs:176,189,196,207,286` → `provenance="measured"` (the delta at `:246` stays `verified`), `spec §9.2`'s three table rows and its two sample lines, `spec §4.16`'s wording, and whatever `CONF-87` asserts about the value set — one card, one case amendment, no gate |
| **paid** | the §5 owed item, landed 2026-10-18 by the owner ruling recorded in §5's update bullet: relabel + the fourteen-site `CONF-87` amendment (not weakened — one label per sample, delta series still distinguished) + the three `--json` label sites, pinned; `spec` §4.6/§9.2/§4.16, `DESIGN` §12.9/§12.10 and the book moved in the same change |
