# ADR-017 — decision models evaluated against this gateway: the candidate surface, the constraint verdicts, and the shadow-evaluation plan (Jev, Laya)

- Status: accepted
- Date: 2026-09-21
- Kind: **evaluation**. Nothing is adopted. No code, no config, no schema and no conformance ID lands
  with this ADR; the deliverable is a verdict per candidate plus an executable offline plan.
- Related: AGENTS 1–6, 8 and 9 (the byte boundary, content determinism, the observation boundary, the
  verified/inferred convention, no fabricated prices, no change-detector tests, docs-before-code, the
  measurement outside the search space); ADR-003 (a revertible, individually accounted transform
  pipeline), ADR-005 (the trace is the only product ↔ autowork channel), ADR-007 (span-faithful
  forwarding), ADR-009 (the store; the write-path latency budget), ADR-010 (the event log is state
  truth), ADR-011 (one classifier; the action set; the pattern tables are code), ADR-012 (the ladder,
  the L1 envelope, the never-mutable paths), ADR-013 (shadow → session-bucketed canary → automatic
  rollback), ADR-014 (plan-first; the upstream is the authority; session stickiness; session-boundary
  probes), ADR-015 (the two permitted byte mutations), **ADR-016 (the primitive register, and the
  decision-provider seam DP-1 with its mode M6)**; spec §1 (non-goals), §3 (selection semantics and the
  reserved `auto` slot), §4.6 (plan-first), §6 (observation), §7 (the accounting convention), §8 (error
  semantics), §9.1/§9.2/§9.3 (the reporting surfaces and the ones deliberately not served);
  DESIGN §5, §6, §9 (`router replay`), §10, §12.3, §12.4, §12.6, §12.8 (the CONF rule), §13 (the
  register, the leaks, the seam's placement); `autowork/program.md` (the gate table, direction D10, the
  cost discipline); `book/roadmap.md`.

## Background

R7-1 (ADR-016) named the system's capabilities and froze **DP-1**, the decision-provider seam: a way to
supply *which candidate route this request takes* from something other than the compiled-in rules,
without touching any primitive's invariant. It also registered **M6 `advised-decision`** — the mode such
a provider would promote into — as *not implemented, contract frozen*, and required any later placement
proposal to satisfy DP-1.1…DP-1.5 first.

This ADR answers the question that seam invites: **can a "decision model" — the class of small,
non-generative, structured-decision models represented by TypeSafe's System One / Jev — be the thing
that sits there, and if not now, what must be measured before it could be?**

Three properties define the class (as documented by the vendor, not measured here):

- **Unordered state in, typed value out with calibrated probabilities.** The output space is a declared
  type (choice / score / calibrated boolean); the model returns a probability per option and a
  confidence. The vendor's claim is *no type errors*: a value the type does not admit is not
  manufacturable.
- **Parallel sampling.** All answers to all questions come back from one call, not one call per question.
- **Generation is abandoned.** The model does not produce strings at all. Its own framing is that this is
  what makes the output safe to consume mechanically.

Two candidates, deliberately of different kinds:

| Candidate | Form | Licence / licence model | Footprint |
|---|---|---|---|
| **Jev** (`jev-1.13.0`, TypeSafe System One) | cloud HTTP API, closed weights | hosted API; price and terms by vendor | none local |
| **Laya** (`convaiinnovations/laya`, `laya 0.3.4`) | local inference, open weights **and** open code | **Apache-2.0** | card-reported: `ModernBERT-large` (395M) fine-tuned plus a from-scratch decision head (2-layer transformer + option-marker scorer + act/escalate head); **measured here: 421.3M params, float32, 3.7 GB peak RSS** |

The vendor also publishes a **methodology** ("workflow evals") that matters more to this repository than
either model does: *assume the code contains the right compute graph; use the strongest model's average
prediction as the **reference probability** — not ground truth — and score the system under test against
it.* The repository has exactly one surface in that shape: the **semantic-corroboration gate**
(`autowork/program.md`), which today has no judge convention and no samples (direction D10's groundwork
is undone).

### The evidence this ADR is written against

