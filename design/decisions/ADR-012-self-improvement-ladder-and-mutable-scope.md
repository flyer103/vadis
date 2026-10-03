# ADR-012 — the self-improvement ladder: L0 human-gated artifacts, L1 in-envelope self-tuning, L2 agent-composed tier-B plugins, L3 core self-merge refused

- Status: accepted
- Date: 2026-09-19
- Related: ADR-002 (tier-A has no module-level HMR; experimental plugins are tier-B and out of process; config-level coordination), ADR-003 (policy is a revertible, declarative, individually accounted artifact; the verified/inferred convention), ADR-005 (the trace is the only product-analysis-loop interface; research never enters the serving path), ADR-008 (rule files, first-hit override, the inline test as a rule's only spec), ADR-009 (the store and its durability tiers), ADR-010 (the event log as state truth), ADR-011 (the pattern tables are code, not an auto-adoptable artifact), ADR-013 (the online rails that make L1 safe); AGENTS hard constraints 2, 3, 4, 6, 8; spec §1 (non-goals), §4 (the plugin schema), §4.4 (rule files), §6 (observation), §7 (the accounting convention); DESIGN §4 (plugin runtime, realms, `intercept`), §9 (`router replay`), §10 (test strategy), §11 (risks), §12.8 (conformance IDs); the loop charter (mission, roles, gates, direction pool); `book/roadmap.md`

## Background

The loop side has one job (the loop charter): keep iterating the product against measurable goals,
with a human-aligned direction, a serial pipeline, four blocking gates, and a negative result that commits
documentation only. What is stated nowhere is **how far the loop may go on its own**. That is a design
question here for a specific reason: the loop *writes code and config in this repository every round*, and
the gate it is judged by is implemented in the same repository it can edit.

Two failure modes follow, and neither is hypothetical for this project:

1. **The measurement is inside the search space.** A loop that can edit the corpus, the judge or the gate
   can raise its score without improving the product, and the change is invisible because the artifact that
   would have caught it is the artifact that moved. `AGENTS.md` already tells every worker that "gates are
   cheap, wrong-direction rounds are not" — i.e. the loop is *encouraged* to be efficient about gates.
   Efficiency about a movable gate is how a benchmark becomes fiction.
2. **Unbounded automation of a money-and-bytes path.** Both invariants of the serving path are silent when
   broken: a wrong rewrite costs cache (and *raises* cost), and a cost regression looks exactly like a
   normal day. A self-modifying core has no blast-radius limit, because there is no smaller unit to fail.

The assets that make a ladder answerable already exist: tier-B plugins are out of process and therefore
killable (ADR-002); `ctx.isolate` realms and `ctx.intercept` sample/shadow exist in the runtime and are
already named for the A/B and shadow use cases (DESIGN §4); `router replay` computes money on the real code
path rather than a Python re-implementation (DESIGN §9); and the verified/inferred convention (spec §7)
already forbids an inference from entering a gate. What is missing is the ladder's *entry conditions* and
the statement of what may never move.

## Decision

1. **Four levels; each names what may change, its gate, and who promotes.**

   | Level | What may change | Gate | Who promotes |
   |---|---|---|---|
   | **L0** (what exists today) | offline policy artifacts (config values, rule TOML) proposed by the loop and reviewed as ordinary changes | the four blocking gates of the loop charter on a fixed-trace replay | human alignment before execution; reviewer verdict after |
   | **L1** | **parameter and rule switches inside a pre-registered envelope**, adopted by the loop and deployed as a canary | conformance + the fixed-trace replay + the canary's own triggers (ADR-013) | automatic, inside the envelope |
   | **L2** | an agent-**composed tier-B plugin** (out of process, timeout- and crash-isolated) | conformance + the fixed-trace replay + the tier-B isolation requirements | automatic to *canary*; **human to default** |
   | **L3** | the product's core crates | — | **not done**: the loop may write core code (it does today) but may not land it; merge stays human |

   L3 is a refusal stated as a rule rather than an omission: a PR from the loop against `crates/` is an
   ordinary change with an ordinary gate, and the merge button is not in the loop's hands.

2. **The mutable scope is defined by path, and it is smaller than the repository.** The loop may never
   change:

   | Path | What it is | Why it is outside the mutable scope |
   |---|---|---|
   | the loop charter's gate table | the gate definitions | a gate the loop can loosen is not a gate |
   | the corpus | the frozen evaluation corpus | it is the experiment's input; moving it moves the answer |
   | the loop's harness | the evaluator: replay driver, judge, cost report | it is the instrument; an instrument the measured party can re-calibrate measures nothing |
   | `tests/conformance/` | the conformance assertions and their IDs (DESIGN §12.8) | these are the invariants the product is built against; a case that can be edited is survivable |
   | the loop's L1 envelope | the L1 envelope (item 3) | it decides what "good enough to adopt" means |
   | `AGENTS.md`, `docs/spec.md`, `design/` | the contracts the gates are derived from | a gate derived from an editable contract is editable |

   Mutable at **L1**: only the config values and rule switches named by the envelope. Mutable at **L2**
   (human-gated): a new tier-B plugin package. Everything else is L3: a PR, and a human merge.

   The rule is enforced by **construction, not discipline**:
   - the evaluator runs from a **pinned commit** (recorded in the round file), so a verdict names the
     instrument that produced it;
   - the corpus is **content-hashed** into a manifest, and the harness **refuses to produce a verdict** if
     the corpus digest or the conformance ID set has moved since the manifest was frozen;
   - a diff that touches a never-mutable path fails the round regardless of its measured gain.
   - Honest boundary: hashes are **tamper-evident, not tamper-proof** — a loop able to edit both the harness
     and the manifest could conceal the edit. What actually holds the line is the path rule plus independent
     verification (a separate profile re-runs the gate and never reuses the implementer's numbers). The
     digest exists so that "this verdict was produced at commit X over corpus digest D" is always
     answerable, and so an accident is caught loudly rather than argued about.

3. **L1's pre-registered envelope is named, bounded, and owned by the human.** The loop's L1 envelope
   (loop side; **the file does not exist yet** — it lands with the L1 implementation, not with this ADR):

   ```toml
   # Loop-side declaration of what the loop may adopt on its own.
   # Never read by the product (AGENTS constraint 3): the product only ever
   # sees the config or rule artifact this loop writes.
   envelope_version = 1
   min_sample = { turns = 200, sessions = 20 }   # below this, no adoption

   [[param]]
   key   = "cache.breakeven.safety_factor"       # a spec §4 config path
   range = [1.0, 2.0]
   step  = 0.1

   [[param]]
   key   = "plugins.tool-output-rules.config.rules_file.<rule>.max_lines"
   range = [0, 40]
   step  = 1

   # A key absent from this file is not auto-adoptable, whatever the evidence.
   ```

   - Only **numeric or boolean switches inside the declared range** are auto-adoptable. A **new model**, a
     **new plugin**, a **new rule**, a **new config key** (i.e. a schema change) or any change to the
     *shape* of a policy artifact is outside the envelope and goes through the human gate, however large
     its measured gain.
   - The envelope is a **gate definition** (it decides what "good enough to adopt" means), so it is on
     item 2's never-mutable list: changing it is a human decision, and the loop's evidence for changing it
     is a round file, not a self-edit.
   - Adoption is reversible **by construction**: what the loop writes is a config or rule file, so rollback
     is "write the previous artifact" (ADR-003's revertibility), which is also what lets ADR-013's triggers
     run without a human.

4. **The cost gate is a conjunction; "cheaper" alone never passes.** A round's cost claim passes only
   together with fidelity and sampled quality:
   - protocol fidelity (the 3x3 matrix plus the upstream-visible prefix hash) green — blocking, unchanged;
   - `prefix_continuity` not below the baseline — blocking, unchanged;
   - the corpus **frozen and auditable**: same digest as the manifest, and the report names it, with the
     sample size and the window (spec §7's reporting discipline, applied to the loop's own claims);
   - sampled quality: the semantic-corroboration signal (structured-output parse rate, sampled judge
     comparison) must not degrade beyond the declared margin and must state its sample size. It *warns* by
     default (the loop charter), but it becomes **blocking for any round whose claim is "cheaper"**,
     because the mission is to lower cost *without* sacrificing fidelity or reliability — a cost win bought
     with quality is a different product, not a cheaper one.
   - A cheaper number produced by removing work **the corpus does not exercise** is a corpus finding, not a
     win: the response is a corpus change (human, never-mutable), and the round is recorded as such.

5. **Honest boundary: the verifier is empirical.** Replay plus conformance can establish exactly one
   sentence: *"under the frozen corpus and the pinned evaluator, the measured metric moved by X over N
   samples."* It cannot establish "the system is better", and it cannot see a regression the corpus does not
   exercise — the corpus is finite traffic captured on given days, while the real client mix drifts
   (ADR-004's measured statelessness is one client's behaviour).
   - Therefore every self-improvement claim carries (frozen corpus digest, evaluator commit, sample size,
     metric, verified/inferred convention) — the same discipline spec §7 imposes on savings, now imposed on
     the loop's claims about itself.
   - In this project **"self-improvement" always means empirical self-improvement under a frozen
     measurement.** Any text claiming more (autonomy, emergence, "it improves itself") is not supported by
     the mechanism and is not to be written.
   - This is also why L3 stays a refusal: the cheapest way to fake an empirical win is to hold the
     experiment's ground truth, and core self-merge hands over exactly that.

6. **What the ladder is not.** It is not a claim that the loop trains a model (it tunes artifacts, at most
   composing a plugin). It is not a promise of an unsupervised loop (every level above L1 has a human
   promotion step). And it does not replace the direction pool: L1 does not choose *what* to optimize — the
   human-aligned direction does — the envelope bounds only *how far* one direction may move one knob.

## Alternatives considered

| Alternative | Why rejected |
|---|---|
| one level: the loop may do anything the gates pass | the gates live in the repository the loop edits, so "the gate passed" would stop meaning anything |
| keep everything at L0 (a human adopts every number) | bounded numeric knobs with a replay-backed delta are exactly where human attention buys least; refusing to automate them caps the whole loop at the speed of a human reading diffs |
| let the loop also improve the gate/corpus ("it will only improve them") | the measurement becomes part of the search space, and the failure is silent by construction — the one change that can make every later verdict meaningless |
| go straight to L3 with a strong gate | unbounded blast radius on a money-and-bytes path whose failure modes are silent, with the gate's own ground truth inside the loop's write scope |
| enforce the never-mutable list by review convention alone | review is a human step whose cost scales with volume; path plus provenance (pinned evaluator, corpus digest) is cheap to enforce mechanically |
| freeze the corpus forever (never update it) | the real traffic mix drifts; the correct rule is "the round cannot move it", with a corpus update as its own human-owned round |
| make the envelope a product feature (a config section) | it would put loop policy into the serving path and add config surface for something the loop side can hold itself (AGENTS constraint 3) |

## Rationale

- The ladder is defined by **what the human's attention is spent on**, not by technology maturity: L1
  automates the decisions a replay can settle, and keeps the human for the decisions where the question is
  itself a judgement (a new model's quality, a new rule's semantics, the envelope's bounds).
- The never-mutable list is the core of this ADR and is deliberately written as *paths plus provenance*: a
  rule stated as "the loop must not cheat" is unenforceable, while a rule stated as "these paths fail the
  round, and the verdict records its commit and corpus digest" is mechanical.
- Tier-B is the isolation boundary the ladder needs, and ADR-002 gave it for a different reason: an
  out-of-process plugin can be timed out and killed, so a bad L2 candidate degrades to "the candidate does
  not load" instead of "the gateway is broken".
- Putting the ladder in writing now, while every level above L0 is still empty, is what makes L1's `auto`
  a bounded decision rather than a precedent.

## Consequences

- the loop charter gains a pointer to this ADR (its gate table is the L0 gate; the ladder says who may
  move what). The round template gains three fields — evaluator commit, corpus digest, envelope version —
  which is the implementation of item 2's provenance rule.
- the loop's L1 envelope and the corpus manifest are new **loop-side** artifacts; neither is read by the
  product (AGENTS constraint 3).
- **`AGENTS.md` should gain one binding clause** so this rule has the same standing as the other hard
  constraints: *"The gate definitions, the frozen corpus, the conformance assertions and the L1 envelope are
  outside the mutable scope: a change to any of them is a human decision, never a loop outcome; a gate
  verdict records the evaluator commit and the corpus digest it ran against (ADR-012)."* That file is an
  agent-instruction file and is write-protected for a headless worker, so the clause is handed off with
  this round (see the handoff note on the card) rather than edited here. DESIGN §11 carries the risk row in
  the meantime.
- A round proposing an L1 adoption must name the envelope entry it sits inside; a proposal outside the
  envelope is routed to the human gate by construction (the loop cannot adopt it), which also means the
  envelope's coverage is the thing to review when a class of cheap wins keeps landing on the human's desk.
- The L2 promotion path (canary to default) reuses ADR-013's rails unchanged; L2 adds only "the candidate
  is out of process".
- A round that fails because it touched a never-mutable path is recorded like any other negative result
  (round file plus the loop state record), and its measured numbers are kept as evidence — a "win" obtained by moving
  the measurement is exactly the artifact worth keeping visible.
- Not covered here: the tier-B plugin sandbox (syscall/network limits, resource caps) is its own change;
  this ADR fixes the ladder's entry conditions and the mutable scope only.

## Publication note (2026-10-03, R62-2)

The `R<n>` labels and finding ids in this document name iterations of the project's own
private analysis loop — a loop that is not part of this repository, so no label here is
resolvable by a reader of it; they are kept as the provenance of the decision. This
publication pass removed only the dead-pointer class: every reference into that loop's
working tree (its file paths and round-record names, its state record, charter, execution
model and replay contract, its scripts and module names, and the kanban card ids), each
replaced by the neutral phrase the sentence needs. Nothing else moved — no figure,
threshold, `§`/`ADR`/`CONF` id, code sample or contract sentence.
