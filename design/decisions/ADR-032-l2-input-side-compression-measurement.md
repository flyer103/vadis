# ADR-032 — the L2 measurement: the lever, the corpus tier, the label conditions, and what a `verified` figure from input-side compression may claim

- Status: accepted
- Date: 2026-09-24
- Related: AGENTS constraints 1 (the byte boundary), 2 (content determinism), 3 (the observation
  boundary), 4 (**no unverified savings**), 8 (docs before code) and 9 (**the measurement is not part of
  the search space**); **ADR-012** (the self-improvement ladder and the mutable scope — the gates, the
  fixed corpus, the conformance assertions and the L1 envelope are outside it); **ADR-019** (the transform
  mode and the content-edit contract — the mechanism this measurement measures); **ADR-026** (corpus tiers
  and automated scoring — CORP-12's `needs[]`/`closer` vocabulary); **ADR-027** (per-arm plans and the two
  comparison rules); **ADR-028** (the wire recorder: `external`'s honesty cost, and the five conditions'
  live half); **ADR-029**/**ADR-030** (the same boundary shape: *the method is the loop's, the numbers are
  never in a contract file*); **ADR-031** (the cost model and the ranking — the money method this ADR
  deliberately does not restate);
  `autowork/harness/replay-contract.md` §5 (ARM-3.8/3.9/3.9.1/3.9.2), §9 (PROV-8, the label ladder),
  §10 (REAL-9, the live path), §11 (HAND-10: the capture→freeze handoff) and §14 (CORP-12, the two tiers);
  `autowork/program.md:141` (D3's acceptance test) and `:58-96` (steps 0/0.5);
  `autowork/harness/r35-1/FREEZE.md` (the round's decision table) and `r35-1/RECEIPTS.md` (its readings).

## Background

**The repository has run four rounds of measurement and holds no `verified` figure.** R30 made the live
byte audits runnable, R31 fixed the recorder and ran the first post-R24 live pair, R32 measured the
router's own latency under scale, R33 measured the transform path's latency, and R34 built the money
arithmetic and ranked the six levers by $ — every row `inferred`. `program.md:141`'s **Cost gate** reads
*"the `verified` $ and token ledger of a fixed-trace replay"*, and among the four numeric directions D3 is
the only one that is a router transform with a landed mechanism and a defined acceptance test.

**Three things were missing, and each is structural rather than cosmetic.**

1. **No statement of what the lever *is*, operationally.** The transform exists (`ADR-019`, the tier-A
   engine, `rules/tool_output.toml` with its 13 inline tests) and its acceptance test is written in one
   line of `program.md`, but nothing said which rule realises *input-side compression* for the clients
   this repository configures, how the transform's own delta is measured, or what "the corpus can exercise
   it" means as a predicate rather than a plan.
2. **No instrument for the claim.** The label ladder's `verified` is reachable only on a live
   `paired-sessions` run of a **signed** corpus, and the frozen corpus's own `exclusion_note`
   (`autowork/corpus/codex-pair-2026-09-22/manifest.toml:40`) says its tool outputs are small **on
   purpose**. R34 registered the gap (`R34-2-F3`) as a shape note; nothing had measured it.
3. **No bound on what a `verified` figure would mean when it arrives** — on a `paired-sessions` pair the
   two arms send two different client sessions, so the ladder's own arithmetic
   (`delta.input_total = Σ(a − b)`) carries the between-session difference as well as the transform. The
   committed live row proves it: **+79** with **zero** declared edits.

## Decision

### D1. The lever: the tier-A rule engine, in the request's own `Transform` mode, and the rule that realises it is named

L2 is realised by the landed mechanism and by nothing new: `crates/router-plugins/src/transform_rules.rs`
(the engine over `rules/tool_output.toml`), the `Transform` mode and its header resolution, the
composition step `compose_transform_stage` (`crates/router-proxy/src/forward.rs:382-417`) and the per-rule
ledger.

**The rule that realises it for the clients this repository configures is the shell-log rule
`bash-log-noise`** — the one shipped rule that *drops content* from a command's output (volatile-duration
normalisation, progress/blank-line stripping, `truncate_lines_at`, `max_lines`) — with `diff-budget` and
`grep-hits-budget` for the patch and search families and `tool-result-json` for JSON payloads. The
naming is a measurement, not a preference: every tool call in this repository's only real client traffic
is named `exec_command` (36 payload nodes, `r35-1/corpus-shape.json`).

**Two figures, never substituted for one another:**

- **the transform's own byte delta** — the treatment arm's ledger (`TransformRecord{plugin, edited_paths,
  saved_input_tokens, added_input_tokens, cache_impact}`), per rule per request, cross-checked against the
  recorded wire by `ledger.edit_counts_match_wire`; exact in bytes, **`inferred`** in tokens
  (`estimate_tokens` = `bytes/4`, `crates/router-core/src/transform.rs:189-191`);
- **the pair's measured usage delta** — `delta.input_total` read from the **upstream's own `usage`** on
  both arms. This is the only figure that may carry `verified`.

### D2. The acceptance predicate is a **triple**, not a non-zero delta

A corpus (or a run) **exercises L2** iff, on a live `paired-sessions` run of it:

1. the treatment arm's ledger declares **≥ 1 edited path** on a paired measured item (the transform
   fired); **and**
2. `delta.input_total ≠ 0` with the sign of a saving; **and**
3. the pair's between-session content difference is **bounded and published** — the `[[pair]]`'s
   `session_note` states what is asserted identical, and the round's record carries the two measured
   items' own byte difference outside the payload nodes.

`verified net gain > 0` means the triple **plus** the five conditions of §D3. The third limb exists
because the first two can both hold while the number is not the transform: `r31-2`'s committed row reads
`delta.input_total = 79` with `0 edited path(s)`.

### D3. The label conditions are the contract's, quoted rather than restated

A `verified` label requires, conjunctively (`replay-contract.md:1456-1512`): (0) distinct observed client
session identities **and** a live path in REAL-9 §10.1's sense; (1) **both arms `external`**, on ADR-028's
recorder route only when that route's own conditions hold; (2) the upstream's own `usage`, both arms;
(3) the three byte audits **passing** on every paired item (never an absence — `VER-4.4.1`); (4) a clean
harness tree at a named commit with the router binary hashed. Any failure ⇒ `inferred`,
`verified_eligible: false`, and the **first failing condition named**. **This ADR adds nothing to the
ladder** — it is outside the loop's mutable scope (ADR-012 item 2) — and adds one reporting obligation:
limb 3 of D2 must be published beside the figure.

### D4. The corpus tier is not a policy choice; it is a type boundary

**Only a signed corpus (`autowork/corpus/<id>/`) can carry the claim.** The ladder's fields do not exist
on the auto layer's row type, its `[[pair]].kind` vocabulary is `paired-arms` only
(`inferred`/`unpaired-session` by ARM-3.9.2), its `source` vocabulary excludes `capture`, and a
declaration naming a live path is refused (`replay.py`'s `suite-live-not-permitted`). This is structural,
not policy: §14.2.1 chose a **row type that does not define the ladder at all** over every mechanism that
could be answered wrongly by an actor the loop can write.

**The loop may build the bytes; a human freezes them.** Two legitimate routes — a **capture** of real
client traffic (HAND-10 §11.1/§11.5; the orchestrator's act, and §11.4 forbids producing it as a card's
work) or a **nomination** from the auto layer (§14.6: build + `nominate`, with the bytes copied by the
human who accepts it) — and in both the freeze is §11.3's four commands, executed by a human, with
`created_by = human:…` (COR-2.4). The loop's entire surface is copying bytes, hashing them, filling
`item_count`, emitting a digest and printing a refusal table. **A round whose honest path needs a signed
freeze stops and registers `needs_input`** — it does not improvise a corpus and does not reach for one.

### D5. What a `verified` figure from this lever may and may not claim

**May claim:** that on a named corpus, a named commit and a named clean instrument, two live external arms
on the same workload in two client sessions produced a measured usage difference whose sign is a saving,
with ≥ 1 ledger-attested edit on the treatment side — and the figure, with its per-row usage, sample
count, basis, producing commit and window rate, is recomputable from committed bytes.

**May not claim:** that the figure is the transform's alone (limb 3's bound is published beside it); that
it generalises beyond the corpus's own shape; that it is a product claim (`router stats` serves no
`$`-ranked table, ADR-031 D6); that it carries a threshold (the L1 envelope is `autowork/STATE.md`'s
waiting-on-human row 1); that a `tee`'d original is retrievable (the retrieve channel is unimplemented,
spec §4.4); or — where the corpus's bytes were composed rather than captured — that they are client
traffic. On a corpus frozen under delegation (R23-F6) the *same breath* must say so, as must a run whose
`external` rests on ADR-028's recorder route.

### D6. Selection is part of the lever, and a rule that cannot fire is not a lever

A rule whose selection inputs name no client this repository configures is **unit-green and
live-unreachable** — the class the repository already calls a defect (`R33-F1`). The operative selection
path for real traffic is `match_tool`, over the tool names the wire actually carries
(`transform.rs:154-156`); a reachability assertion whose subject is the declared table rather than an
observed client vocabulary cannot fail for the right reason.

## Consequences

- **R35-1 stops.** The corpus limb needs a human freeze; the card registers `needs_input` and writes no
  corpus and no corpus-side plan. The lever, the protocol, the label conditions, the prohibitions and the
  ledger are decided and corpus-independent, so the round can resume the moment the corpus exists.
- **Two blocking findings and two rule-file repairs enter R35-2's scope**: selection by the clients' real
  tool names, and `tee` on the shell rule (today only the patch and search rules carry the marker).
- **Every later round must name the corpus's provenance** in the same breath as a figure derived from it
  — the delegated freeze, the recorder route, or composed bytes.
- **Nothing in the product moves**: no trace field, no event, no schema version, no reporting surface, no
  new dependency. The measurement is the loop's; the numbers live in the round's own artifacts.

## What this ADR does not decide

- **It sets no threshold**, no envelope and no band (row 1).
- **It does not rank the levers or touch ADR-031's arithmetic** — it adopts that method (`$ per 1 000
  requests`, a named base, the window rule, the label rule) and adds no row.
- **It does not amend the label ladder, the gates, the corpus, the conformance assertions or the L1
  envelope** (ADR-012 item 2; AGENTS 9).
- **It does not decide whether `crates/router-core`'s `TOOL_KINDS` table should name configured clients**:
  that is a human's wording decision, registered in `r35-1/FREEZE.md` §PREREQUISITE.
- **It does not choose between the capture route and the nomination route** — that is the human's answer
  to the round's `needs_input`.

## References

`autowork/harness/r35-1/FREEZE.md` (D1…D9 and the `needs_input` request) · `r35-1/RECEIPTS.md` (the base,
the corpus score, the probe's method, the readings) · `r35-1/probe_corpus_shape.py` + `corpus-shape.json`
(the committed measurement) · `r35-1/gates.log` (the gates at this card's own HEAD) ·
`autowork/harness/replay-contract.md` §5/§9/§10/§11/§14 and `:1456-1512` (the five conditions) ·
`autowork/harness/r31-2/run-evidence/result.jsonl` (the committed live row: `delta.input_total = 79`, `0
edited path(s)`) · `autowork/corpus/codex-pair-2026-09-22/manifest.toml:40` (the corpus's own
`exclusion_note`) · `rules/tool_output.toml` · `crates/router-core/src/transform.rs:154-191` ·
`crates/router-plugins/src/transform_rules.rs:178-337` · `crates/router-proxy/src/forward.rs:382-417` ·
`autowork/program.md:141` · `design/DESIGN.md` §12.12 (the transform pipeline) and §12.18 (this method's
product-side pointer) · `design/decisions/ADR-031-cost-model-and-lever-ranking.md`.
