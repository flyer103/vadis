# ADR-026 — the corpus splits in two: a human-signed layer that may carry `verified`, and an automated layer that structurally cannot

- Status: accepted
- Date: 2026-09-22
- Related: AGENTS hard constraints **4** (no unverified savings — the rule this ADR protects by keeping a
  mechanism figure out of the word), **5** (no fabricated prices or provenance — the licence/attribution rule
  the auto layer inherits), **6** (tests must not depend on current data — why the rubric asserts relations
  and frozen computations rather than snapshots), **9** / **ADR-012 item 2** (the measurement is not part of
  the search space: no gate, no fixed corpus, no conformance assertion, no L1 envelope moves here), 3 and 7
  (the observation boundary and English-only); **ADR-009 item 3** (the product persists no body — the reason a
  corpus is an out-of-band artifact at all); **ADR-005** (the trace is the only product→autowork channel);
  `autowork/program.md` (the D3 cost gate, the direction pool, the per-round flow — which gains step 0.5),
  `autowork/work-mode.md` (one builder per suite, the fan-out rule); the replay harness contract's **COR-2**
  (the corpus schema), **ARM-3.9** (the two pairing kinds), **VER-4.4 rev 3** (the five conditions `verified`
  needs), **PROV-8 §9.3.0** (the declaration ban), **HAND-10 §11.3/§11.4/§11.6** (the freeze act, what the
  loop may not do, the admitted string), and the round's own landing, **CORP-12** (`replay-contract.md` §14).

## Background

The user's ruling of 2026-09-22 (recorded in round 23's freeze card, `t_6fd3b39b`) settles what the loop may
automate, and it settles it in one direction: **corpus engineering is fully automated, and the source of the
word `verified` is narrowed.** The ruling's points, as the card states them:

1. automated corpus material **never** enters `verified`; it is for mechanism verification, transform
   development, and screening *"where should the real money be spent"*;
2. `verified` is reserved for **real wire paired captures**;
3. a real corpus's freeze is still **one human signature each** — `created_by` matching `^human(:.+)?$`,
   COR-2.4 unchanged, and the **charter's hard clauses do not move**;
4. the capture and the draft are the orchestrator's products (HAND-10 §11.1/§11.4, already frozen); the human
   performs exactly §11.3's four steps.

Three facts about the state of the loop make that ruling a design problem rather than a policy statement:

- **`verified` is the only currency the cost gate accepts.** `program.md`'s D3 ("a new rule's net gain > 0") is
  the one blocking gate never met, and its criterion is counted only from `verified` figures. At this round's
  base there are **zero** such figures in the repository's history — the two human rows (the corpus freeze, the
  live replay budget) are untouched, so the replay harness's `verified` branch has never executed.
- **The apparatus exists and is trustworthy; its input does not.** R12 built the capture format, the corpus
  schema, paired arms, the mock, the byte audits and the label arithmetic; R12's own independent
  re-verification found the label-authority hole (**R12-F1**: a three-line declaration minted `verified`) and
  fixed it by deriving provenance from the apparatus (**PROV-8** §9). The corpus it needs is the human's act
  (COR-2.4, ADR-012 item 2) — and it does not exist yet.
- **The loop must be able to work without that act.** A loop that can only measure on a human-frozen corpus
  cannot develop a transform, cannot screen candidates, and cannot tell the human *which* material is worth
  freezing. The alternative — a loop that signs its own corpus — is the one thing ADR-012 item 2 forbids: it
  would let the loop grow the fixed corpus toward whatever its transform measures well.

So the question this ADR settles is not "may the loop build corpora" (it must) but: **what may a number about a
loop-built set be called, and what keeps it from being called something it is not — mechanically, rather than
by policy?** The answer is a second artifact type, and a wall made of the absence of a field.

The tempting answer is the one R12-F1 already taught this repository to distrust. A declaration key
(`tier = "auto"`, `verified_ok = false`, `signed: true`) is a string written by the same actor whose work is
being labelled; PROV-8 §9.3.0 bans exactly this class of input from the label, and ACC-11.2 exists to falsify
it by mutating declarations and watching the label stay put. A one-off policy signature — a `signed_by` field,
or a detached signature over the manifest — is the same string one level up, and it is worse, because it
*converts* a type boundary into a verification boundary: a bug in it produces a false `verified`, where the
absence of a field can only produce a refusal.

