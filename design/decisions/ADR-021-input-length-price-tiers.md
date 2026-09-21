# ADR-021 — a price table may be banded by input length: the vendor's bands are transcribed tier for tier inside `price`

- Status: accepted
- Date: 2026-09-22
- Related: AGENTS hard constraints 1 (the byte boundary — a price is not body bytes, so nothing here touches
  the passthrough path), 4 (no unverified savings — this ADR adds no figure of any kind), 5 (no fabricated
  prices — the reason a band's ceiling and its four numbers must each carry a cited heading), 7 (English), 8
  (docs before code), 9 (the measurement is not in the search space: no gate, corpus or conformance assertion
  moves); ADR-003 (the cost pipeline) / ADR-006 (integer fixed-point accounting) / ADR-018 (the unit travels
  with the value — a band carries no unit of its own, and the price table stays the entry's); ADR-014 and spec
  §4.6 (the plan's zero marginal price and the metered cap are untouched); ADR-019 (the saving convention this
  ADR adds nothing to); spec §4, §4.0, **§4.10** (the contract this ADR explains), §6, §7, §9.2; DESIGN §5,
  §12.4, §12.5, §12.9 (GAP-Q20), §12.13 (the landing); GAP-Q6 (holidays are not modeled — the same stance is
  taken for promotional calendars); `config.example.yaml` (the file the transcribing round fills in)

## Background

### What shipped

A model entry has exactly one price table: the four tiers (`input_miss` / `input_hit` / `cache_write` /
`output`, per 1K tokens in the entry's currency) and one `peak` multiplier with its windows (spec §4.0). The
table is integerised once at load (`config.rs`'s `PriceCfg::to_price_table`) and rebuilt once per request for
the resolved route, and the request's money is `cost(usage, table, at)` over the **measured** usage — which is
what makes the cost group `verified` in the sense spec §7 and §9.2 give that word.

### The reading that breaks it

Several vendors publish their tables **banded by input length** — the same model carries one rate while the
request's prompt stays within a threshold and another rate above it, occasionally a third band. With one field
for one rate, such a page can only be transcribed as **one** of its bands:

- quote the smallest band: every long-context request is under-priced (its real rate is higher);
- quote the largest band: every short request is over-priced (its real rate is lower).

Neither is an estimate and neither is invented — and that is exactly what makes the failure quiet: the config
holds a number the page really published, applied to requests the page prices differently. The figure that
lands in the trace, in `router stats` and in every downstream gate is then wrong for a *known* subset of
traffic, and `verified` (a measured delta in measured usage) cannot detect it, because the *usage* is measured
and only the *rate* is mistranscribed. The requirement this ADR answers: `config.example.yaml` must be able to
transcribe a banded official table **band for band**.

### Why one band cannot be "the" price

A banded table is not an approximation of a flat one. The discontinuity is the vendor's own published
structure: crossing a ceiling re-prices the request, in their table, at their rates. Reproducing it is
faithful; smoothing it (an average rate, an interpolation, a progressive split) would be *our* arithmetic
presented as *their* price, which constraint 5 forbids in the same sentence it forbids estimation.

## Decision

**1. The bands live inside `price`, and the flat block is the degenerate banded one.** `price` carries either
the four flat tiers (today's shape, untouched) or a `tiers:` list whose every entry carries the same four
prices; never both, never neither, and `peak` is required by both. The flat shape means exactly what a one-band
list means — **one price for every input length** — and the two spellings must produce identical money for
identical usage. No existing entry needs a byte changed (the backward-compatibility clause).

**2. A band's ceiling is inclusive, and the top band declares none.** `up_to` is a band's upper bound on input
tokens: a request with `n <= up_to` is priced by that band. The **last** band omits the key and covers every
larger input; omitting it is the only spelling of "no ceiling". A ceiling is a plain integer number of tokens —
no `k`/`m` suffix (a measured count is compared against it, and a vendor's "200K" is not reliably 200×1024).

**3. `n` is the measured total input of the request.** `n = usage.input_total` — the whole prompt as the
upstream counted it, **cached tokens included**. Not `input_total + output` (that is the quota convention,
GAP-Q1) and not an estimated prefix figure. A band describes the size of the prompt that was sent, so the band
must not move with how much of it the cache served.

**4. The selected band prices the whole request.** The band's four prices price *all* of the request's tokens —
a banded table, not a progressive one. No vendor publishes a bracket formula, and inventing one would put the
router's arithmetic where the vendor's price belongs.

**5. Peak/off-peak is orthogonal to the band.** One `peak` table per model entry, at the price level and
outside the tiers; `peak` inside a band is a load error. The band chooses the four prices, the time window
multiplies their sum once — the existing arithmetic, unchanged, and no band can carry a multiplier of its own.

**6. Bookkeeping does not change, and the record gains nothing.** The trace's `cost` group stays four money
buckets + `total` + `cost.currency`, computed by the same code; **which band priced a request is not a trace
field.** It is derivable from the record itself (`usage.input_total` against the priced config of that era),
the record already carries money rather than prices, and a band is a price — not a change to the request, not a
saving, and not a delta. No new figure of any kind is introduced by this ADR.

**7. Structure is refused at load; prose is checked by a human.** A start-up refusal covers both shapes at once,
an empty or over-8 list, a ceiling that is not a positive integer, zero or several unceiled bands, an unceiled
band that is not last, equal or descending ceilings, a band missing one of its four prices, a band's
`input_miss`/`input_hit`/`output` converting to 0 (`cache_write: 0` stays legal, per band), and a `peak` inside
a band. A **hole** between bands needs no refusal because it is not expressible: a band's floor is the previous
band's ceiling + 1. The one thing the loader cannot check — that a band's ceiling and numbers are the page's —
is governed by a citation rule: one comment line per band, naming the page's own heading for that band (and its
own URL when the citation spans pages) beside the entry's single `source`.

**8. A figure computed before the upstream answers uses the first band.** The switch's re-prefill cost and the
cache-aware breakeven's two unit prices are read at decision time, when no measured `n` exists. They are
computed from the first (lowest-ceiling) band — the one band every entry is guaranteed to have, and a choice
that invents no estimate — and they keep their existing `inferred` label (§7). They may not be read as a
band-faithful quote of the real cost; the trigger for revisiting is a request-size estimate existing at
decision time, which the dependency allowlist does not have (GAP-Q14, and GAP-Q20 registers this row).

## Alternatives considered

- **A sibling key (`price_tiers`) beside `price`.** Expressive power identical, and it was the cheapest diff.
  Rejected because one model entry would then hold prices in two places, so "which one won" becomes a rule of
  its own and a reader must check both. Inside `price`, the alternation is structural: exactly one of the two
  shapes is present, and the loader says so.
- **`price` becomes a YAML sequence in the banded case.** Rejected: the key would have a union type, the flat
  form would have to change spelling (breaking every existing entry), and the load errors would degrade to
  "invalid type" instead of naming the offending band.
- **An explicit `from`/`to` per band.** Rejected: adjacent bands would state the same boundary twice — the
  drift surface §4.0 exists to prevent — and it makes a *gap* expressible, which then needs its own validation.
  With ceilings only, coverage is structural (`[0, ∞)` by construction) and the illegal states are
  unrepresentable; that is cheaper than validating them.
- **`up_to: unbounded` as a keyword.** Rejected: two spellings for one property, a string where the field is
  otherwise an integer, and it makes a *middle* band able to claim "no ceiling". Omitting the key gives the
  parser exactly one spelling, and the "must not be in the middle" refusal is needed under either spelling.
- **`k`/`m` suffixes on the ceiling.** Rejected: `context`'s suffix multiplies by 1024, while a vendor's "200K"
  band usually means 200,000. A band boundary is compared against a *measured* token count, so it must not
  depend on which K a reader assumed. The page's own wording belongs in the band's citation comment.
- **Progressive (marginal) bands — the first 200K at one rate, the excess at another.** Rejected: no vendor
  publishes a bracket formula for these tables; the arithmetic would be the router's invention, and it would
  silently disagree with the invoice.
- **A `peak` per band.** Rejected: N copies of one table, and a band that lacked the multiplier would
  under-price peak traffic. Orthogonality is enforced by where the key lives, once per entry.
- **A `source` per band.** Rejected: the entry has one source and it must be the page carrying all of its
  bands; the band's *section* is prose, which a per-band URL field would not make machine-checkable either.
  The comment convention keeps the check where it already happens — one click, at review.
- **Bands on output length.** Rejected: the output does not exist when the request is priced, the published
  tables band on input, and pricing a request on its answer would make the price depend on the answer.
- **Inferring bands from `context`, from a sibling entry, or by interpolation.** Rejected as fabrication
  (constraint 5): a band's ceiling is a fact the vendor published.
- **Recording the applied band in the trace.** Rejected *for now* — see the trade-off below; it stays
  available as an additive change.

## Rationale

The config's job is to be **checkable** — ADR-020's argument, applied one level down. A flat transcription of a
banded page is checkable and wrong: the reviewer clicks the cited URL, finds the number, and confirms it, while
the router prices a growing share of the traffic at a rate the page assigns to another band. The mistake is
invisible exactly where it matters most: the cost gate's own figure. Transcribing the bands as published costs
one line per extra band and moves the check no further away than the heading the numbers sit under. Between
"a schema that cannot express the page" and "one key whose absences and ordering are refusals", the repository's
constraints (1, 4, 5, 9) choose the second — and the second is additive, so nothing already written has to move.

## Consequences

- A banded entry is longer by one block per band, and each band carries the page's own heading in a comment. A
  flat entry is unchanged, character for character, and prices identically (spec §4.10 rule 1) — the
  implementing card owes that equality as a unit test.
- `n` is measured, so a request's band is decided **after** the response; the one figure read earlier (the
  switch's cost) is band-agnostic by convention and stays `inferred` (GAP-Q20). Nothing in the router guesses a
  request's size to pick a band.
- The measurement apparatus is untouched **by construction**: no trace field, no event kind, no projection, no
  gate definition, no corpus, no conformance assertion, no `schema_version` move (§12.6's rule is not even
  reached, because nothing is added), and no saving figure anywhere in the round.
- `book/cost-and-caching.md` gains the user-facing reading: why a longer prompt can change the price of the
  whole request, and where the bands are recorded — with no price number and no type sketch (constraint 8).
- The router's own reading of a banded table is *reproduced discontinuity*: a one-token difference across a
  ceiling re-prices the entire request, because that is what the vendor's table does.

## Honest boundaries and verification owed

- **The router does not verify a band against the page.** The loader can prove a table is well-formed and that
  the money follows from it; it cannot prove the ceiling or the four numbers are the vendor's. The only defence
  is the citation rule (constraint 5) plus the human re-read — the same boundary ADR-020 accepted for URLs and
  ADR-018 for `region`.
- **The band a request *should* fall in is the page's fact, not the router's.** Where a page is ambiguous (a
  band stated in "characters", or a threshold in a unit the router never sees), the entry stays flat rather
  than guessing; the honest move is a flat table plus a comment, not an inferred band.
- **Pre-response figures are band-agnostic** (item 8). The number stays `inferred`, and the direction of its
  error is not claimed to be safe in either direction.
- **No cost, cache or latency gate is re-measured** by the round that lands this: the freeze changes no
  serving-path byte, so the four gate commands are regression evidence, and the band semantics are proven by
  unit tests at the seams (the boundary table, spec §4.10) plus the independent re-derivation a later card owes.
- **The trace cannot be re-priced after a price change** — and a banded config makes that more visible, not
  worse: the band a request fell in lives in the config of its era, which is why the trace carries money and
  the config carries prices (the existing split, restated here because banding makes it visible).

## Reversibility

Fully reversible and additive: `tiers` is a new optional key, every existing entry is untouched, and nothing is
persisted that depends on the shape — the trace's members, the store's event kinds and the projections are all
unchanged. Removing the key later is the inverse commit (the loader refuses `tiers`, banded entries return to
one band each, and the money returns to the band-1 figures for every request). The one thing that does not
reverse cheaply is data, not code: a config transcribed band for band would have to choose a band again, and
traces written under a banded config are priced by rates that live only in that config — the same property
every price edit already has, and the reason `source` carries a read date.
