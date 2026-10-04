# ADR-034 — a promoted corpus must resolve on the declared live base (the promotion's route-namespace gate)

- Status: accepted
- Date: 2026-09-24
- Related: AGENTS constraints 3 (the observation boundary), 4 (**no unverified savings**),
  8 (docs before code) and 9 (**the measurement is not part of the search space**); **ADR-012** (the
  mutable scope — a corpus digest is part of a gate verdict); **ADR-015** (the byte boundary: mutation
  (b) replaces the top-level `model` *value*, which is why a routing scalar is not content);
  **ADR-022** (a candidate serves only on its own wire — the rule the two namespaces meet at);
  **ADR-026** (corpus tiers and automated scoring); **ADR-032** (the L2 measurement; **D5**, the composed
  bytes' honesty cost); the loop replay contract §11 (HAND-10, the capture→freeze handoff)
  and §14 (CORP-12, the tiers); the loop's freeze note §D2.0·A2 (the promotion path's ten
  steps with one owner each) and §A3 (reading (a): the authored session identity);
  the loop execution model §"Promoting an auto-layer nomination into the signed tier";
  the loop route check (the gate), the loop's receipts record (the
  re-freeze's receipts), the loop's route-probe record (the measurement).
- Numbering note: **ADR-033 is reserved** by R37's unmerged branch (`round/37-rrsi-adoption`, where the
  RRSI adoption contract lives). This number is taken deliberately so the two branches cannot collide at
  merge time; the gap in `design/decisions/` is a reservation, not an omission.

## Background

**A promoted corpus can name a route that the run it was frozen for cannot serve — and nothing in the
promotion path checked.** The recipe (the loop's freeze note §D2.0·A2) copies a nominated body **byte for
byte** (step 6) and then writes the signed manifest (step 7). The bytes come from the **auto** layer,
whose composer writes the model string that matches **that layer's own mock-only base**:

- `corpus_auto.AUTO_BASE_CONFIG` — "the auto layer's own mock-only base … with THREE providers — one per
  wire (the wire gate serves a candidate only on its own wire, ADR-022) … **Apparatus, not a price
  claim**": providers `deepseek-chat` / `deepseek-responses` / `deepseek-anthropic`, each with one URL;
- `sources/__init__.py`'s composing template — `model = f"deepseek-{protocol}/deepseek-v4-pro"`, with the
  comment that the model names the provider whose wire matches the protocol.

The **signed** tier's live run declares a different namespace by contract: the loop live-base config
(a dedicated declaration of what is being measured, not the deployed onboarding config — ARM-3.3.2 rev 3)
declares **provider `deepseek`** (one entry, three wires) and the alias **`coding-fast`**, and nothing
else. The vadis's route resolution (`crates/vadis-proxy/src/forward.rs`'s `resolve_route`, the single
owner on both media) consults the alias map, then the literal `provider/model` split — **there is no
wire-suffixed provider form**.

**Measured, not argued.** The first freeze of the L2 exercise corpus
(the recorded L2-composed pair, commit `a25b493`) promoted the nomination's bytes verbatim.
R38-1's money-free, loopback-only pre-flight probe (`harness/r38-1/receipts/route-probe.json`) measured:

| body | answer |
|---|---|
| the corpus's body **verbatim** (`"model": "deepseek-responses/deepseek-v4-pro"`) | **HTTP 404** `{"error":{"type":"unknown_provider","message":"unknown provider 'deepseek-responses'"}}` |
| the same body with **only** the `model` value replaced by `coding-fast` | **HTTP 200**, routed to the loopback mock |

The refusal happens **before any upstream call**, on **both arms**. One live invocation — the round's
authorized, budgeted act, the only one the whole round may spend — was therefore not spendable as
declared. And the repair could not be an edit: an item row's bytes are an input of the COR-2.3 digest, so
a corrected body is a **new freeze** (COR-2.3/COR-2.4: refuse, do not repair), which is a human act under
AGENTS 9 / ADR-012 and the row-11 policy.

