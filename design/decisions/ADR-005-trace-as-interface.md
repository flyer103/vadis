# ADR-005 — the trace is the only product↔analysis-loop interface; the analysis loop = Python orchestration + the product's replay

- Status: accepted
- Date: 2026-09-19

## Background

The analysis loop must keep iterating on the product (cost policy, plugins, rules). The old project's
biggest lesson was **a double implementation of the policy**: the product implemented the policy in Go and
the evaluation side re-implemented it in Python, which produced train/serve skew (inconsistent
classifier/feature conventions) and polluted many rounds' conclusions with convention problems. The second
lesson was pushing research observation into the serving path (injecting internal context, which made
prompt_tokens 6× the competitor's and directly distorted the cost convention).

## Decision

1. **A single direction**: product → analysis-loop only through **trace JSONL** (the DecisionRecord of spec §6);
   analysis-loop → product only through **policy artifacts** (config / rule TOML / tier-B plugin). Research code
   never enters the serving path.
2. **The analysis loop's language = Python (uv) orchestration**: data collection, judging, statistics and reports
   use the most mature tools in the ecosystem; **no second implementation of the policy logic**.
3. **Policy simulation always calls the product itself**: `router replay --trace ... --config ...` uses the
   same binary, the same decision pipeline and the same encoding path as production, replacing only the
   outbound HTTP with a local simulation. Offline replay is therefore skew-free by construction and
   reproducible.
4. **A gate only holds on replay**: any "cheaper/better" conclusion must be replayable on a fixed trace;
   live A/B serves as corroboration only.

## Rationale

- Judging/statistics/visualization cost an order of magnitude less on the Python side; and the part that
  must be "byte-identical to production" is already implemented on the Rust side, so re-implementing it is a
  net loss.
- The trace as the only interface makes "research does not enter the serving path" a verifiable structural
  fact (a boundary that can be asserted) rather than a discipline requirement.
- The replay convention and the online convention share one origin, which turns "how much did it save" from
  an opinion into a recomputable number.

## Consequences

- The trace schema is part of the product contract (spec §6); a change must be synchronized with the
  analysis harness (same repo, atomic commit).
- the analysis loop can only observe what is in the trace; a missing dimension must first be filled in on the product
  side — a deliberate constraint (it keeps the research side from patching data by inference).
- Experiments with tier-B plugins (judging/exploration class) go through the out-of-process protocol and come
  with timeouts and crash isolation by construction; tier-A carries only the deterministic path.
- Experiments that need real upstream calls (judging, generation) keep the budget gate; paid rounds must have
  their budget confirmed in advance.

## Publication note (2026-10-03, R62-2)

The `R<n>` labels and finding ids in this document name iterations of the project's own
private analysis loop — a loop that is not part of this repository, so no label here is
resolvable by a reader of it; they are kept as the provenance of the decision. This
publication pass removed only the dead-pointer class: every reference into that loop's
working tree (its file paths and round-record names, its state record, charter, execution
model and replay contract, its scripts and module names, and the kanban card ids), each
replaced by the neutral phrase the sentence needs. Nothing else moved — no figure,
threshold, `§`/`ADR`/`CONF` id, code sample or contract sentence.
