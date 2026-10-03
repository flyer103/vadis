# ADR-001 — Rust product + single repo (root = the product, the loop side beside it)

- Status: accepted
- Date: 2026-09-19

## Background

We need a gateway for everyday codex / hermes use: it must be byte-faithful, low-overhead and extensible in
the long run (plugin-based experiments), and it needs a loop side that keeps iterating on it.
Two candidate shapes: a single repo (the root is the product, the loop tree beside it) or two repos (the
product + a separate analysis-loop repo).

## Decision

1. **The product uses Rust** (a workspace, multiple crates): the data plane needs no GC jitter, a
   predictable p99, and the strong typing constraint of "never alter a byte" (`serde_json`'s order
   preservation and raw byte passthrough are easier to get right than in a dynamic language).
2. **Single repo**: the repo root = the product, with the loop side beside it.

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

- The large files under the loop's tree (traces/results/corpus) must be gitignored; the repository keeps only
  the harness/config/round files.
- The loop side's language need not match the product's (see ADR-005).
- A single repo means CI must distinguish two planes: the `cargo` group (blocking) and the analysis loop group
  (reporting-style, non-blocking).

## Publication note (2026-10-03, R62-2)

The `R<n>` labels and finding ids in this document name iterations of the project's own
private analysis loop — a loop that is not part of this repository, so no label here is
resolvable by a reader of it; they are kept as the provenance of the decision. This
publication pass removed only the dead-pointer class: every reference into that loop's
working tree (its file paths and round-record names, its state record, charter, execution
model and replay contract, its scripts and module names, and the kanban card ids), each
replaced by the neutral phrase the sentence needs. Nothing else moved — no figure,
threshold, `§`/`ADR`/`CONF` id, code sample or contract sentence.
