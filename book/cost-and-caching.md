# Cost and caching

Status: outline only. This chapter explains the levers and how to verify them. It does
**not** contain price numbers or type sketches: prices live in `config.example.yaml`
(each entry with its `source` URL and capture date), the accounting definitions live in
`docs/spec.md` §7.

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
  provider's own models; what happens when a quota runs out is a configured policy.
- **Breakeven**: sticky sessions and any cache-writing transform are judged against an
  explicit breakeven rule rather than intuition.
- **The transform pipeline** and its priority order: cache fidelity, then input-side
  payload reduction, then output-side discipline, then provider arbitrage. Every step is
  reversible, declarative and individually accounted.
- **Verified versus inferred**: only a measured usage difference counts as a saving; a
  local tokenizer estimate is labelled as such and can never be reported as measured.
- **How to check the claim**: `router stats` to read the cost and cache report,
  `router replay` to recompute money over a fixed trace with the same code path.

## Authoritative sources

- [`config.example.yaml`](../config.example.yaml) — the price table and the only place
  price numbers exist, each with `source`.
- [`docs/spec.md` §4.0](../docs/spec.md) — the price convention (why this document holds
  no numbers, how peak windows are encoded as a multiplier).
- [`docs/spec.md` §7](../docs/spec.md) — the accounting convention: `verified` vs
  `inferred`, and the reporting requirements for any savings claim.
- [`docs/spec.md` §6](../docs/spec.md) — cache metrics exposed to the user
  (`cache_hit_rate`, `prefix_continuity` percentiles).
- [`design/DESIGN.md` §5](../design/DESIGN.md) and [§6](../design/DESIGN.md) — the cost
  engine and the cache policy.
- [`design/decisions/ADR-003-cost-pipeline.md`](../design/decisions/ADR-003-cost-pipeline.md)
  and [`ADR-006`](../design/decisions/ADR-006-integer-nanousd-accounting.md) — the pipeline
  discipline and the fixed-point money rule.
