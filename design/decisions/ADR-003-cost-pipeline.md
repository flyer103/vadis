# ADR-003 — saving cost = a revertible, declarative, individually accounted transform pipeline

- Status: accepted
- Date: 2026-09-19

## Background

"Built-in token saving" most easily degenerates into a pile of hard-coded tricks that cannot be attributed
or rolled back, and that often **conflict with the prompt cache** — compressing or rewriting early context
invalidates the whole prefix cache, and the 30% saved is eaten by the recomputed re-prefill. The community
already has reusable mature shapes: rtk (Apache-2.0, Rust, a declarative TOML filter pipeline + inline
tests + original-text tee/retrieve, fail-safe passthrough, <10ms overhead); caveman (its proxy is BSL-1.1,
used as a design reference only, not embedded).

## Decision

1. **Saving cost = a transform pipeline**, where each transform is a plugin that must be: revertible
   (ADR-002), content-deterministic (no dependency on turn/clock/RNG), individually accounted, and falling
   back to the original text on failure.
2. **A fixed priority** (first-order first, backed by measurement: the second round of the same session has
   `cached_tokens` 14400/14520 = 99.2%):

   | Tier | Means | v0.1 |
   |---|---|---|
   | P0 | prefix-cache fidelity (zero rewrite, stickiness, breakpoint injection, cache ledger) | ✅ |
   | P1 | input-side payload compression (tool_result/logs/JSON/diff/search results; original-text tee + retrieve) | ✅ |
   | P2 | output-side discipline (append-only instruction injection, max_tokens/stop, structured-output constraints) | ✅ |
   | P3 | provider arbitrage (hit pricing, peak/off-peak, plan quota priority, batch) | ✅ |
   | P4 | dedup/trim/summarize (rewrites early content, the biggest cache conflict) | deferred |

3. **Rules are data**: compression rules are described in TOML (shaped after rtk: pipeline stages,
   match_output, keep/strip_lines, truncate, head/tail, max_lines, on_empty), and **every rule must have
   inline tests** (input/expected), with three-level override (project → user → builtin). Admission of a new
   rule = all inline tests green + the cache regression passes.
4. **A binary accounting convention**: `verified` (the measured delta of upstream usage, requiring a
   comparison round) and `inferred` (a local estimate). Only verified may enter a gate or be reported
   externally; a report must state its convention, sample size and time window.
5. **License discipline**: reuse the implementation ideas and code of Apache-2.0/MIT; BSL/non-OSI licenses
   (e.g. the caveman runtime) are read for design only and never embedded.

## Consequences

- Every transform raises the net-gain question of "added tokens for saved tokens"; a rule with a negative
  net gain must be stoppable by the gate (this is also why the verified convention exists).
- Input-side compression **applies only to tool/environment payloads** and never rewrites user intent:
  third-party measurements (JetBrains, 86 tasks; Adobe CAVEWOMAN, arXiv:2606.24083) show that compressing a
  human prompt makes the model answer longer and worse.
- Deferring P4 is a deliberate decision: any rule that rewrites history needs a `prefix_continuity` metric
  and a rollback mechanism first.
