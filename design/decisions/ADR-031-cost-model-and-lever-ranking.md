# ADR-031 — the cost model and the levers ranked by $: the formula, the denominator, the labels and what a ranking may never claim

- Status: accepted
- Date: 2026-09-24
- Related: AGENTS constraints 2 (content determinism), 4 (**no unverified savings**: `verified` measured /
  `inferred` local estimate, gates count only `verified`), 5 (**no fabricated prices**: an official page +
  date, never an estimate), 8 (docs before code; the book links to the authority) and 9 (the measurement is
  outside the search space); **ADR-018** (one amount, one currency, no rate, no cross-currency add — the
  `Money` type and the per-currency map); **ADR-014** (the plan/metered pairing and the spill);
  **ADR-026** (corpus tiers and automated scoring — the `needs[]`/`coverage_gap[]` vocabulary a direction
  rationale cites); **ADR-027** (per-arm plans and the two comparison rules); **ADR-029** (the scale
  baseline: *the method is the loop's, the threshold is not*); **ADR-030** (the transform latency method:
  the same shape of boundary — a method frozen by the loop, numbers never in a contract file);
  ADR-012 (the gates, the corpus, the conformance assertions and the L1 envelope are outside the loop's
  mutable scope); ADR-019 (the transform mode and the ledger); spec §2/§4.0/§4.4/§4.6/§4.8/§4.10
  (the config schema, prices, tiers, peak windows, `plan_policy`), §6 (the trace contract: `usage`, `cost`),
  §7 (the labels), §9 (the report); DESIGN §5/§12.4 (the cost engine), §12.8 (the conformance allocation),
  §12.16 (the baseline's quantity), §12.17 (**this method's product-side pointer**);
  `autowork/program.md:34-42` (the gate table: the **Cost** gate reads the fixed-trace replay's `verified`
  figure; **D3** is the P1 lever) and `:58-96` (steps 0/0.5); `autowork/STATE.md:846` (waiting-on-human
  **row 1**, still open — no threshold is written here).

## Background