## Decision

**1. Two tiers, two artifact types — never one artifact with a flag.** The **signed layer** is
`autowork/corpus/<corpus-id>/manifest.toml` (COR-2, unchanged: `schema_version = 1`, a human `created_by`, both
pairing kinds). The **auto layer** is `autowork/corpus-auto/<suite-id>/suite.toml`
(`schema = "router-auto-suite/1"`, `created_by = "auto:<builder>"`, `paired-arms` only, and a `source`
vocabulary of `synthetic` / `hermes-record` / `hf` / `github` — no `capture`). Same id regex, same digest
recipe, same `[[pair]]` reference rule; different file name, different schema key, different loader. A
disagreement between root and schema key — either direction — is a refusal at exit 2.

**2. The labels are different vocabularies, and the auto row has no ladder field at all.** A corpus row
carries `saving_label` / `saving_claim` / `verified_eligible` / `adopt_eligible` (VER-4.1, PROV-8 §9.3). A suite
row carries `screen_label` / `screen_claim` / `screen.*` and **does not define** any of the ladder's fields.
That is the wall: no value the loop writes can supply a field its schema does not have, and no path from the
suite loader constructs a corpus row. The question "may this row say `verified`?" is not answered wrongly
anywhere — it is not asked.

**3. Four belts, each independently falsifiable, none of them the wall.** (a) The auto `source` vocabulary
excludes `capture`; (b) `[[pair]].kind` excludes `paired-sessions` — the two shapes VER-4.4 rev 3's conditions
0–2 are built on, so a suite cannot even declare the shape a `verified` ladder is defined over; (c) the auto
layer has **no live path**: a declaration naming a suite with `run_kind = "live"` is refused at exit 2 before
anything starts, so the evidence condition 2 needs (usage read from an external counterparty) cannot exist for
a suite's bytes; (d) a mirror `created_by` rule: a `human:` value in a suite is refused, so the two smuggling
routes (a signed manifest under the auto root, a suite under the corpus root) are refusals rather than
relabellings.

**4. The auto layer is the loop's to write, and it is uncommitted where it is bulky.** The builder and its
adapters write it (`harness.corpus_auto build`); the loader and the scorer are read-only. Its metadata
(`suite.toml`, `suite-spec.toml`, `nomination.json`, `score-card.json`) is committed; its bodies
(`raw/`) are gitignored — the same split CAP-1.5 already freezes for a corpus, and the `.gitignore` line is one
card's, landing before any body is written.

**5. Promotion is a nomination, never a write.** The layer's only offer to the human is a list of items it
recommends — hashes, licences, proposed roles — and the human's §11.3 act is what turns one into a corpus. No
auto-layer tool may write, create, move or grow anything under `autowork/corpus/`; a path there is a refusal at
exit 2, which is §11.2.1's rule carried to a second tool. **The loop may find the item; only the human may
freeze it.**

**6. The scoring rubric is frozen as computations, not as descriptions.** Coverage (protocol, turn shape,
payload kind, payload origin, source, size, and the set-level count) and quality (loadable, determinism,
near-dup, canary, licence, discriminability) each name a definition, a frozen computation, a threshold and —
the part that matters — a **disposition**: whether a failure drops one item (the set is rebuilt without it) or
refuses the set. The card's determinism rule has its own allowlist. `discriminability` is a **byte-reach**
proxy, `est_tokens` and the transform ledger's saved-input figure are **inferred**, and no rubric number may be
restated as a saving.

**7. Coverage becomes an input to the direction decision, and nothing else changes.**
`program.md`'s per-round flow gains one step ("corpus score", before the direction decision, beside the
five-round lookback), and a coverage gap or a `needs` entry is what the direction's rationale must answer.
`needs` carries `closer: "loop" | "human"` — which is how `program.md` step 1.2's "no human prerequisite" rule
becomes visible instead of discovered.

