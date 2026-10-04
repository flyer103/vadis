# ADR-030 — the transform path's latency: the four-assembly shape, the attribution model and the noise-band reporting rule

- Status: accepted
- Date: 2026-09-23
- Related: AGENTS constraints 1, 2, 3, 4, 5 and 9; **ADR-029** (the scale baseline: the gate quantity
  `router_overhead_ms` = `result.overhead_ms − result.upstream_ms`, the A–E ladder, the per-rung rules,
  the ceiling criterion, and §D5's boundary — *the method is the loop's, the threshold is not*);
  ADR-019 (the transform mode: mode channel, edit discipline, the three invariants, the ledger);
  ADR-003 (rules as data) and ADR-008 (the trust gate); ADR-012 (the gate definitions, the corpus, the
  conformance assertions and the L1 envelope are outside the loop's mutable scope); ADR-016 DP-1.4 (the
  envelope has no contract home); ADR-017 §3 (declared vs measured); ADR-009 (the store's one writer and
  the latency budget as the vadis's own work); spec §2.1 (the mode channel), §4.4 (rule files, `tee`),
  §4.13 (the body bound), §6 (the trace contract), §7 (the labels), §8 (the refusal contract),
  §9.2/§9.3 (the report); DESIGN §12.1 (the dependency allowlist), §12.12 (the pipeline and its
  ledger), §12.15 (the body bound), §12.16 (the baseline's quantity); the loop charter (the
  **blocking** latency gate and its declared reference, rtk's <10 ms shape);
  the loop state record (**R9-G5**: the transform path's latency unmeasured) and the same file (the
  waiting-on-human **row 1**, still open).

## Background

**The blocking gate names the transform and no measurement has ever included it.** The loop charter
makes *"the decision **+ transform** overhead p99 stays within budget (benchmarked against rtk's <10 ms
shape)"* blocking. Every latency figure in the repository predates the transform path: R4's
`overhead_ms` p50 1 ms / p99 6 ms (a mock upstream, no rule engine) and R2G6's narrow self-overhead pair
(the loop state record's R2G6 pair). R9's landing wired the pipeline and measured nothing (the loop state record, **R9-G5**:
"R9-3's probe has **no** timing section"), and DESIGN §12.12 said in its own words that the budget "is
re-measured by the round that lands this" — a sentence the landing round did not discharge. R32 froze
the *quantity*, the *ladder*, the *per-rung rules* and the *ceiling criterion* (ADR-029) and declared,
in §D5, that **the transform path's contribution is `to be measured by R33`**. This ADR is R33's half:
the **method** by which the transform-inclusive measurement is taken, comparably with R32's.

**Two structural facts about the transform path decide the shape of the measurement.** Both are
measured, not assumed, at R33's base by the harness-side probe
the loop's freeze probe script (its relations are the evidence; the numbers live in that
file and are not restated here):

1. **The payload locator returns tool-output nodes only.** `vadis-core::transform::payload_nodes`
   yields a chat body's `messages[n].content` where `role == "tool"` (paired to an earlier assistant
   `tool_calls` naming the tool), a responses body's `input[n].output` where the item is a
   `function_call_output`, and an anthropic body's `tool_result` block. A body whose payload sits in a
   **user or assistant message** — which is exactly R32's own synthetic payload shape, one
   `role: user` message carrying the pad — has **zero payload nodes**, so loading the engine and asking
   for `X-Vadis-Transform: transform` on it changes **no byte** and fires **no rule**. R32's shape is
   therefore the right *reference* and the wrong *carrier*.
2. **Rule selection is by declaration, and the landed declaration table cannot reach every rule.**
   `CompiledRule::selects` matches the tool name carried by the wire's own pairing and the kinds the
   declared table gives that tool. At this base three of the landed rule file's four rules fire on the
   live path and **`tool-result-json` cannot** — a fact the probe measures and which is registered as a
   finding of R33's freeze, not quietly worked around here.

**A measurement of "the transform's cost" is a difference of two measurements, and the shape of that
difference is the whole design problem.** R32's own lesson is that the *named rung* of a criterion is
noise-sensitive at 1 ms integer resolution while the **flatness of a curve** is citable; a round that
compares two assemblies must additionally separate *the payload shape it carried the transform in* from
*the transform itself*, or its headline difference will be a shape artefact.

## Decision

### D1. The measurement is a four-assembly comparison on R32's own ladder