Everything below is either **operator-measured** (an independent probe, not this card's work) or
**re-derived by this card** from the operator's raw results. No number is a vendor figure unless it says
so; no model was called by this card; no key was read, printed or stored.

| Artifact | Where | What this card did with it |
|---|---|---|
| 40-fixture set for the **ADR-011 upstream-error-classification task** (`{http_status, response_body}` → `class` (choice, 8 options) + `retry_elsewhere` (calibrated boolean)); human `truth` **with a note per item**; the `truth` is *not* the pattern table | `<home>/bench/decide-bench/fixtures.json`, mirrored at `/tmp/decide_bench/` | read; counted (40 items, exactly 5 per class); verified the scope note: `connect_failure` / `timeout` are excluded because ADR-011 gives their evidence to the transport layer |
| Three-way head-to-head: our hand-written classifier (**real code**, included by `#[path]`), Jev (cloud), Laya (local) | `results.json` (same dirs), driver `run_decide.py` | **re-derived every headline rate** from `results.json` — all match the operator's summary |
| Local feasibility probe: install, weights, device, load time, RSS, latency by question shape | `laya_probe.py`, `laya-probe-full.log` | read; numbers quoted below are the log's |
| The vendor's price | blog `https://typesafe.ai/blog/introducing-system-one-models-and-jev` — **`https://typesafe.ai/pricing` is a 404** | **cited, never measured**: every money figure below is labelled *published-rate derived* and carries that URL + date (AGENTS 5) |
| Laya's model-card self-report (`https://huggingface.co/convaiinnovations/laya`) | read by the operator | **third-party, not re-verified here** (an attempt to fetch the card from this run was blocked); quoted only as the card's own claim |

**Independent re-derivation, this card.** From `results.json` (`n = 40`, one run, one machine — Apple
M4 Pro 48GB, macOS, 2026-09-20):

| Backend | vs human truth | vs our contract | consequence axis (`retry_elsewhere`) | p50 | p95 | ECE(p_correct) | cost / 1000 decisions |
|---|---|---|---|---|---|---|---|
| our hand-written table (real `error_class.rs`) | **29/40 = 72.5%** | — | 34/40 = 85.0% | ~0 ms | ~0 ms | — | 0 |
| **Jev** (cloud) | **40/40 = 100%** | 29/40 = 72.5% | **36/40 = 90.0%** | **280.6 ms** (steady 274.4, min 248.3) | 340.3 ms | **0.05** | **$0.0247** *(published-rate derived)* |
| **Laya** (local, Apache-2.0) | **30/40 = 75.0%** | 21/40 = 52.5% | 24/40 = 60.0% | **90.0 ms** (steady 95.9) | 205.9 ms | 0.132 | $0 (self-hosted) |

Two facts, verified mechanically:

- The 11 misses are `q3 q4 r4 a3 m3 m5 c2 c3 c5 o4 o5`. **Seven of them flip a recovery action** —
  i.e. the class the table chose and the true class differ in `fails_over()` or `demotes_provider()`
  (`crates/router-core/src/error_class.rs:71-83`): `q3` (`auth`→`quota_exhausted`: misses the demotion
  and the plan-first spill), `q4`, `a3`, `m3`, `m5` (`format_error`→something that *does* fail over: no
  failover where one was due) and `c3`, `c5` (`auth`→`content_policy_blocked`: a **paid failover for a
  deterministic refusal**, which ADR-011 item 3 exists to forbid). The other four (`r4`, `c2`, `o4`,
  `o5`) differ in wording only — the recovery action is identical. This reproduces, from the code's own
  predicates, the operator's reading that most of the table's error is routing-relevant.
- **Laya is not "worse at the task" so much as *off-distribution*.** The checkpoint shipped is
  fine-tuned on email-ticket triage; our task is zero-shot transfer. Its own card reports
  zero-sample-family macro accuracy **0.651 / ECE 0.207**; this run measured **75.0% / ECE 0.132** — at
  or above what the card's own zero-sample band predicts, i.e. exactly the behaviour of a model meeting
  an unfamiliar family. The honest sentence is two-part: *as shipped, not suitable for this task; to use
  it on this task it needs fine-tuning or at least evaluation on our own distribution* — which is what
  its card demands and what §7 below sets up.

## Decision

### 1. The verdict, in one table

Five candidates were put to the seam and to the constraints. `auto` and "the classifier" are the two a
reader would propose first, and they land in opposite places — which is the finding.

| # | Candidate surface | Where it would land | AGENTS 1 (bytes) | AGENTS 2 (determinism) | AGENTS 3 (observation) | AGENTS 4 (no unverified savings) | latency (declared p50 15 / p99 50 ms) | verdict **today** | the only admissible form if pursued |
|---|---|---|---|---|---|---|---|---|---|
| ① | **routing selector** (the `auto` slot; choose among candidates) | DP-1 + M6, exactly as designed — the seam between P3 and P4 | holds by construction (DP-1.2) | holds **only** via DP-1.3 (session boundary) | holds (config artifact in, trace out) | holds (a provider returns a route, never a figure) | breaks inline (DP-1.4) | **not adoptable** — spec §1 lists automatic model selection as a non-goal and the `auto` slot does not exist (leak **L4**) | human decision first (make `auto` real or delete it) → then M6, opt-in, owner-declared budget, decision in the trace |
| ② | **upstream error classification** (ADR-011) | **not DP-1, and not at P4's classifier either** (`error_class.rs`); loop-side only | n/a (no bytes) | n/a (post-response) | holds if loop-side | an *inferred* saving only (a failover not taken has no `usage`) | irrelevant if offline | **admissible offline only** | an offline oracle whose output a **human compiles into a table row** (ADR-011 item 11 keeps the tables code → this is L3: written by the loop, merged by a human) |
| ③ | **guard / validation** — fill the semantic gate's missing judge samples with **reference probabilities** | **no primitive**: it is W5's loop-side artifact, outside the product | n/a | n/a | holds (loop-side; the gate definition stays human-owned) | holds (a probability is not a money figure) | irrelevant (offline) | **suitable, loop-side only** | the D10 groundwork: a frozen sample set + a judge convention + declared budget; **never** read by the product (AGENTS 3) |
| ④ | **transform admission** (the P6 token-saving pipeline) | **P6** — which has **no implementation** | **fails** if the model rewrites content | **fails** if a transform's output is a model's | holds | **fails** (a saving the model asserts is unmeasured) | breaks inline on every request | **not suitable inline** | offline **rule proposal**: the model proposes TOML rules; a human merges them; the deterministic engine applies them (ADR-008's first-hit override, the inline test as the rule's only spec) |
| ⑤ | **budget / spill and cache decisions** | **P4's predicate / M2 (plan-first)** | n/a | n/a | holds | holds | n/a | **refused structurally** | none: ADR-014 item 2 (only upstream evidence may move the account), item 1 (session stickiness; no per-request account choice), ADR-013 item 2 (no mid-session exploration). A model's opinion may not move the account. |

**The seam's correspondence to R7-1, restated as a single rule.** The table's latency column is the
**declared** envelope (`p50 15 ms / p99 50 ms`), not a contract — §3 states its provenance and why it may
not be cited as ADR-009's. DP-1 admits an advisor that answers
*which of these legitimate candidate routes this request takes*, at a session boundary, from
**structure only** (DP-1.2: protocol, byte lengths, session/turn, candidate routes, projections — no
message text), with a required budget (DP-1.4) and a recorded identity (DP-1.5). Candidate ① is that,
exactly. Candidate ② — the one with the strongest measured result — **is not expressible on it**: DP-1
sits between P3 and P4, *before* the request is sent, while error classification runs *after* the
response arrives, and DP-1.2's `DecisionRequest` carries no response body. So the best-measured
candidate is the one the frozen seam cannot host, and the candidate the seam can host is the one spec §1
excludes. That is not a defect in either; it is the boundary of what this ADR is allowed to conclude
from.

### 2. Capability matrix: cloud API vs self-hosted, per dimension

Every cell is a verdict with its evidence. "Suitable" always means *for this gateway*, never *in
general*.

| Dimension | Jev (cloud API) | Laya (local, self-hosted) | Which is suitable, and why |
|---|---|---|---|
| **Latency, our question shape** | p50 280.6 ms / p95 340.3 ms; variance from the network | p50 **31.7 ms** for 1 question (30.3–32.5, one 684.2 ms outlier), **56.4/64.4 ms** for 3 questions, **90.0 ms** for 2 questions over 8 options + long criteria | **neither, inline.** Both exceed the p50 budget; both are usable offline (§3) |
| **Latency predictability** | cloud RTT; no local control | σ ≈ 0.4 ms across 7 runs, 62.3–66.2 ms; one **684 ms** outlier in a 7-run sample; bench p95 205.9 ms (n=40) | **Laya**, for a *declarable* budget — but the declared budget needs the p99 story, and the outlier plus the bench p95 *are* the p99 story |
| **Cost** | **$0.0247 per 1000 decisions** *(published-rate derived: 587.4 input tok/call at $0.042/MTok, output free; source URL + date recorded)* | $0 marginal; costs 3.7 GB resident RAM and ~2.2 GB of weights | **Laya**, for a resident process; **Jev**, if nothing may be resident. Cost is *not* the deciding axis at this volume (a full 40-item run cost $0.000987) |
| **Egress (does user-controllable content leave the machine?)** | **yes** — and for the classifier task the *state is the upstream's response body*, which can quote what the user sent (a content-policy refusal names the offending content; a 400 may echo the prompt) | **no** — weights and inference are local; zero egress by construction | **Laya**, decisively, and as a **first-class** judgement: this gateway sees user content |
| **Offline / air-gapped** | needs the network per decision | runs with the network down (weights fetched once) | **Laya** |
| **Auditability** | closed weights, vendor retention/ToS not stated on the pricing page (404); the only public figure is a blog post | Apache-2.0 **weights and code**; the decision is reproducible on the operator's own hardware | **Laya** |
| **Input budget** | no published per-question cap observed in this run (587.4 tok/call used) | **512 tokens per question** (question + options + state), truncated beyond | neither is unlimited; a state description must be size-bounded and the truncation must be visible (our classifier state fits: 313.9 tok/call) |
| **Language** | vendor does not state a language restriction in the material read * | **English only** (model card) | a boundary to state, not a blocker: our state for classification is provider error text (mostly English); a **Chinese-prompt state would degrade** |
| **Deployment shape** | nothing resident; per-call network dependency | **resident sidecar only**: load 21.9–26.2 s cold, **3.7 GB peak RSS**, `mps` device on Apple silicon, no CUDA needed | Laya is affordable *only* as a long-lived process — never load-per-request |
| **Calibration (reported)** | ECE(p_correct) **0.05**, mean probability on chosen 0.95, n=40 | ECE(p_correct) **0.132** (and 0.309 on p(truth)) | **Jev**, on this task — with the caveat that 40 samples make ECE coarse (§7.4) |
| **Determinism / reproducibility** | a closed model over a network: pinning "the same answer twice" is not ours to guarantee | local, versioned, but still stochastic in principle | **neither, as-is**: reproducibility is a *seam requirement* (DP-1.5), so it must be recorded, not assumed — and `router replay` is **not served** (spec §9.3) |
| **Scope fit (measured)** | 40/40 classes, **but 4/40 wrong on the consequence axis, systematically** | 30/40 classes — and **9 of its 10 class errors are one collapse**: it answers `server_error` 14 times where the truth holds 5 — and 16/40 wrong on the consequence axis | **Jev, for the class question only** — see §5's class/consequence rule |

\* not verified here; recorded as "not stated in the material read", never as "no restriction".

**The vendor claim, tested.** "No type errors" holds in this run in the narrow sense it claims: Jev
returned a member of the declared 8-way choice for all 40 items, no free text, no parse failure, all
answers in one call per item. That is a real property and it is why this class of model is *consumable*
by code. It is not the property that matters most here, and §5 is why.

### 3. The latency arithmetic, stated explicitly

The conflict the card demands be resolved is: **a hot path whose declared budget is p99 < 50 ms, against
models whose end-to-end latency is 70–500 ms (vendor) / 31.7–90 ms (our own measurements of Laya).**

First, the honest provenance of that budget, because it is load-bearing:

- The envelope `p50 < 15 ms / p99 < 50 ms` is **cited as ADR-009's** in R2/R3/R4/R5's round records and in
  `autowork/STATE.md:149`, but **ADR-009 contains no such numbers** (its own text says the write-path
  budget must name *measured* numbers, and "Re-measurement is owed"). ADR-016's DP-1.4 registered this as
  a finding and required the envelope be given a contract home by a human decision. **This ADR therefore
  does not cite it as an existing contract.** It writes the envelope as **"declared value + measured
  value"**: declared `p50 15 ms / p99 50 ms`; measured at R4, router's own overhead `p50 1 ms / p99 6 ms /
  max 6 ms` over 61 requests (`result.overhead_ms`, `autowork/STATE.md:147-149`; spec §9.2 fixes that this
  figure *excludes* `upstream_ms`).
- That exclusion matters: an advisor's wait is **not** an upstream forward. Under spec §9.2's own
  convention it is **router's own work**, so it lands inside the envelope rather than beside it.

Now the arithmetic. `p50` for a path that consults an advisor is at least `1 ms + the advisor's p50`:

| Advisor | advisor p50 (measured) | path p50 becomes (1 ms + p50) | vs declared p50 = 15 ms | advisor p95 (measured) | vs declared p99 = 50 ms |
|---|---|---|---|---|---|
| Laya, 1 question (probe, 7 runs) | **31.7 ms** | **32.7 ms** | **2.2×** | not estimable from 7 runs (one 684 ms outlier in the sample) | — |
| Laya, 3 questions (probe, 7 runs) | 64.4 ms (56.4–64.4 across two processes) | 65.4 ms | 4.4× | not estimable from 7 runs | — |
| Laya, 2 questions / 8 options (bench, n = 40) | 90.0 ms | 91.0 ms | 6.1× | **205.9 ms** | **4.1×** |
| Jev (bench, n = 40) | **280.6 ms** | **281.6 ms** | **18.8×** | **340.3 ms** | **6.8×** |

Two conclusions, and the second is the one that gets forgotten:

1. **Inline use is a different budget, not a slower one.** Every candidate breaks the *declared* p50 by
   at least 2.2×; on our task's question shape (2 questions, 8 options) Laya's measured p95 (205.9 ms) is
   4.1× the declared p99, and the cloud candidate's p50 alone is 18.8× it. So the default verdict for
   every candidate is **not suitable inline**.
2. **The budget is not ours to move.** "Use it inline and accept slower" is not a design decision this
   ADR may take: the envelope decides what "the round passed" means, which AGENTS 9 / ADR-012 put
   outside any loop's reach, and DP-1.4 already requires an operator-stated `timeout_ms` with **no
   default** precisely because the envelope has no contract home. An inline advisor is therefore
   admissible **only** as *opt-in + a separately owner-approved decision-latency budget + the decision
   written into the trace* — otherwise the only sanctioned form is **offline comparison over a fixed
   set** (§7).
3. **A latency budget cannot be stated without the question shape.** The same model answers 1 question in
   31.7 ms, 3 in 56.4–64.4 ms, 2 long-criteria questions in 90.0 ms, and 10 states × 3 questions in
   290–432 ms (≈ 29–43 ms/state, batched). Any `timeout_ms` is therefore a function of (questions per
   call, options per question, state size) and must be recorded *with* those dimensions, not as a bare
   number.

### 4. Egress and compliance (a first-class judgement, not a footnote)

- **The state is not abstract.** For candidate ② the state is `{http_status, response_body}` — the
  upstream's own error text. Providers quote the offending input in refusals and echo request fragments
  in 400s, so the state of this task can contain **user content**. A cloud advisor therefore sends user
  content to a third party; a local one does not.
- **The local model's zero egress is an architectural advantage for *this* product.** The gateway is
  local-first by construction (AGENTS' own framing: clients point their `base_url` here; the store is one
  local file; the operator is the only user — spec §1's single-operator non-goal). Introducing the first
  off-machine dependency *into the decision path* would be a property change, and it would be introduced
  by the component whose only output is a routing opinion.
- **Stated precisely, to avoid over-claiming:** this is **not** an AGENTS constraint violation — AGENTS 1
  and 2 govern request bytes and content determinism, AGENTS 3 governs the *repository* boundary between
  product and loop, and none of them forbids a product-side network call (the product calls upstreams by
  definition). It is an operator-level property that the ADR records as a decision input, and it is why
  the local candidate stays on the table despite scoring lower on the classification task.
- **Auditability.** Laya's weights and code are Apache-2.0 and its inference is reproducible on the
  operator's hardware; Jev's terms, retention and price page were not obtainable for this evaluation
  (`https://typesafe.ai/pricing` is a 404 — the price is a blog statement). For a component that would
  see error text, "auditable by the operator" is a criterion, not a nice-to-have.
- **What is *not* claimed:** that a local model is safe, private in some deeper sense, or free of
  operator-side risk. The claim is only: its egress is zero by construction.

### 5. The determinism boundary: what a model may be asked, and what it may never do

| May be asked | Evidence / contract |
|---|---|
| Classify, score, choose among declared options, abstain | the class/consequence rule below; DP-1.1's bounded advice |
| Decide **which of the already-legitimate candidate routes** a request takes, at a session boundary | ADR-016 DP-1.1/DP-1.3 |
| Produce a **probability** used as a *reference* offline | §7.8's method |
| **Never**: rewrite content (prompt, messages, order, tool schemas) | AGENTS 1 + 2; DP-1.2's "a provider never touches bytes" |
| **Never**: produce a money figure, a token figure, or an estimate that could be read as one | AGENTS 4; P8 owns every figure |
| **Never**: influence outbound bytes without the effect being the *recorded* route decision | AGENTS 1 + 2; DP-1.5 |
| **Never**: act on arithmetic, date comparison, counting, or multi-hop table lookup | constraint 2's spirit, and **Laya's own model card** states that its calibration is measured on its benchmark set and that arithmetic/counting/date comparisons must stay in deterministic code — quoted as the vendor's limitation, which happens to coincide with our constraint |

**The class/consequence rule — the most portable finding in this ADR, and it is measured, not argued.**

Jev answered **every one of the 40 classes correctly** (40/40) while getting the consequence question
(`retry_elsewhere`) wrong **4/40** — and wrong the same way every time: `q4` (quota exhausted), `a2` and
`a4` (auth), `m4` (model not found). In each case it answered *"a different provider would not change the
outcome"*, which contradicts ADR-011's premise: another provider has its **own** credentials, its own
allowance and its own roster, so these are exactly the failures a failover exists for. A model can know
what happened and still be wrong about what to do with it.

Therefore:

> **The class (what happened) may come from a model. The consequence (what to do) is derived by code
> from the class.** `ErrorClass::fails_over()` and `ErrorClass::demotes_provider()`
> (`crates/router-core/src/error_class.rs:71-83`) are the consequence function, they are `const fn`s,
> and a calibrated boolean from any model is **evidence about a class, never an authorization to act.**

This rule is what makes candidate ② usable at all: the model's 4/40 consequence errors are *discarded by
construction* (we take the class, we compute the action), while its 40/40 classes are exactly the input
the table needs. It also converts the vendor's headline ("no type errors") into the question that
matters: **typed ≠ correct, and correct-class ≠ correct-action.**

### 6. The candidates, one at a time

#### ① Routing selector (`auto`) — the seam's intended guest
*Where it lands:* DP-1 exactly, promoting to M6. Nothing in this ADR contradicts ADR-016's placement.
*Blocked by two human decisions, not by measurement:* spec §1 lists **automatic model selection** as a
v0.1 non-goal, and the `auto` slot is documented-but-unreachable — leak **L4** (`forward.rs:403-407` is a
literal comparison; `trait Selector` does not exist). ADR-016 leaves "keep and define, or delete" to a
human.
*Constraint verdict:* holds on 1/2/3/4 by construction (DP-1.2, DP-1.3, config-in-trace-out, route
rather than figure). **Latency: fails inline by default** (§3).
*Measurement available today:* none for *this* question. The bench measured classification, not route
choice; a selector evaluation needs the D10 evaluation set (`program.md`), which does not exist.
*Verdict:* **not adoptable now**; the path is: human decides L4 → a frozen evaluation set → an offline
comparison → opt-in M6 with a declared budget. Note honestly that a *good* selector would not be
exercised by a 40-item classification set at all; it needs session-shaped traffic with the candidates
the roster actually has.

#### ② Upstream error classification (ADR-011) — the strongest result, at the wrong seam
*Where it lands:* **neither** DP-1 nor the classifier. Not DP-1 (pre-request placement; DP-1.2's
structure-only input carries no response body). Not the classifier either: ADR-011 item 11 rules the
pattern tables **code, not an auto-adoptable artifact**, and ADR-016 item 4 restates the consequence
("a provider *consumes* classes, it does not contribute patterns").
*What the measurement actually shows, and how to use it:* the table is **72.5%** against human truth and
**7 of its 11 misses change a recovery action** (verified above). Jev's classes are **100%** on the same
40 items. That is a strong signal that the tables can be improved — and the correct use of it is the
**L0/L3 route**: the model produces classes *offline*, a human turns the disagreement pattern into a
table row, the table stays code, and the resulting change is measured by the ordinary path (verified
cost/fidelity gates) rather than trusted from the model's own report.
*What must not happen:* wiring the model's *consequence* answer into retry behaviour. The 4/40
systematic error (§5) is exactly the failure mode ADR-011 item 3 and item 4 were written to prevent, and
a 100%-class score would have made it look safe.
*Verdict:* **admissible offline only** — the highest-value offline use in this ADR, and the cheapest
(§7.8's budget arithmetic).

#### ③ Guard / validation — the semantic gate's missing judge samples
*The gap:* `autowork/program.md`'s semantic-corroboration gate is *"structured-output parse rate, sampled
judge comparison"*, warning-grade; **no judge convention and no samples exist** (D10's groundwork is a
direction, not an artifact), and ADR-012 item 4 makes this signal **blocking for any round whose claim is
"cheaper"** — so a cost claim today rests on a gate that cannot currently be evaluated.
*The fit:* System One-class models return calibrated probabilities over declared options, which is
precisely the input a reference-probability instrument needs. The vendor's own methodology says what to do
with them (§7.8).
*Boundaries:* loop-side only (AGENTS 3: research never enters the serving path; the product never reads
`autowork/`); the **gate definition and the corpus are human-owned** (AGENTS 9, ADR-012 item 2), so this
adds *samples and a convention*, never a threshold; and the reference is **not ground truth** — treating
it as such is precisely the failure ADR-012 item 5 warns about ("the verifier is empirical").
*Verdict:* **suitable, loop-side, warning-grade.** The cheapest useful thing in this ADR.

#### ④ Transform admission (P6) — refused inline, useful as a rule proposal
*Where it lands:* P6 `transform-chain`, which is **contract-only** (`router-plugins/src/lib.rs:1-4` is a
stub; every record carries `transforms: Vec::new()`). A shadow of a candidate transform needs P9
(`ctx.isolate`/`ctx.intercept`), also contract-only — M3's own dependency (ADR-016 item 5).
*Why inline fails, on three axes at once:* (a) it sits on **every** request, so §3's arithmetic applies
to the full traffic rather than to session boundaries; (b) an unmeasured saving asserted by a model is
**constraint 4's** exact prohibition; (c) a model whose output *is* the content change is a rewrite path
— constraints 1 and 2's target.
*The admissible form:* the model proposes a **rule** (TOML, per ADR-008: first-hit override, an inline
test as the rule's only spec); a human merges it; the deterministic engine applies it; the ledger labels
the result `verified` or `inferred` per spec §7. This is exactly W2's honest note in ADR-016 ("no
transform contributes today").
*Verdict:* **not suitable inline; suitable as an offline rule proposer**, once P6 exists to apply what it
proposes.

#### ⑤ Budget / spill and cache decisions — refused structurally
- ADR-014 item 2: **the upstream's word is the authority; the local counter is a warning.** A state
  transition may only follow upstream evidence. A model's opinion is not upstream evidence.
- ADR-014 item 1: the account is **sticky per session**; a per-request decision between accounts is
  forbidden because it re-prefills the conversation at the miss price.
- ADR-013 item 2: **no mid-session exploration**; the unit is the session, and assignment is a pure
  function of `(session key, salt)`, not of a model's output.
- ADR-012 item 3: the L1 envelope's bounds are a human decision, so a model may not propose them either.
*Verdict:* **refused in every form that acts.** The offline form has nothing left to optimize that L0/L1
does not already own.

### 7. The shadow-evaluation plan (executable to "the next round's first step")

#### 7.0 What exists, and what is missing — the honest inventory
- **`autowork/traces/` does not exist in this worktree**, and neither does `autowork/corpus/` — `.gitignore`
  names them (`autowork/traces/`, `autowork/results/`, `autowork/corpus/raw/`) and no capture round's output
  is in the tree. `router replay` — the same-code-path replay DESIGN §9 specifies — is **not served** in
  v0.1 (spec §9.3: the CLI accepts `serve` and `stats` only). DP-1.5's reproducibility test therefore has a
  prerequisite that does not exist yet, and this plan must not assume it.
- **M3 `shadow` is contract-only** (it composes P9, ADR-016 item 5). So the "shadow" this plan uses is
  **loop-side**: re-decide recorded states offline and compare — not an in-process isolated realm. When
  P9 lands, the in-process shadow becomes available *and* strictly more informative; until then the
  offline form is the only truthful one.
- The bench corpus lives **outside the repository** (`<home>/bench/decide-bench/`, mirrored to the scratch
  dir `/tmp/decide_bench/`) with no manifest and no digest, and the Rust baseline is included by an
  **absolute machine path** (`#[path = "<home>/…/crates/router-core/src/error_class.rs"]`) —
  re-checked by this card: that file and this branch's copy differ in **one comment line only** (R6's
  vocabulary sweep), so the baseline *is* HEAD-equivalent in behaviour, but nothing in the artifact
  proves that. Both facts must be fixed before any of it can be evidence (ADR-012 item 2: a frozen
  corpus, a manifest, and a verdict that names the evaluator commit).
- **No real traffic is in this evaluation:** all 40 items are hand-authored. They are representative by
  construction (5 per class, with adversarial wording the table misses), not by sampling — which is why
  §7.4's interval arithmetic, not the item count, bounds what may be claimed.

#### 7.1 Step 0 — freeze the evaluation set as an L0 artifact
Lands in the repository, loop-side, product-invisible (AGENTS 3):
- `autowork/corpus/decision-eval/manifest.toml` — corpus digest, item count, the scope note (what task
  is being measured), the exclusion note (`connect_failure`/`timeout`, per ADR-011 item 1: the transport
  supplies that evidence and the classifier never guesses it from text — a **scope limit, not a gap**),
  and the labelling rule.
- one JSONL item per line: `{id, task, http_status, response_body, truth_class, truth_consequence,
  truth_note, source}` — `source` ∈ {hand-authored, captured}. The 40 existing fixtures are **seed 0**,
  verbatim, keeping their per-item `note`.
- `autowork/corpus/` is on ADR-012 item 2's never-mutable list: the loop may **not** move it. Growing it
  is a human-owned round (and, when it grows, the manifest digest changes *with* it, in that round).

#### 7.2 Step 1 — make the harness re-runnable and pinned
- Derive every path from the repository root; **no absolute scratch paths**, no `/tmp` mirror.
- Pin the Rust baseline by **content digest** (or by including the crate), not by an absolute path, and
  record which revision it was read at.
- Record the **evaluator commit** in every result file (ADR-012 item 2).
- Keep the secret discipline the existing driver already demonstrates: the key comes from the process
  environment, is never printed, never logged, never written into an artifact.

#### 7.3 Step 2 — metrics, each with its convention
| Metric | Definition | Convention |
|---|---|---|
| `accuracy_vs_human_truth` | class agreement with the human labels, per class and overall | **count** (must state `n`) |
| `agreement_with_our_contract` | class agreement with the shipped classifier's output | **count** — note this is *not* accuracy; where the table is wrong, agreeing with it is a miss |
| `consequence_axis_accuracy` | agreement on the action axis, compared against `fails_over`/`demotes_provider` derived from the class | **count**; the model's own answer on this axis is recorded but **never** an input to a decision (§5) |
| `routing_relevant_misses_fixed` | of the table's misses whose class implies a different recovery action, how many the candidate gets right | count |
| `regressions` | items the table gets right that the candidate gets wrong | count — a candidate with a high accuracy and a non-empty regression list is not adoptable |
| `ECE(p_correct)` | expected calibration error over declared bins | **inferred**-class diagnostic; bins and `n` stated; below ~150 items a 5-bin ECE is too coarse to read (§7.4) |
| `latency p50/p95` | wall time per call | **measured**, and always reported *with* the question shape (questions, options, state tokens) — see §3, conclusion 3 |
| `cost per 1000 decisions` | input tokens × published rate | **published-rate derived** — source URL + date in the artifact; never presented as a measured bill (AGENTS 5) |
| `egress` | whether user-controllable content left the machine, and how many bytes | a **property**, stated per backend |

#### 7.4 Step 3 — sample size, stated before any claim
`n = 40` supports **directional** readings only. Concretely: the table's 29/40 has a 95% Wilson interval
of **[57.2%, 83.9%]** (a plain Wald interval is ±13.8 pp) — one item moves the headline 2.5 pp. To claim
"better" with 80% power at α = 0.05 against a 72.5% baseline the set needs roughly **110 items for a
+15 pp claim, 270 for +10 pp, 1173 for +5 pp** (two-proportion arithmetic, computed for this ADR). Until
the set is a few hundred items, the honest wording is **"the direction and the mechanism"**, never "the
model is better".

#### 7.5 Step 4 — the decision rule for a proposal (warning-grade; a human still merges)
An offline candidate becomes a *proposal* only if **all** hold: it beats the baseline by a declared
margin **and** fixes at least one class whose recovery action differs **and** introduces no regression on
items the table already gets right **and** is expressible as a table row a human can write under
ADR-011's rules. The output of the whole exercise is a **diff-shaped proposal**, not a runtime dependency.

#### 7.6 Step 5 — how it enters ADR-012's ladder (and why not L1)
- **L0 is the entry point, and it is the only available one:** offline artifacts (a frozen evaluation set,
  a report, a proposed table row) proposed by the loop and reviewed as an ordinary change.
- **L1 is closed to this, by ADR-012 item 3's own text:** auto-adoption covers *numeric or boolean
  switches inside the declared range*; a **new model, new plugin, new rule or new config key** is outside
  the envelope "however large its measured gain". A decision model is all four.
- **A future inline advisor is M6 + a human decision**, per ADR-016: a new config block
  (`decision_provider{kind,timeout_ms,admit}`) with a **required, owner-stated** timeout, opt-in, and the
  decision recorded through fields that already exist. Only *that* timeout value could later live inside
  an envelope.
- **M4's canary rails need P9** (ADR-016 item 5), so there is no automatic canary path for an advisor
  today even if it were adopted.

#### 7.7 Step 6 — failure, rollback, and why an offline shadow cannot be `verified`
- **Offline:** the failure mode is "the proposal is rejected", the rollback is "discard the artifact" —
  nothing in the product changed (ADR-003's revertibility applied to a loop-side artifact).
- **If an advisor is ever installed inline:** its absence restores today's behaviour bit-for-bit
  (ADR-016's config shape), every failure is `Abstain` → the deterministic rules answer (DP-1.4), and the
  rollback is "remove the config block" plus ADR-013 item 4's triggers once it is canaried (which need
  ADR-012 item 3's minimum sample: 200 turns / 20 sessions — below that, "inconclusive" is neither pass
  nor fail).
- **The convention trap, stated so nobody trips on it later:** the loop-side shadow's *cost* column can
  never be `verified`. Nothing is sent upstream, so there is no `usage` to measure — exactly ADR-013 item
  1's reasoning about M3. An offline shadow is a **correctness and divergence filter**, and its money
  figures are `inferred` and gate-inadmissible (AGENTS 4). The *verified* effect of any proposal arrives
  only after adoption, through the ordinary path.

#### 7.8 Step 7 — the vendor's workflow-eval method, applied to our semantic gate
What we take, in their words and our terms: **assume the code holds the right compute graph** (for us:
the decision chain is P3/P4 — routing and its predicate are the graph under test); **use the strongest
model's average prediction as the reference probability**, over `k` repeats of the same state, to get
`p_ref(item, option)`; **score the system under test against `p_ref`** with a proper scoring rule
(Brier / log-loss) plus ECE, and **not** with accuracy alone; treat the items where our decision
diverges from the reference as candidates for a rule change (L0) or as findings about the corpus.

What we refuse: calling `p_ref` ground truth. It is an instrument reading — "the best available model's
expected answer" — and ADR-012 item 5's sentence applies verbatim: replay plus a judge can establish
only *"under the frozen corpus and the pinned evaluator, the metric moved by X over N samples"*, and a
reference distribution is not an exception to that. The gate also stays **warning-grade** until a human
moves it (the gate table and the corpus are on the never-mutable list: AGENTS 9, ADR-012 item 2), and it
is the *only* signal that can make a "cheaper" claim blocking (ADR-012 item 4) — which is the real
reason to build it.

Budget, declared in advance per `program.md`'s cost discipline: the operator's full 40-item two-backend
run is measured at **$0.000987** (Jev, published-rate derived; 23,496 input tokens over 40 calls at
587.4 tok/call). The reference-probability method multiplies by `k` repeats: `k = 5` × 40 items = 200
calls ≈ 117,480 tokens ≈ **$0.0049**. The constraint on this work is sample size and labelling, not money.

#### 7.9 The next round's first step, as a write-set
One card, docs+harness only, no product change:
`autowork/corpus/decision-eval/{manifest.toml,items.jsonl}` (the 40 fixtures as seed 0, plus the digest
and the conventions of §7.3), the harness fix of §7.2, and a declared budget for the first paid run.
Nothing in `crates/`, `docs/spec.md` or `design/` moves; no CONF id is taken.

### 8. The not-doable list

| Not doable — ever, under this ADR | Why |
|---|---|
| Let a model rewrite prompt content, reorder messages, or touch tool schemas | AGENTS 1 + 2 (the byte boundary and content determinism) |
| Let a model influence outbound bytes without that influence being the recorded route decision | AGENTS 1; DP-1.5 (replay would be a re-roll, not a measurement) |
| Let a model produce a money or token figure, or turn an inferred number into `verified` | AGENTS 4; P8 owns every figure |
| Wire a model's *consequence* answer (`noul`) straight into a retry/refusal decision | measured: 4/40 systematic errors on exactly the account/credential cases a failover exists for (§5) |
| Let a model move the account (spill / return) | ADR-014 item 2 (upstream evidence only), item 1 (stickiness) |
| Let a model explore mid-session, or advise per request | ADR-013 item 2; DP-1.3 |
| Read `autowork/` from the serving path, or write the product from the loop except via an artifact | AGENTS 3 |
| Move the gate, the corpus, the conformance assertions or the envelope | AGENTS 9; ADR-012 item 2 |
| Allocate a CONF id, a new trace field, a new event kind, a new `error.type`, or a new pipeline stage for an advisor | ADR-016 item 4's refuse-list; DESIGN §12.8 (IDs are a human decision) |
| Quote Laya's model-card numbers as *our* measurements, or the blog price as a *bill* | AGENTS 5; §7.3's cost convention |
| Claim M3 `shadow` exists, or that the product can replay an advisor today | ADR-016 item 5 (P9 contract-only); spec §9.3 (`router replay` not served) |
| Present the offline comparison's money numbers as `verified` | no upstream `usage` exists; ADR-013 item 1's reasoning |

## Alternatives considered

| Alternative | Why rejected |
|---|---|
| **Adopt Jev inline for the classifier now** (it scored 40/40) | It cannot sit at DP-1 (post-response task, structure-only input) and cannot replace the classifier's tables (ADR-011 item 11). Its score is a reason to improve the table **offline**, not to put a network call on the failure path — and its 4/40 consequence errors are exactly what a naive wiring would act on |
| **Adopt Jev inline for routing now** | spec §1's non-goal holds, the `auto` slot is unreachable (L4), and inline use needs an owner-approved decision-latency budget that does not exist while the declared envelope has no contract home (DP-1.4) |
| **Adopt Laya inline** (it is local, cheap, fast and open) | §3: 31.7 ms single-question p50 already doubles the declared p50 bound, and 90.0 ms on our task's question shape breaks the p99 bound too. Fast for a model is not fast enough for a hot path. It also scored *below* the table on this task (the evidence section above) |
| **Adopt Laya as the sanctioned local decision model anyway, on egress grounds** | Egress is a strong argument for the *local form*, not for *this checkpoint on this task*: 75.0% classes and 52.5% contract agreement is off-distribution behaviour (above), and using it as the classifier's oracle would import a collapse onto `server_error` — 14 predictions against a truth of 5 (§2) |
| **Build M6 and a real `DecisionProvider` now** | ADR-016's own alternative table rejected this: the interface is frozen before the fan-out, and DP-1.3's admission rule exists precisely to stop the per-request flip. Nothing is measured yet that would justify the code |
| **Let a model write transforms directly (P6)** | constraints 1 + 2 + 4 at once: a rewrite path, a non-pure transform, and an unmeasured saving. The admissible form is a proposed rule a human merges |
| **Let a model own spill/return (M2)** | ADR-014 item 2: only upstream evidence may move the account. A model's opinion is not evidence |
| **Treat the 40-fixture set as the gate corpus as-is** | it lives outside the repository with no manifest or digest, its baseline is included by an absolute machine path, and 40 items cannot support a "better" claim (±13.8 pp Wald; ~1173 items for a +5 pp claim). It is the **seed**, not the corpus |
| **Skip the offline step and go straight to a canary** | M4's rails need P9 (contract-only), and there is no envelope entry for a model (ADR-012 item 3) — the canary would have no legal instrument |
| **Use the vendor's price page / the model card as evidence** | the price page is a 404 and the figure is a blog statement (labelled *published-rate derived*); the model card is third-party and was not re-verifiable from this run. Constraint 5 forbids entering either as a measured fact |
| **Write the envelope's contract home here** | it is a human decision (AGENTS 9; ADR-016 DP-1.4's registered finding). This ADR writes "declared + measured" and leaves the envelope where it is |

## Rationale

- **The seam is real; its guest is not, yet.** ADR-016's value was making the question landable. The
  measurement now says something uncomfortable and specific: the candidate with the best numbers sits
  where the seam cannot reach, and the candidate the seam can host is excluded by spec §1 until a human
  moves it. Recording that is more useful than either "we should adopt a model" or "we should not".
- **The class/consequence split is the finding that survives the vendor.** It came out of a 40-item
  measurement, it is grounded in two `const fn`s already in the code, and it is falsifiable: any future
  candidate that gets classes right and consequences wrong is handled by it without re-litigating.
- **Offline-only is not a demotion.** AGENTS 3 already puts research outside the serving path; an oracle
  that writes a table row a human merges is the same shape as the L0 artifacts the loop already
  produces. The difference between "a model in the decision path" and "a model that improves the tables"
  is the difference between an unbounded dependency and an ordinary diff.
- **Egress belongs in the design, not in an appendix,** because this is the one product in the repository
  whose input *is* user content, and because the cheapest, fastest, most auditable candidate is also the
  only one that keeps that content on the machine.
- **The latency envelope's missing contract home blocks more than the numbers do.** Even if a 32.7 ms
  p50 were acceptable, accepting it is a human's declaration, not a round's conclusion — and until that
  declaration exists, "inline" is not a form this project is allowed to choose.

## Consequences

- `book/roadmap.md` gains one line: this is an **evaluation conclusion** (nothing adopted; the offline
  plan is the step), with the next step named. That line is the only book change (AGENTS 8: the book is
  user-facing; the numbers and type sketches stay in the engineering documents).
- **No code, no config, no schema, no CONF id, no trace field.** `design/` gains this ADR only; the
  register (DESIGN §13) is untouched because the candidates add no primitive, no mode and no leak — M6
  stays "not implemented; contract frozen" (ADR-016 item 5).
- **DP-1.4's finding stands:** this ADR does **not** give the latency envelope a contract home; it states
  "declared + measured" and requires that any inline use come with an owner-approved budget.
- The **40-fixture set is recommended as the seed of the frozen evaluation corpus** (D10's groundwork),
  with the manifest, digest, scope note and labelling rule of §7.1 — after which it is on the
  never-mutable list and only a human round may move it.
- **Two prerequisites for any future inline conversation** are now named in one place: the `auto`/L4
  human decision (candidate ①), and `router replay` (spec §9.3) plus P9 (ADR-016 item 5) for DP-1.5's
  reproducibility and M3/M4's rails.
- If a later round installs an advisor, the admission checklist it must satisfy is DP-1.1…DP-1.5 — and
  nothing in this ADR authorizes landing it.

## Honest boundaries

- **Not verified here:** Laya's model-card figures (third-party; a fetch from this run was blocked; the
  numbers above are quoted as the card's own claims, read by the operator), the vendor's price beyond the
  blog statement (the pricing page is a 404), and any claim about a model's behaviour on traffic that was
  not measured.
- **Not measured here:** an inline advisor's effect on the path (nobody has run one on the serving path);
  the representativeness of a 40-item hand-authored set against real client traffic (**zero** real traces
  exist in this tree); and any route-selector performance, because the evaluation set for that question
  does not exist.
- **Directional only:** one run, `n = 40`, one machine (Apple M4 Pro 48GB, macOS), one day. The interval
  arithmetic in §7.4 is the reason the wording throughout is "direction and mechanism".
- **Laya is not "bad at decisions".** It is a checkpoint fine-tuned for email-ticket triage evaluated
  zero-shot; its measured 75.0% / ECE 0.132 sits at or above its own reported zero-sample-family
  band (0.651 / 0.207). The sentence the evidence supports is: *as shipped, not suitable for this task; to be
  used on this task it must be evaluated or fine-tuned on our distribution* — which is the vendor's own
  requirement, and which §7 exists to make possible.
- **This ADR does not decide what happens next.** It names a candidate, an admissible form, a budget and
  a write-set; the alignment with the human happens in a round, per `program.md`'s per-round flow.

## Redaction note (2026-10-03, R62-1)

A personal absolute path (the owner home directory) inside the `#[path = …]` quote at the
bench-corpus bullet was replaced with the neutral placeholder `<home>`. The decision,
its reasoning and the measurement are unchanged; only the machine-local path prefix
was redacted for publication.