**8. `verified` stays exactly where it was.** A gate number still requires the signed layer plus REAL-9's live
path under VER-4.4 rev 3's five conditions. This ADR freezes where the loop's *own* numbers may be used; it
moves no gate, no conformance assertion, no corpus, no envelope, and it does not touch COR-2's schema, ARM-3's
pairing table, VER-4's ladder, FAIL-6's codes or DET-5's allowlist. Its refusals are FAIL-6.2's existing `2`
and `6`.

## Alternatives considered

- **A declaration key (`tier`, `verified_ok`, a `signed:` flag on the run or the manifest).** Rejected: it is
  R12-F1's hole in a new coat, and the repository already owns the test that falsifies it (ACC-11.2 mutates the
  declarations and requires the label not to move). The cost we accept instead is a second schema, a second
  loader and a second row type — real duplication, bought for a wall that cannot be argued past.
- **A one-time policy signature: a `signed_by` field, or a detached signature over the manifest.** Rejected on
  three counts. (i) It does not exist in this harness's threat model: the same code path that writes a manifest
  would write its signature, so the loop can mint one — the string moves one level up, it does not leave. (ii)
  It needs key management, i.e. a new secret class in a repository whose only credential discipline is "the
  value never enters an artifact" (CAP-1.4, §4.7). (iii) It trades a *type* boundary for a *verification*
  boundary: a bug in a signature check produces a false `verified`, while a missing field can only produce a
  refusal. Kept as a registered possibility for a day when an out-of-band signer genuinely exists — at which
  point it would *widen* the signed layer's route, not narrow the auto layer's.
- **Make the mode carry the wall: the live path refuses to read auto-layer input** (the freeze card's
  candidate (a)). Rejected as the wall, adopted as a belt: a mode-conditional rule exists only in the modes it
  names, and it needs an input telling it which kind it is reading — which is the declaration problem again.
- **Make condition 0 unsatisfiable in the auto layer** (candidate (c)). Rejected as the wall, adopted as a
  belt, and the reason is the most useful piece of this ADR's reasoning: condition 0's witness is
  **byte-derived**, and the auto layer composes bytes. A composed item can carry a `prompt_cache_key` and a
  `session_source` string, so a byte witness cannot separate composed bytes from captured bytes. Candidate (c)
  would have been a declaration in bytes' clothing.
- **Put the auto layer entirely under the gitignored `autowork/results/`.** Rejected: a suite is a **fixed
  input** — a digest a screen verdict cites and a round re-derives — not a run artifact; unversioned, a screen
  verdict could not name what it screened. The corpus's own split (metadata committed, bodies not) is the right
  precedent, and it is the one taken.
- **Extend COR-2's `source` enum with the adapter names (`hf`, `github`, `hermes-record`).** Rejected: it would
  put automated provenance inside the signed schema and blur the one boundary this ADR exists to draw. A
  promoted item enters a corpus as `synthetic` (it is not a capture), with the nomination quoted in its `note`
  — COR-2.2.3's existing route for material that is not a capture, no schema change.
- **Give the auto layer a cheaper label (`unverified`, `provisional`, `screened`).** Rejected: sharing any word
  with the ladder invites a reader to compare them. `screen_label`'s values (`promising` / `neutral` /
  `regressive` / `uninformative`) share nothing, and `screen_claim` is a sentence about **bytes**, not tokens.
- **Make the auto layer's numbers gate-eligible "once they are good enough".** Rejected: a threshold is a
  declaration, and it would hand the loop the one lever ADR-012 item 2 forbids it — control of what counts as
  the measurement.
- **Let the scorer write the corpus item rows and have a human approve the diff.** Rejected: approval-by-review
  is a signature with a nicer interface; the act COR-2.4 requires is the human's own write, and §11.3 already
  makes it four commands.
- **Skip the rubric and let the loop pick sets by judgement.** Rejected: "which material is worth paying for"
  is exactly the question a frozen computation answers reproducibly, and an unfrozen answer is the one thing a
  later round cannot re-derive.

## Rationale