Four assemblies, one variable apart, on the same machine, the same commit, the same stand-in class and
the same per-rung rules:

| Assembly | Rule engine | Mode header | Payload shape | What it isolates |
|---|---|---|---|---|
| **P** — the reference | absent (`plugins: []`, R32's own declaration) | absent | R32's own (a user-message pad) | the same-machine baseline the whole comparison is subtracted from |
| **M** — the mode arm | loaded | `transform` on every request | **R32's** shape | the mode channel and the locator scan with **nothing to match** |
| **K0** — the shape control | loaded | absent | the **carrier** (the text inside a tool-output node) | the carrier shape's own cost, with the mode closed |
| **K** — the measured arm | loaded | `transform` on every request | the carrier | the transform applied |

- **`K − K0` is the transform's own contribution** (plan, apply, ledger) at a fixed payload shape;
  **`K0 − P` is the carrier shape's own cost**; **`M − P` is what merely asking for the mode costs**.
  No one of those three is allowed to be reported as either of the others, and the report names the two
  rungs it subtracts.
- **The quantity is ADR-029's, unchanged**: `router_overhead_ms`, integer ms, records with
  `upstream_ms: null` excluded, read from the trace. The transform-inclusive half is a second reading of
  the *same* quantity, not a new one, so R32's numbers and R33's add up.
- **The rule set is the repository's own `rules/tool_output.toml`**, loaded by the real assembly from
  `plugins[].config.rules_file` — never a purpose-built stub, and never a rule fabricated to make a
  number look better. The report names the rules that actually fired.
- **`cache_guard` / `strict_prefix` are deliberately not part of the comparison.** A per-node trim
  reports `cache_impact: neutral` by construction, so the guard's verdict cannot differ; loading it
  would add a second plugin to a diff whose only purpose is one variable. The cache regression stays
  where ADR-019 and `CONF-16` put it.
- **The interleaving is part of the method.** For each rung tuple the four assemblies run back to back,
  and the block is repeated; the run order is reported. Machine warmth is the material difference the
  method cannot eliminate, and interleaving is the neutraliser that keeps it from landing on the
  comparison. A reference-vs-R32 gap is reported as **drift**, never as a regression.

### D2. The attribution: what the trace can say about a rule, and what it cannot

Per rung, from the trace's own fields (spec §6 — no new field, no product change): the rule ids that
fired, the edited node addresses, per-node `bytes_in`/`bytes_out`, `tee_id`, and the two label fields
(`verdict`, `cache_impact`). The ledger row is the **audit surface**: a reviewer compares spans, not
documents.

Beyond that, the method estimates a **cost model from four points** — one node / small body, many
nodes / small body, one node / large body, many nodes / large body — of the form
`t ≈ a + b·(node bytes) + c·(nodes)`.

**The confound is part of the decision, not a footnote:** the rule engine's own work is a function of
the **node's text** (line-by-line passes, line-count slicing), so a node that fills the body makes the
body scan and the rule pass grow together, and `b` is **not** separable into "scan" and "rule work" at
those four points. The model predicts *what a rung costs* so a future round can size a rule before
writing it; it never claims to be the scan's own time. **A per-rule *time* is not attributable at all**
from this shape: what is attributable per rule is its *share* of a rung difference, its node count and
its byte arithmetic, and the model's coefficients with the confound stated.

### D3. The noise floor: repetitions, a band, and a citation rule

- **Resolution 1 ms, integer, declared.** A p99 of `0` is reported `<1 ms` and is never a ratio's
  denominator (ADR-029 D1's guard).
- **Repetition.** The load-bearing rungs are run **R = 3** times and reported as a **band** `[min, max]`
  across the repetitions. A rung's *absolute* p99 is citable **only when its band is at most 2 ms
  wide**; otherwise the rung is reported as its band and marked `absolute p99 NOT established`. R = 1
  is permitted only where the rung is a differential rather than a distribution (the gate-quantity
  control, whose raw and derived readings differ by two orders of magnitude) or a binary admission fact
  (the body-bound legs) — and the report states R per rung.
- **A difference is citable only outside the band.** A derived difference (the transform's own
  contribution) may be quoted as a number only when it exceeds the union of the two rungs' bands;
  otherwise the report says **"within the declared band — NOT established"**. This is the rule that
  stops R33 from producing a second unreconcilable `C22`.
- **The ceiling stays a relation, never a rung name** (`R32-2-F3`, confirmed by R32's close-out):
  quote `p99 ≤ 5 × max(p99@C=1, 1 ms)` per rung with **both references visible**.
- **The machine, the commit and the achieved payload bytes are part of every figure** — a figure that
  does not name its machine is not citable, and the round names the **binary it built**, never a
  borrowed commit string.

### D4. The body bound is measured with the transform enabled, because the order is decidable

`server.max_body_bytes` (spec §4.13, ADR-029's landing) sits at the boundary **above** the
transform-mode resolution, so an over-bound request must be refused **before** any plan is computed.
The method includes two legs: a body one byte above the bound with the mode asked (the refusal must be
the vadis's own `413 request_too_large`, carry `details.limit_bytes`/`content_length` and
`X-Vadis-Request-Id`, leave exactly **one pre-pipeline record** whose `transform_mode` is
**`passthrough` despite the header**, and reach the stand-in **zero** times), and the same body under a
raised bound (served **and** transformed, with a ledger row). The refused request's **latency** is not a
rung: a record with `upstream_ms: null` is excluded from the gate's quantity by ADR-029 D1, so a
distribution over refusals could not carry a gate figure. **The refusal is read with a raw-socket
client**: the vadis refuses a declared over-length body without reading it and closes the connection
after the response, so a library client can lose the race and report a reset where the frozen clause
promises a complete `413` with a closed connection.

### D5. What this ADR does not decide: the bar

**No threshold, no band, no envelope value is set here or anywhere else in R33 by a loop card.** The
number the blocking gate compares against is a **human** decision (ADR-012; the loop state record's
waiting-on-human row 1, still open), and ADR-016's registered finding was not that the envelope was
unknown but that it had **no contract home**. R33's cards hand over a **proposal with numbers** — the
threshold's *shape* (an assembly clause, a transform clause and the flatness relation), candidate
values justified by the measured envelope and by the loop charter's declared reference, a recommended
contract home, and the sentence that would go into the specification — marked
`PROPOSAL — pending row 1`. A loop card that writes a bar into spec, DESIGN or an ADR has exceeded its
authority, whichever document it picks.

**The gate keeps exactly one quantity and the reported figure is corroboration only.** `client_elapsed_ms`
is never a gate input (ADR-029 D2), and a difference of two p99s is a *derived* figure whose two
inputs are named.

### D6. Where the numbers live

Per-run raw files under the loop's evidence for that decision (one machine-readable file per rung, the run's own
trace JSONL beside it) and a summary table; the round record; the freeze's decision tables. **Not in
this ADR and not in DESIGN:** DESIGN §12.12 records the *method's* home by pointer and states no
number, and the round's figures are restated by the round record — the same single-source rule
ADR-029 D6 applied, because a second copy of a measurement is a copy that drifts.

### D7. What this ADR does not authorize

Any product change for the sake of measurement (a per-step timing field, a counter inside the engine):
such a change is a spec §6 / product decision, it must be justified as **non-gate and non-observable in
the served bytes** (AGENTS 1's byte boundary, AGENTS 3's observation boundary), and it must land in its
own round with its own red control — the shape frozen by R33 needs none. Also not authorized: any change
to the loop charter's gate definition, to a threshold, to the corpus or to the L1 envelope (ADR-012); any
new Rust dependency (the harness stays Python and drives the real binary as a black box, DESIGN §12.1);
`GET /metrics` (spec §9.3 keeps it planned-not-served); rate limiting; a sharded store; inbound TLS;
and re-adjudicating R32's published numbers, which are quoted as a **prior run** and are neither
re-derived nor re-labelled here.

## Alternatives considered

| Alternative | Why rejected |
|---|---|
| carry R32's own payload shape into the mode-asked arm and call the result "the transform's cost" | the shape has **zero payload nodes**, so the arm measures the mode channel and a scan that finds nothing — the rule path is never entered and the card's own instruction (exercise the real path, not a stub) would be unmet. It is kept as the **`M`** arm, for the one answer it can honestly give |
| run the carrier shape and compare against R32's published numbers (no `K0` arm) | the difference would conflate the payload **shape** with the transform, across two assemblies and two sessions: no subtraction is defined |
| use a realistic multi-tool agent body from the corpus as the carrier | it is the live-run class (keys, a paid upstream) and it breaks the 20 KiB / 200 KiB tuple comparability that is the reason R33 exists |
| a purpose-built stub rule file sized to produce a tidy edit | a rule that exists to make a number is the definition of a measurement bent to its answer; the repository's own rule file is the object whose cost the round exists to report |
| add a µs-resolution field, or a per-step timing field, so the engine's own time is readable | a spec §6 field-group change and a product change; both schema versions stay 2 this round, and the round's purpose is the gate quantity, which is integer-ms by contract |
| a Rust benchmark (`criterion`) over the composition step | a new dependency (DESIGN §12.1) and it bypasses the listener, the store, the trace and the encoding path — precisely the layers whose cost is the question |
| one repetition per rung (R32's own count) | it reproduces the defect that cost R32 two rounds of adjudication (the ceiling's rung name, `C22`'s tail): at 1 ms resolution a single reading is not a figure |
| report a mean/median of repetitions and hide the spread | the spread is the information; hiding it is how an unreconcilable number reaches a report |
| load `cache_guard` with `strict_prefix: true` in the measured assembly | a per-node trim is `neutral` by construction, so the guard cannot change a verdict; it would add a second variable to a one-variable comparison |
| measure only the buffered path (or only the streaming one) | both call the same composition step, but the relay's buffering differs by an order of magnitude at large payloads; a single medium hides a path-specific cost |
| set the ⟨budget⟩ here, from the measured envelope | the gate's threshold is outside the loop's mutable scope (ADR-012) and the envelope's home is an open human row: the loop would be setting the bar it is judged by |

## Rationale

- **A difference of two measurements is only as good as the control between them.** `K0` is the whole
  reason the round can say anything about the transform: without the mode-closed twin of the same
  payload shape, "the transform costs X ms" would silently include the carrier's own cost.
- **Zero nodes is a fact, not an opinion.** The `M` arm exists because the measured premise (P1) makes
  it cheap, and because a reader who wants to know what merely *enabling* the mode costs deserves a
  measured answer rather than a sentence.
- **A band is a claim with its own uncertainty attached.** The integer-millisecond quantity cannot
  support a single-reading claim, and R32's two rounds of adjudication are the evidence. Reporting the
  band costs three runs and removes a whole class of future disagreement.
- **The confound is stated because an unstated one is a lie by omission.** "The rule costs `b` per KiB"
  would be the natural sentence for a reader to write; the method says in advance that the four points
  cannot isolate the scan from the rule's own per-byte work.
- **The bar stays the human's, and the shape of the proposal is where the loop's competence ends.**
  Freezing a quantity, a control, a repetition rule and an attribution model is methodology; choosing
  what "within budget" means for a product is the owner's. This ADR is written so that the decision can
  be made *with* the measurement rather than *instead of* it.

## Consequences

- R33's cards run the four-assembly ladder on one machine and report ADR-029's quantity, the three
  derived differences, the rules that fired, and the bands — with the inert rule of `rules/tool_output.toml`
  declared in the report rather than hidden (registered by R33's freeze as `R33-F1`, with its own owner).
- The round's own premise probe establishes the shape claims at its base (the zero-node reference, the
  carrier's ledger row, the closed-mode byte equality, the four rules' reachability, the bound's
  ordering); the load figures remain `to be measured by R33-2/R33-3`.
- DESIGN §12.12's latency clause stops claiming that the landing round re-measured the budget and points
  here instead; it states no number.
- **Nothing is minted:** no `verified` figure, no price, no saving claim, no threshold, no gate
  definition, no corpus change, and both schema versions stay **2**.
- Honest boundaries carried forward: the measurement is **one machine at one commit**; the quantity is
  integer-millisecond coarse; the transform's cost is a **difference of two p99s** and is `NOT
  established` whenever it sits inside the declared band; the per-rule *time* is not attributable from
  the trace; and the envelope's ⟨budget⟩ remains waiting-on-human row 1.

## Publication note (2026-10-03, R62-2)

The `R<n>` labels and finding ids in this document name iterations of the project's own
private analysis loop — a loop that is not part of this repository, so no label here is
resolvable by a reader of it; they are kept as the provenance of the decision. This
publication pass removed only the dead-pointer class: every reference into that loop's
working tree (its file paths and round-record names, its state record, charter, execution
model and replay contract, its scripts and module names, and the kanban card ids), each
replaced by the neutral phrase the sentence needs. Nothing else moved — no figure,
threshold, `§`/`ADR`/`CONF` id, code sample or contract sentence.
