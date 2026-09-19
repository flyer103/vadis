# ADR-001 — Rust product + single repo (root = the product, `autowork/` = the loop side)

- Status: accepted
- Date: 2026-09-19

## Background

We need a gateway for everyday codex / hermes use: it must be byte-faithful, low-overhead and extensible in
the long run (plugin-based experiments), and it needs a loop side (autowork) that keeps iterating on it.
Two candidate shapes: a single repo (the root is the product, `autowork/` is the loop) or two repos (the
product + a separate autowork repo).

## Decision

1. **The product uses Rust** (a workspace, multiple crates): the data plane needs no GC jitter, a
   predictable p99, and the strong typing constraint of "never alter a byte" (`serde_json`'s order
   preservation and raw byte passthrough are easier to get right than in a dynamic language).
2. **Single repo**: the repo root = the product; `autowork/` is the loop side.

## Rationale

- Product and loop side share a **tightly coupled interface**: trace fields, the plugin contract, the config
  schema, the replay subcommand. Two repos would turn "change the trace format" into a cross-repo
  synchronization problem, and that is exactly the class of bug the old project suffered from most (drift
  across files/repos).
- The loop side's artifacts (rule TOML, config, tier-B plugins) must evolve consistently with the product
  contract in the same commit; a single repo makes "gate PASS → merge into main" one atomic operation.
- Reference shape: an earlier, private Go implementation of the same idea (the root is the product
  + a `research/` loop side) has already proved that this organization can run for the long term.

## Consequences

- The large files under `autowork/` (traces/results/corpus) must be gitignored; the repository keeps only
  the harness/config/round files.
- The loop side's language need not match the product's (see ADR-005).
- A single repo means CI must distinguish two planes: the `cargo` group (blocking) and the `autowork` group
  (reporting-style, non-blocking).