- **A wall made of an absent field cannot be argued with.** Every mechanism that *decides* whether a loop-built
  number may be called `verified` takes an input, and every input is something the loop can write. The only
  formulation with no input is the one where the field does not exist in that artifact's schema — which is also
  why the proof is a **negative** test (mutate every declaration; assert the key is still absent) rather than a
  positive one.
- **The two layers answer two different questions, so they deserve two vocabularies.** The signed layer
  answers *"did the transform make the real provider's bill smaller?"* — a question with a counterparty, a
  session cache and a bill. The auto layer answers *"does this rule's bytes reach this material, on both arms,
  reproducibly?"* — a question about the instrument. Naming the second with the first's word is what AGENTS
  constraint 4 exists to prevent, and its cheapest enforcement is that the word is not in the second's schema.
- **Automating collection is what makes the human's act a decision instead of a chore.** The human's freeze is
  kept (and must be kept) as the only route into `autowork/corpus/`; a loop that has already measured a
  candidate set makes that freeze a choice among named, hashed, licence-checked items rather than a curation
  project. The nomination is that difference made concrete: it carries no bodies, and it executes nothing.
- **A rubric earns its keep only if it can fail.** Each metric in the frozen rubric carries a threshold *and* a
  disposition, so "the set is thin here" (advice, feeds direction) and "the set is not a set" (refusal) are
  different sentences — the same distinction FAIL-6.2 draws between exit `4` and exit `2`. Coverage that can
  never reject anything is a report; coverage that can reject everything is a gate; this is neither.
- **Byte reach is honest where a token delta is not.** The auto layer is money-free, so the strongest evidence
  it can hold is a property of the bytes the router wrote and the arms it wrote them in. `discriminability`
  makes that the *named* proxy, and `screen_claim` is a sentence about bytes — which is exactly what "spend
  real money here" needs, and exactly what it must not overstate.
- **One builder per suite is a write-set rule, not housekeeping.** A suite's digest is cited by a screen
  verdict; two cards building one suite would produce two incompatible inputs to one claim. `work-mode.md`'s
  resource table gains that row for the same reason it already has one for a state dir.

## Consequences

- **Contract.** `autowork/harness/replay-contract.md` rev 4 — **CORP-12** (§14) is the landing: the clause map,
  the tier table and the loader's refusal matrix (§14.1), the candidates/the mechanism/the twelve-row negative
  battery with its exit codes (§14.2), the coverage rubric with the frozen `payload_kind` classifier (§14.3),
  the quality rubric with the frozen near-dup metric and the discriminability proxy (§14.4), the score card's
  frozen keys with its determinism allowlist and `needs[]` (§14.5), promote/reject and the nomination (§14.6),
  the four adapters with the local-record privacy boundary (§14.7), the loop's permitted use (§14.8), the
  money-free acceptance clauses **ACC-13.1…13.6** (§14.9) and the honest boundary (§14.10). §0.1, §0.2, §9.3,
  §11.3 and §11.4 each gain one pointing sentence or row; **no frozen sentence changes** and nothing is
  un-frozen.
- **Process.** `autowork/program.md`'s per-round flow gains step **0.5** ("corpus score", before the direction
  decision, beside the five-round lookback), numbered so every existing step keeps its meaning;
  `autowork/work-mode.md`'s scheduling step 0 reads the score card beside the lookback table, and its resource
  table gains one row (one builder per suite).
- **Implementation.** A new module, `autowork/harness/corpus_auto.py` (`build` / `verify` / `score` /
  `nominate`), is a **second entry point to its own rule set** — it does not import the corpus loader's
  validation, and the corpus loader does not import it. `run.toml`'s key sets gain `suite` /
  `expected_suite_digest`, and a declaration carrying both a corpus and a suite is refused. `harness.corpus
  draft|verify` and `harness.replay check|run|compare` keep their spellings, flags and exit codes.
  `.gitignore` gains exactly one line, `autowork/corpus-auto/*/raw/` — measured not-yet-ignored at this round's
  base, and one card's to land.