**Why the existing tripwires were silent.** `The loop corpus tool verify` (§11.3 step 3) validates the manifest
against the bodies — schema, `created_by`, per-item sha256, `item_count`, the pair block — and the auto
layer's scorer had run the suite against its **own** mock base, where the namespace resolves and the suite
scored 9/9 discriminative. Both are green, and both are green about **different bases**. Nothing compared
the body's route against the base the run declares.

**The finding this ADR remedies.** The card that stopped (R38-1) registered it as
**R38-1-F1** in the loop's evidence record §6 — blocking for the round's third limb, owner the
human (freeze repair, delegated) or the operator (`live-base.yaml`) — and closed with a process note that
proposed, in one line, the step D2 now makes mandatory: *"every item's `model` resolves under the intended
live `base_config`"*. That note is the ancestry of this decision; D1–D5 are its executable form.

**The owner's ruling.** On 2026-09-24 the human owner adopted the orchestrator's two-part recommendation
(verbatim in the loop's session thread: 〈按照你的建议进行〉): **(a)** re-freeze the corpus with its route
namespace reconciled to the declared live base, and **(b)** give the promotion path a step that refuses an
unreconciled body. (a) was executed as a **delegated freeze** — the owner authorized, the orchestrator ran
HAND-10 §11.3's steps, the disclosure is in the corpus's own FREEZE DISCLOSURE block, and the precedent is
R23-F6; receipts in the loop's evidence for that decision). This ADR is (b)'s WHY.

## Decision

### D1. The gate, its command, and the rules it mirrors

the loop route check refuses a corpus whose item bodies name a route the declared base config
cannot serve. It mirrors the vadis's own two rules **and no others** — `aliases[model]` (whose target's
provider and model must themselves be declared, the load-time rule), then the literal `provider/model`
form — and it reports the refusal in the vadis's own words (`unknown provider '…'`, `unknown model '…'
on provider '…'`), so a reader can compare the gate's verdict with the answer the vadis would give.

- **It refuses; it never repairs.** A failing corpus is not frozen as it is. No body is rewritten by the
  tool, and nothing is written under the corpus (COR-2.4).
- **It binds both artifacts by sha256** — the base config and the corpus manifest — so a green card names
  the exact pair it read.
- **Exit codes follow FAIL-6**: `0` every body resolves; `2` at least one refusal.
- **Money-free and offline**: it reads two files and parses JSON — no socket, no credential, no provider.
  This is what lets the gate be a required step rather than an aspiration.

### D2. One new step in the promotion path, and its owner

the loop execution model §"Promoting an auto-layer nomination into the signed tier" adds the step between
the body copy (recipe step 6) and the manifest write (step 7):

the loop's route check, run over the nomination's corpus against the declared base config.

**Owner: the card that owns the promotion** — the same card that owns the auto-corpus tree and prints
the freeze commands (§D2.0·A2 step 1–5's owner). It runs the gate **before** it hands the bodies to the
human's freeze, so a human is never asked to freeze a corpus that cannot run. The freeze itself stays the
human's act; this step moves no authority.

### D3. What the reconciliation may be — and it is the freeze's act, disclosed

When the gate refuses, exactly two acts are legitimate, and both are visible:

1. **Rewrite the body's routing scalar** to a value the declared live base resolves. The rewrite is
   **byte-level and in place** (no re-serialisation: no whitespace, quoting or key-order change anywhere
   else), it is **disclosed in the manifest's FREEZE DISCLOSURE block as an authored field**, it moves the
   item's sha256 and therefore the corpus digest, and it is therefore **a NEW freeze** — authorized by the
   human, executed under the delegated precedent, with the old corpus left frozen and untouched beside it.
   This is the route R38 took: `l2-composed-pair-2026-09-24` → `l2-composed-pair-2026-09-24-live`.
2. **Declare the namespace on the base side** — point `live-base.yaml` at the provider names the bodies
   carry. This is the **operator's** config act, and it is legitimate only when that really is what is being
   measured; it is not the default, for the reason in Alternatives (b).

**A routing scalar is not content.** Mutation (b) of the byte boundary (ADR-015) replaces the top-level
`model` value with the resolved route's native id before the request leaves the vadis, and the L2 rule's
target lives under `input`. So a byte-level rewrite of that one scalar does not move what the corpus
measures — and the freeze must **assert** that mechanically rather than assume it: the rewrite receipt
records, per item, that the prefix and suffix are byte-identical and that the two bodies' JSON is identical
except for `model` (the loop's nomination diff).

### D4. What the gate is not

It is not a content check, not a provenance check, not a wire check (`wire_api == proto_in` is the product's
own gate, ADR-022), not a statement about keys, prices or reachability, and **not a substitute for any of
VER-4.4's five conditions**. A green card means exactly one thing: *the declared base can serve these
bodies' routes*. Nothing else may be read off it.

### D5. The tripwire stands in both directions, as a test

The first freeze is a **standing RED control**: `l2-composed-pair-2026-09-24` must keep being refused with
the provider named, and `harness/tests/test_corpus_route_check.py` asserts it — together with the green
side on the re-freeze and on `codex-pair-2026-09-22`, the corpus that **has** run live. If the gate ever
refuses a corpus that demonstrably ran, the gate is wrong about the world and the test says so.

## Alternatives considered

**(a) Change the composer (or `AUTO_BASE_CONFIG`) so the auto layer composes a live-resolvable namespace.**
*Not taken here.* It is the more thorough repair — it removes the mismatch instead of gating it — but it
moves **every** auto suite's bodies and every suite digest, and the mock-only base is what the auto scorer
scores against; that is a change to the auto layer's own artifact set, and it belongs in the round that
owns the auto-corpus tree and can re-measure the suites. Registered as an open candidate (owner
`harness-conv`) for the round close-out to carry; this ADR deliberately does not take it.

**(b) Make `live-base.yaml` carry the auto layer's provider names by default.** *Refused as the default.* It
makes the rig's namespace the authority, hides the mismatch from the next promotion, and contradicts what
that file is for: a dedicated declaration of **what is being measured**, pruned by the operator
(ARM-3.3.2 rev 3). It stays legitimate only as D3's second arm, when it is what is genuinely measured.

**(c) Absorb the value with a `/`-shaped alias key** (`"deepseek-responses/deepseek-v4-pro"` →
`deepseek/deepseek-v4-pro`). *Refused.* This is the hazard class R14-F2 registered: alias keys have no
lexical rule, so a `/`-shaped key silently shadows the explicit-route form. It would also keep the corpus's
namespace dependent on the rig.

**(d) Leave it to the pre-flight probe each running card writes.** *Refused as the mechanism.* R38-1's probe
worked — it caught the defect and saved the round's live call — but nothing **required** it, it exists in one
card's rig directory, and the next promotion would have had to reinvent it. A gate that only some cards
happen to run is not a gate.

## Consequences

- **A promotion gains a mandatory, cheap, offline refusal** between the body copy and the freeze: a corpus
  that cannot be served is refused in a second, before a human is asked to sign it and before any live
  invocation is declared.
- **The measurement's honesty cost of D3.1 is a disclosure, not a silence**: a freeze that authors a routing
  scalar must say so in its FREEZE DISCLOSURE block, in the same breath as ADR-032 D5's composed-bytes
  caveat and §D2.0·A3's authored-session caveat.
- **The gate is loop-side only.** It changes no product byte, no gate definition, no threshold, no
  conformance assertion and no contract in `docs/` or `design/DESIGN.md`; it adds one harness tool, one test
  module and one workflow step.
- **What it cannot do, stated so it is not over-read**: it cannot know whether a provider is reachable
  (keys, network, cooldown), it cannot tell a composed corpus from a captured one, and it cannot see the
  wire. It closes exactly one class: *a promoted body whose route the declared base does not declare* — the
  class that cost this repository one authorized live invocation it could not spend, and one freeze it had
  to re-cut.

## Publication note (2026-10-03, R62-2)

The `R<n>` labels and finding ids in this document name iterations of the project's own
private analysis loop — a loop that is not part of this repository, so no label here is
resolvable by a reader of it; they are kept as the provenance of the decision. This
publication pass removed only the dead-pointer class: every reference into that loop's
working tree (its file paths and round-record names, its state record, charter, execution
model and replay contract, its scripts and module names, and the kanban card ids), each
replaced by the neutral phrase the sentence needs. Nothing else moved — no figure,
threshold, `§`/`ADR`/`CONF` id, code sample or contract sentence.