**The repository has four rounds of measurement and no cost figure.** R30 made the live-path byte audits
runnable; R31 fixed the recorder and ran the first post-R24 live pair; R32 measured the router's own
latency under scale; R33 measured the transform path's latency. None of them produced a token or dollar
delta: R33's own record says it in one line — *"this round measured **latency**, not tokens; the token
ledger rows are `inferred`"*. `program.md:40`'s Cost gate reads *"the `verified` $ and token ledger of a
fixed-trace replay"*, `autowork/STATE.md` states that **no `verified` figure has ever been produced**, and
the last live run's own row says why it could not be: `verified_ineligible_reason: condition-4
instrument-not-usable (dirty harness tree)`.

**The loop's next move is a choice between levers, and the choice is the expensive part.** R35+ may spend
real money on a lever, so the loop must decide **which** lever to buy first, and it must do that from
arithmetic rather than intuition. Three things were missing and each is structural, not cosmetic:

1. **No single arithmetic.** The product computes money in exactly one place
   (`crates/router-core/src/cost.rs:279-300`: five tiers, integer `Nano`, one floor, the peak multiplier on
   the sum), and an *analysis* has no frozen statement that it uses that arithmetic rather than a
   plausible-looking re-derivation. A re-derived money line drifts from the trace it claims to explain —
   and a drift is indistinguishable from a saving when both are small.
2. **No denominator.** "A lever saves money" is not a comparable claim until the unit and the base are
   fixed: per request or per token, on which corpus, at which rate. A lever that changes one request in a
   thousand and a lever that changes every request look identical without it.
3. **No label discipline for a ranking.** AGENTS 4 already forbids presenting an inferred number as
   measured; a table is exactly the artifact where that discipline is easiest to lose (an `inferred` row
   summed with a `verified` one, a rate quoted from the wrong window, a price copied without its source).

**The window is a live, measured trap.** The config's own price entry carries a peak multiplier and two
UTC windows, and the trace records which one applied: the committed R31 live row carries
`peak_applied_pct: 200` because it ran Wednesday 06:34Z — inside `06:00–10:00` — while the R23 rows carry
`100` at 13:00Z. The same usage is therefore quoted two different ways depending on which rate is named,
and only one of them is the spend. This is `R31-3-F2`, and it is the reason this ADR makes the *quoting*
rule part of the method rather than a house style.

## Decision

### D1. The ranking's money is the product's own cost function — adopted, not re-derived

A money line is computed by exactly the arithmetic of `crates/router-core/src/cost.rs:279-300`:

```
uncached = usage.input_total − usage.input_cached
miss     = floor(uncached × price.input_miss / 1000)
hit      = floor(usage.input_cached × price.input_hit / 1000)
write    = floor(usage.cache_write × price.cache_write / 1000)
out      = floor((usage.output + usage.reasoning) × price.output / 1000)
base     = miss + hit + write + out                       # saturating integer adds
total    = floor(base × peak_applied_pct / 100)           # peak_applied_pct from price.peak.multiplier_at(row.ts)
```

- the unit is **`Nano(u64)`** (integer nano-currency) and `Price` is **nano-currency per 1K tokens**;
- there is **exactly one rounding point** — a floor at the `/1000` step;
- **reasoning tokens are billed at the output tier**;
- the four tier fields are **pre-peak** values; the multiplier lands on **`total`** only;
- the currency is **copied from the price table**, never converted.

**Alternatives considered.** (a) A harness-side token arithmetic in bytes/1M — rejected: a second
implementation of an invariant (ADR-016 §13.3's one-owner rule) that would drift from the trace it
explains. (b) Rounding each tier and then the total — rejected: §2's exact match against two committed
rows is lost. (c) Applying the peak multiplier per tier — rejected: contradicts `cost.rs:260-261` and
inflates the amount. (d) A float money path — rejected: the product has none, and a float's last digits are
indistinguishable from a small saving.

**Trade-off.** Adopting the product's arithmetic means an analysis inherits the product's *interface*
(the trace's `usage` field names, `cache_write`, `reasoning` in output) — if the trace's contract moves
(`TRACE_SCHEMA_VERSION`), the ranking moves with it. That coupling is the point: it is what makes a
ranking row comparable with a trace row.

**Reversible?** Yes, cheaply — the arithmetic is one function's worth of code in an analysis artifact, and
the ADR is append-only. Nothing in the product changes.

### D2. The denominator is **$ per 1 000 requests**, on a **named base**, and every row names both

- **The unit is the request**, because that is the unit the router decides on, the unit a trace row already
  carries, and the unit every lever in the inventory acts through. A `$/1M tokens` figure is *not* the
  ranking unit: it would need a second normalisation (whose token mix?) and it would hide the difference
  between a lever that changes every request and one that changes one in a thousand.
- Prices, in contrast, stay in the vendor pages' own unit (`/1M`) with their URL + date, because AGENTS 5
  requires the source's own figure; the row's money is in nano-currency.
- **The base is named, fixed and shared.** A row printed on base `X` is not comparable with a row printed
  on base `Y`, so each row names its base, its sample count `n`, its **basis** (which records were
  included — e.g. all-records vs warm-up-excluded) and the producing commit.
- A base that is **not committed** cannot be a ranking base: a figure whose inputs live only in the tree
  that produced it is not reproducible (the `R33-4-F1` class). Where the producing rig is a live run, the
  evidence is copied into the card's own `run-evidence/` directory — the form R31 established — and
  `.gitignore` is never edited to make a trace commit-able.

**Alternatives considered.** (a) `$/1M tokens` as the ranking unit — rejected (above). (b) Per-provider or
per-provider-model aggregation as the unit — rejected: it answers L4/L5's question only and is blind to
L1–L3, which are per-request effects. (c) Rebasing every row onto the vendor's page unit — rejected: it
re-introduces an FX/mix assumption the product's type system exists to forbid.

**Trade-off.** A per-request figure is only as representative as its base's request shape; a base of two
requests gives an exact arithmetic and a weak average. The method accepts that and makes it visible (§D4's
`inferred` band) instead of hiding it behind a plausible-looking aggregate.

**Reversible?** Yes — a new base is a new table, not a rewrite; the unit is the only part that is expensive
to change (every row would be restated), and it is chosen for the reason above, not by preference.

### D3. The window rule: a money line quotes the rate the run's own window requires

- Peak/off-peak is decided by the **row's own timestamp** against that price entry's `peak.windows`
  (days, `from`/`to`, `tz`), not by the day the analysis ran.
- Every printed money figure **names the rate it used**: the window policy that applied and the resulting
  `peak_applied_pct`. An off-peak figure for a peak-window run may be printed **only as a labelled
  counterfactual**, never as the spend.
- A window holding more than one currency has **no combined total** — per-currency map only. This is not a
  style rule: `Money::checked_add` refuses the addition and no exchange rate exists anywhere in the
  product (ADR-018).

*(This is `R31-3-F2`, adopted as a rule; its measured witness is the committed row's `peak_applied_pct: 200`
beside the `100` rows of a different week-hour.)*

### D4. The label rule: `verified` or `inferred`, never mixed, and nothing is minted by a ranking

1. A lever with **no measured evidence** appears **only** under a **declared assumption**, carries
   **`inferred`**, and prints that assumption beside its number.
2. **`verified` is not mintable by an analysis.** Only the replay harness's ladder mints it, and only with
   the producing row's provenance and condition list attached (VER-4.4). A ranking that prints `verified`
   without them is a defect, not an approximation.
3. `inferred` and `verified` rows are **ordered in separate bands** and are **never summed**.
4. A row whose base is not reproducible is `inferred` **twice over** (assumption + base) and says so.
5. A row that cannot be computed at all (no local route, no mechanism) is carried as an **inventory row
   with no number**, never as a zero.

### D5. The ranking rule: one row per lever, ordered by its own arithmetic, with its evidence attached

A ranking row carries, at minimum: the lever's id, its mechanism in one line, its **Δ** expressed in the
D2 unit, the **base** (id + `n` + basis + producing commit), the **price entry** it used with the source URL
and date, its **label**, and the **one sentence** that says what would move it from `inferred` to
`verified`. A lever that cannot be expressed in the unit gets a row with an explicit "not computable, and
why" rather than an omitted row — an omission reads as a zero.

### D6. What a ranking may never claim (each is a refusal)

1. **Mint `verified`** — no row, no headline, no total.
2. **Copy a price number into `book/`** — the book links to the authority (AGENTS 8).
3. **Print a price without its official source URL + date** (AGENTS 5).
4. **Write a threshold** — budget, transform band, envelope, L1. `autowork/STATE.md:846`'s row 1 stays the
   human's; a method may *measure* a quantity without *setting* a limit (ADR-029's boundary, unchanged).
5. **Touch** the frozen corpus, `tests/conformance/`, `autowork/harness/replay.py`,
   `autowork/harness/replay-contract.md`, `autowork/program.md`, `autowork/work-mode.md`.
6. **Present the ranking as a product surface** — `router stats` serves no `$`-ranked table, and a claim
   that the router ships one would be false.
7. **Rank a saving whose retrieval path does not exist** — e.g. a `tee`-shaped byte saving while v0.1 has
   no originals store and no retrieve channel (spec §4.4).

## Consequences

- **A money row becomes falsifiable.** Because D1 adopts the product's arithmetic, any row can be checked
  against the trace it came from — field by field, including the rate that applied. R34's freeze does this
  against two committed live rows and reproduces both to the nano; the red control (a wrong window, a
  cross-currency add, a mislabelled base) is what keeps the equivalence honest.
- **"Which lever first" becomes an argument with arithmetic under it**, and each row carries the sentence
  that would upgrade it — so R35's choice is auditable after the fact rather than reconstructed.
- **No cost is added to the product**: the ranking is loop-side. The product's serving path, its trace
  contract and its schema versions are untouched by this ADR.
- **A cost the loop accepts:** a base of a few real calls gives an exact arithmetic and a weak average, so
  the first ranking rows will be visibly thin. The method prefers a thin honest base to a fat assumed one,
  and D5's "not computable, and why" row exists so the thinness is legible.

## What this ADR does not decide

- **Any threshold** (row 1), and the choice of the first lever itself (that is R34-2's table and R35's
  round).
- **Any product surface** — no new command, field, trace member or schema version. The ranking lives in
  committed harness artifacts (DESIGN §12.17 is its only product-side pointer).
- **Which corpus a future cost measurement is run on.** A lever's *verification* needs a corpus whose
  content the lever actually touches; the frozen corpus's own `exclusion_note` says it is not that corpus
  for input-side compression, which is a `needs[]` entry whose `closer` is `human` (ADR-026's vocabulary).

## References

- `crates/router-core/src/cost.rs:113-137`, `:150-172`, `:205-218`, `:260-300` — the money type, the price
  unit, the band selection and the formula this ADR adopts.
- `config.example.yaml:110-163` — the price entries with their own source URL + date, and the peak windows.
- `autowork/harness/r31-2/run-evidence/` — the committed live rows whose recomputation is this method's red
  control; `autowork/harness/r34-1/` — R34's freeze, its receipts and its ledger adjudication.
- `autowork/program.md:34-42` (the gate table), `:58-96` (steps 0/0.5), `:135-147` (the direction pool).
- `autowork/STATE.md:846` (row 1) and `:731-734` (the cache facts the lever inventory rests on);
  `autowork/harness/replay-contract.md:1154`, `:1444-1450` (the label ladder and the dirty-tree suppression).
