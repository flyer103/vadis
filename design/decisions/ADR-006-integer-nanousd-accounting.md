# ADR-006 — the accounting unit is integer NanoUsd (fixed-point amounts, no f64 accumulation)

- Status: accepted
- Date: 2026-09-19
- Related: spec §6 (cost) / §7 (accounting convention); DESIGN §5/§12.4; implementation `crates/router-core/src/cost.rs`

## Background

router's output convention is "how much money did this policy save", and that number must be **recomputable
byte for byte**: the cost recorded in the trace must be recomputed by `router replay` with the same code
into exactly the same value. A single request's cost is on the order of 1e-7 ~ 1e-3 USD; accumulating with
`f64` over millions of requests drifts, and IEEE-754's rounding behaviour depends on operation order — two
"equivalent" implementations produce reports with different bytes, so the gate's "net gain > 0" becomes
"whoever computes first decides".

At the same time the prices in config are human-readable strings (USD/1K, while official pages mostly
publish USD/1M), so **load time** inevitably touches decimal text. The question is not "may floating point
be used" but "in which layer may floating point stay".

## Decision

1. **Amounts are fixed-point integer nano-USD (1e-9 USD)**: `NanoUsd(u64)`, and the unit price `Price(u64)` =
   nano-USD / 1K token. The decision path (model choice, guard, quota, breakeven, accounting, trace) is
   **integers all the way**.
2. **The amount → nano-USD conversion point is unique and only at load time**: the config's `USD/1K` is
   integer-ized once as `Price((v * 1e9).round() as u64)` (DESIGN §12.5); no decimal appears at runtime any
   more, and `v < 0` or a `round` result of 0 counts as a load error.
3. **No `f64` in cost accumulation**: `router-core` enforces it with a crate-level
   `#![deny(clippy::float_arithmetic)]`; the only exemption is `Usage::cache_hit_rate()` — it is a
   **derived metric** (not on the money path) and is annotated `allow` at each point.
4. **A single rounding point**: each tier's `tokens × price(per 1K)` divides by 1000 with floor
   (`saturating_div_1k`), and aggregation uses integer (saturating) addition; apart from that one place
   there is no rounding anywhere. If proportional allocation is introduced later, it must reuse the same
   rounding point and must not add a second one.
5. **Overflow saturates rather than panicking**: the money path must not panic on abnormal input
   (`saturating_add` / `saturating_mul_pct`); interval estimates use `u128` intermediates.
6. **Decimals appear only in the final formatting layer**: report/trace stringification happens at the
   boundary, and the internal types carry no "approximate value".

## Rationale

- Integers make trace → report recomputation **isomorphic**: replaying the same trace twice necessarily
  yields field-by-field identical results (the basis of CONF-19's assertion).
- A unique conversion point ⇒ an error like "one 0 missing in the price" can surface in only one place
  (loading) instead of being scattered over the decision path.
- Saturating instead of panicking ⇒ malformed usage from upstream (e.g. `cached > total`) degrades into
  "the number is too large" rather than "the process crashes", consistent with spec §8 "fail-safe, do not
  block the request" (`uncached()` uses `saturating_sub` too).

## Consequences

- The boundary cases of the five-tier price, peak/off-peak, quota and breakeven can all be pinned with
  integer anchors (e.g. `gain×100 == sf×cost` being exactly equal must be judged `Stay(NotPaying)`), no
  longer relying on floating-point comparison tolerance.
- When upstream `usage` is missing, "estimating the numbers" is forbidden: the gap is recorded as
  `result.usage_missing`, the cost counts as 0 and is marked explicitly (§7 convention).
- The authority for the price convention remains the config (`source` + the TODO annotation); this ADR fixes
  only the **representation** and does not touch the price-source discipline (AGENTS hard constraint 5).
- If tiered/step pricing per token is introduced later, it must be modeled explicitly as new tiers rather
  than introducing floating point in an intermediate layer.