- **The instrument.** The score card is a *second* artifact with a *second* determinism allowlist; DET-5.1/5.2
  keep theirs. The card records the harness commit and the router binary it ran against, and `harness.commit`
  is deliberately **not** on its allowlist, so a card cannot be read as covering a different instrument.
- **Money and quota.** Nothing here spends. Every suite run is loopback `mock`; the auto layer has no live path
  at all. The live layer's budget discipline (REAL-9 §10, `work-mode.md`) is untouched.
- **The corpus.** No corpus is created, moved, grown or re-frozen by this ADR or by the round that lands it,
  and none may be created by any tool it introduces.

## Honest boundaries

- **The isolation is mechanical for the automatic layer, and only for it.** The signed layer's trust root is
  still a **string**: `created_by` matching `^human(:.+)?$` (COR-2.4, HAND-10 §11.6 — already admitted, and this
  ADR does not blur it). A machine-written `human:` string is still indistinguishable from a human's, and no
  part of this decision changes that. What it changes is a *different* hole: the loop can no longer mint the
  word `verified` without a human, because the word is not in the schema of anything it writes.
- **The label ladder's own residual is untouched.** VER-4.4 rev 3's condition 0 witness is byte-derived, so a
  crafted `synthetic` item inside a *signed* corpus could satisfy its letter. This ADR does not rest on that
  condition — §14.2's wall is a type boundary — and a human who freezes a corpus containing `synthetic` items
  makes their own call (COR-2.2.3 already blesses them).
- **The rubric measures a set, not the world.** `loadable` says the router accepted an item, never that the
  item is realistic; `near_dup` is a frozen heuristic with a stated false-negative direction; `canary` is a
  scanner, so its zero is evidence *for the pattern set it knows* and never a proof that no secret is present;
  `discriminability` is byte reach, and `est_tokens` is a local estimate labelled `inferred` wherever it
  appears.
- **No price, quota, token or dollar figure is decided, quoted or derived by this ADR**, and none may be
  derived from it: it settles what a loop-built set may be called, never what anything costs (constraints 4
  and 5).
- **The auto layer holds third-party bytes.** The licence discipline is: record what the source states, at the
  revision that was read, and never invent one. `unknown` is usable locally for mechanism work and is not
  promotable. Which material may be *committed* is decided at the human's freeze, not by the scorer — and the
  local-record adapter's privacy rules are conventions enforced by code review and by the canary refusal, not
  by anything cryptographic.
- **Nothing in this ADR is implemented at the time of writing.** No suite, no score card, no nomination and no
  `screen_*` row exists; §14.9's clauses are what the implementing card must assert, written as tests rather
  than as results. The mechanism's own proof is a negative battery, and its first honest reading will be a
  card's observed exit codes, not this document's prose.

## Reversibility

Reversible in both directions, and cheaply, because the auto layer touches nothing that already ran.

**Toward one layer:** deleting the auto layer is deleting one module, one root, one `.gitignore` line, one
`run.toml` key pair and one `program.md` step — no corpus, no gate, no conformance assertion and no serving
path is involved, and no `verified` figure in the repository's history was produced by it (there are none).
The screen vocabulary disappears with it; nothing else referenced it.

**Toward a stronger wall (the likely direction):** if an out-of-band signer ever exists, the *signed* layer can
adopt a real signature (the "one-time policy signature" alternative above) without re-opening anything here,
because this decision deliberately kept the two layers' provenance vocabularies disjoint. Nothing in CORP-12
has to be un-frozen for that, and no auto-layer artifact is affected.

**Within the decision:** the thresholds (the near-dup `0.80`/`0.30`, the discriminability `0.20`, the loadable
`0.90`/`0.50`, the size buckets) are screening thresholds in a frozen *computation*; a later round may move a
threshold by an explicit amendment to §14.3/§14.4 — the *shape* (definition, computation, threshold,
disposition) is what this ADR fixes, because that shape is what makes two rounds comparable. The tier wall
itself is not a threshold and is not tunable: it is a schema.

**The asymmetry to keep in mind:** the corpus side of this split is the expensive half to move. A signed corpus
that a human already froze stays exactly what it is; what this ADR changes is only what the loop may call the
sets it builds for itself.
