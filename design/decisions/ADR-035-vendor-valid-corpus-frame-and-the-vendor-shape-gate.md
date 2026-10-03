# ADR-035 — a signed corpus's frame must come from bytes the vendor accepted (the skeleton-graft rule and the vendor-shape gate)

- Status: accepted
- Date: 2026-09-24
- Related: AGENTS constraints 1 (the byte boundary), 3 (the observation boundary), 4 (**no unverified
  savings**), 5 (**no fabricated prices**), 8 (docs before code) and 9 (**the measurement is not part
  of the search space**); **ADR-012** (the mutable scope — a gate definition is outside it);
  **ADR-015** (the byte boundary's two mutations); **ADR-026** (corpus tiers and automated scoring);
  **ADR-028** (the wire recorder); **ADR-032** (the L2 measurement; **D5**, the composed bytes'
  honesty cost); **ADR-034** (the promotion's route-namespace gate — the layer above this one, whose
  **D4** honesty boundary this ADR deliberately mirrors); the loop replay contract §11
  (HAND-10), §14 (CORP-12), §9 (PROV-8) and ARM-3.8/3.9; the loop's freeze note §D1.4 and
  §D2.0; the loop's freeze note (this round's clause list and its receipts);
  the loop's evidence record (this ADR's measurements); the loop route check
  (the gate this one is modelled on); `rules/tool_output.toml` (`[filters.bash-log-noise]`).
- Numbering note: **ADR-034** landed with R38; **ADR-033** landed with R37. This is the next free
  number in `design/decisions/`.

## Background

**Two failures, one layer apart, and the second was invisible to the first.** R38 (merged at `d8a0657`)
spent its one live invocation on a corpus that had already been repaired once:

1. **The route layer (`R38-1-F1`, CLOSED).** The first freeze `l2-composed-pair-2026-09-24` carried the
   auto layer's **mock-only** namespace (`deepseek-responses/deepseek-v4-pro`), which the declared live
   base does not declare: `404 unknown_provider`, both arms, before any upstream call. The remedy —
   landed in R38 — was a re-freeze with one top-level `model` scalar rewritten in place
   (`l2-composed-pair-2026-09-24-live`), plus **ADR-034**'s gate `harness/corpus_route_check.py` and a
   mandatory promotion step.
2. **The vendor layer (`R38-3-F1`, OPEN until a new corpus exists).** On the repaired freeze, both arms
   routed **external** and both bodies were refused **HTTP 400 `invalid_request_error`**: *"The
   `reasoning_text` in the thinking mode must be passed back to the API"* — the **verbatim control arm
   included**.

**The re-freeze is the proof that these are two layers, not one gate twice** (measured by this card,
read-only, both commands on bytes already on disk):

| corpus | ADR-034's route gate | this ADR's vendor-shape prototype |
|---|---|---|
| `codex-pair-2026-09-22` (captured) | GREEN, 0 of 14, rc=0 | GREEN, 0 of 14 |
| `l2-composed-pair-2026-09-24` (first freeze) | RED, 3 of 3, rc=2 | RED, 3 of 3 |
| `l2-composed-pair-2026-09-24-live` (re-freeze) | **GREEN, 0 of 3, rc=0** | **RED, 3 of 3** |

The route gate **passed** the corpus the vendor then refused. A gate that answers a *different*
question is the correct remedy; a stronger route gate would have been the wrong one.

**What the accepted bytes prove, and what the composed bytes lack** (14 accepted bodies vs 6 refused,
re-derived by this card; the loop's evidence record §1):

| | accepted (`codex-pair-2026-09-22`) | refused (both composed freezes) |
|---|---|---|
| top-level keys | **12** — `client_metadata, include, input, instructions, model, parallel_tool_calls, prompt_cache_key, reasoning, store, stream, tool_choice, tools` | **3** — `input, model, stream` |
| every `function_call` preceded by a `reasoning` item carrying `content[].reasoning_text` | **12 of 12** bodies that carry calls (2 carry none) | **0 of 6** |
| bodies / tool-output size | 14 bodies, 35690–42519 B; largest tool output 1988 B | 3 bodies, 1596–2425 B; largest 1842 B |
| live outcome | **200 + real usage** (50 rows over 5 run roots; one further run root of 2 rows at **502**) | **HTTP 400** |

**The owner's ruling (2026-09-24, verbatim `〈确认〉`)** adopted: **(i)** build the corpus from a
**vendor-accepted skeleton** — the conversation shape taken from captured traffic, **only the payload
authored** — rather than re-composing a conversation and hoping; **(ii)** add the **vendor-shape gate**
one layer below ADR-034's; **(iii)** **refuse** the alternative of byte-rewriting the composed bodies to
insert a `reasoning` item.

**The corpus half is not independent of this.** The corpus that can exercise the lever must also be a
corpus the *tier's own rubric* does not read as thin: the captured donor reads `size` **fail** — value
1, threshold 3, the single bucket `['8–64 KiB']`, bodies 35690–42519 B — and the rule the round exists
to exercise (`[filters.bash-log-noise]`) **selects all 36 of its tool-output nodes and has almost
nothing to drop** (36 B of 13772 B; longest line 62 characters; longest payload 46 lines; no line over
240 characters). So the round needs a rule for *which bytes may be authored*, and a target for *how
much*.

## Decision

### D1. The corpus-shape rule (`CS-1`): the frame is accepted bytes, the payload may be authored

> A **signed-tier** corpus body's conversation **frame** must come from bytes a vendor **accepted**;
> only the **payload** — the tool-result text the lever compresses — may be authored.

**Frame** = the top-level fields and their values, every `input` member **except** the
`function_call_output` members' `output` values, the `call_id`/`id` pairing, and the reasoning echo.
**Payload** = the `function_call_output.output` values. Concretely, and byte class by byte class:

| class | authored? |
|---|---|
| the top-level key **set** | no — must be a **superset** of the accepted skeleton (`VG-2`) |
| top-level **values** | no — the donor's, verbatim |
| the top-level `model` value | no — **and no rewrite**, because the donor's `"coding-fast"` is the alias the declared base routes (the route gate reads the donor GREEN, 14 of 14) |
| `input` members that are not `function_call_output` | no — byte-identical to the donor's member at the same index |
| `function_call_output.output` | **yes, and only here** |
| the `input` array's growth order, and the member count | no — a graft appends and removes nothing |

**The growth invariant.** Measured: within a session, item N's `input` array is a **leading-element
prefix** of item N+1's (12 of 12 consecutive pairs, both sessions), while the item body is **not** a
byte prefix of the next. So the invariant a graft must preserve is element-wise over `input` members,
and it carries a consequence: a payload authored for member index `k` is the **same byte string** in
every item of that session that carries `k`. This is what makes "the frame came from the capture"
**checkable** rather than asserted — it is false of any body composed from scratch and true of the
donor — and the graft's receipt asserts it, with a test re-deriving it from the donor.

**The disclosure obligation.** The `FREEZE DISCLOSURE` comment block is *extended* (comments are not
manifest keys, so COR-2.2.1 is untouched) to name: `source = "synthetic"` for every item and why (D6);
the donor capture id + each donor item's sha256; the payload's generator id + version + seed and its
span; **the pair's own difference at its exact address**; and the delegated signature (`R23-F6`).
The measured sentence for this corpus: *the two measured items' tool-output nodes are size-identical
(5 nodes, 3188 B each, not byte-identical); the pair's own 524 B of size difference lies entirely in
the frame — the per-session `call_id`/`id` values and the two clients' own text — 0 B at the payload
nodes.* That sentence is `R38-2-F1`'s field, answered by measurement instead of by prose.

**And the record's wording.** The harness's `diff_summary` sentence — *"two captures of the same
content in two distinct client sessions"* — is **half true** of such a corpus: the frames are two real
client sessions' captured bytes, the payloads are authored. Round files and cards say exactly that.

### D2. The gate: derived from accepted bytes, two rules, one committed artifact

**Home: the loop vendor check** — a new top-level module, invoked as `python -m`,
mirroring ADR-034's gate in shape: the same "where it sits in the promotion" docstring, the same FAIL-6
exit codes (0 / 2), the same mutually exclusive `--corpus <dir-or-id>` / `--body`, the same money-free,
credential-free, read-only declaration, the same one-line-per-body + verdict print. **Not** a second
verb in `corpus.py`: §11.2.1 freezes the corpus CLI's entry points, and a new module is ADR-034's own
precedent. It never writes under the corpus (COR-2.4).

**`VG-1` — the reasoning echo.** If a body carries ≥1 `input` member of `type == "function_call"`, then
≥1 `input` member of `type == "reasoning"` whose `content` contains a `reasoning_text` child must
appear **before the first** `function_call`. Derived from bytes, not prose: 14 of 14 accepted bodies
GREEN (2 vacuously), 6 of 6 refused bodies RED.

**The stronger form is refuted by the accepted bytes and must not be implemented.** "A `reasoning` item
immediately precedes each run of calls" is **false**: in the accepted body `it-14`, member `[15]` is a
`function_call` whose immediate predecessor `[14]` is a `message`. A gate asserting the stronger form
would refuse bytes the vendor accepted — a worse defect than the one it repairs. The counterexample sits
beside the rule in the gate's own tests.

**`VG-2` — the top-level skeleton.** The body's top-level key set must be a **superset** of the accepted
skeleton = the **intersection** of the accepted bodies' top-level key sets (12 keys). A superset is
admitted deliberately: the rule is what the bytes prove, and a body carrying extra fields is not
thereby refused.

**The skeleton is a committed, hash-bound artifact** (the loop vendor skeleton): the
accepted source's identity (`corpus_id`, `corpus_digest`, `manifest_sha256`, per-item sha256), the
skeleton and each accepted body's key set (so the intersection is re-derivable without the bodies), the
two rules' parameters, and the **acceptance witness** — the run roots and statuses, **including the one
run root of 2 rows at 502**. The bodies are gitignored (`R34-3-O1`), so the artifact is what makes the
gate work where they are not; `--derive-accepted` re-derives from the bodies where they exist and
**refuses** if the derivation differs. **The artifact is a gate definition**: AGENTS 9 / ADR-012 put it
outside the loop's mutable scope, so a change to it is a **human's** decision, never a loop outcome.

**Refuse, never repair.** The gate prints what it read, names the rule, and prints the offending
member's index. It edits nothing.

### D3. One step in the promotion path, adjacent to ADR-034's

The step lives in the loop execution model, beside ADR-034's route gate. It runs on the **same bodies
the freeze is about to copy**. Order: **route gate first** (one scalar per body; its refusal is
upstream), **vendor gate second**; both GREEN before the freeze.

The **capture** route's steps (this round's route) are catalogued in the loop's freeze note §C3.1 with one
owner each: draft from the capture → **graft** (frame from the draft, payload authored) → the round's
offline suite scored `$0` → route gate → vendor gate → the human's freeze → `corpus verify` (the
tripwire in both directions) → the authoritative `size` reading → pin the digest → run. The
**nomination** route's ten steps (the loop's freeze note §D2.0·A2) are a different route and are **not**
this ADR's to move; that its table still lacks both gates is registered (`R39-1-F4`, carrying
`R38-2-F3`).

### D4. What this gate is **not** — the honesty boundary, one layer down

ADR-034's **D4** says its gate never claimed vendor validity. The symmetry is exact and binding:

- A GREEN card means **exactly**: *these bodies carry every top-level field the accepted bodies carry,
  and every echoed assistant item carries the reasoning echo the accepted bodies carry* — as witnessed
  by the accepted bodies listed in the artifact.
- It is **not** a content check, **not** a provenance check, **not** a route check (that is the other
  gate), **not** a wire check, and **not** a substitute for the ladder's five conditions or for any
  figure's label.
- It does **not** promise the vendor will accept a body that passes. The vendor's complete validation
  rule set is not enumerable from eight accepted bodies' worth of evidence; a body satisfying both
  rules may still be refused for a reason no accepted body exercises. **The gate refuses two measured
  classes and claims nothing about the rest.**
- It measured the two rules' **discriminating power** on byte evidence it can reproduce (14 GREEN / 6
  RED, plus the fixtures in `C2.6`), and a control set that includes a **GREEN** on a body with **zero**
  tool calls, so the gate cannot drift into a "must carry calls" rule the accepted bytes do not support.

### D5. The size target and the payload floor

**The target is the tier's own, not this ADR's** (`SZ-1`): the promoted corpus must read `size`
**pass** — **≥ 3** distinct buckets over `items[].bytes` among the five frozen edges (1 / 8 / 64 / 256
KiB), **and** `max(bytes) ≥ 8 KiB` (the loop replay contract; `corpus_auto.py:182-196`, the same file;
read on the signed card by `corpus.py:158-162`). Re-derived by this card: the donor reads `size` fail,
value 1, bucket `['8–64 KiB']`. §14.3's disposition for a miss is a `coverage_gap` plus a `needs[]` row
whose `closer` is `human` — **not a refusal** — and this round's freeze makes the target a
**precondition of the round**, stated as the freeze's decision rather than as a threshold someone
calibrated. Given the donor's frame (35690–42519 B), the reachable set is `8–64 KiB` / `64–256 KiB` /
`>256 KiB`, i.e. one grafted payload ≥ ~22 KiB and one ≥ ~220 KiB.

**The floor is derived from the pair** (`SZ-2`): the donor pair's own difference is **524 B by size, all
in the frame** (0 B at the payload nodes), and the ladder's arithmetic carries it
(`delta.input_total = Σ(a−b)`). As a fraction of the measured payload that is 26.4% at today's 1988-byte
node, 6.4% at 8 KiB, 1.6% at 32 KiB and **0.8% at 64 KiB** — so **≥ 64 KiB** puts the pair's own
difference at ≤ 1% of the payload beside it, and 64 KiB is the tier's **own** second bucket edge rather
than a number this ADR chose.

**And material, not merely size** (`SZ-2(a)`): the deciding rule's size-reducing stages are
`strip_ansi`, the `… in 12.3s` suffix `replace`, `strip_lines_matching`, `truncate_lines_at = 240` and
`max_lines = 200` (`rules/tool_output.toml:163-197`). Measured on the donor: the rule selects **36 of
36** nodes and the only content it can drop is a trailing blank line — **36 B of 13772 B**, longest line
62 characters, longest payload 46 lines, no line over 240 characters. So a bigger payload that is still
short lines and few lines would change nothing; the payload must carry **>200 lines**, or **a line
longer than 240 characters**, or lines matching the strip patterns, and the receipt must name which.

**The arithmetic, with the config's own row and no claim** (`est_tokens = ceil(bytes/4)`, inferred;
`config.example.yaml:147-160`'s `deepseek-flash` off-peak `input_miss 0.00015` / `input_hit 0.000003`
USD per 1K tokens, source `https://api-docs.deepseek.com/quick_start/pricing @2026-09-21`): **8 KiB
dropped = 2048 inferred tokens = 0.000307 USD miss / 0.0000061 hit**; 64 KiB = 16384 tokens =
0.00246 / 0.000049; 256 KiB = 65536 tokens = 0.00983 / 0.000197, per request. A 1.6–2.4 KB payload —
the refused corpora — is ~3–6×10⁻⁵ USD of dropped input, under every plausible measurement floor:
which is why "the payload is too small" is a measurement defect and not a preference. **No figure is
minted here; the numbers are arithmetic over a source-named price row, labelled inferred.**

### D6. Which payload source is admissible

**The synthetic adapter's generator family** (`the loop's sources package::_synth_span_bytes`,
`r23-payload-walker@1`), **at a new version or a new generator id** whose size classes reach the buckets
`SZ-1` names: deterministic by construction (the spec's seed and the item index, never the clock),
`licence = "self-authored"`, a revision string `<generator>@<version>#<seed>`, no network and no
credential — and it **already emits** the shrink classes the deciding rule drops (`Compiling crate-`
prefixes, `[ 42%]` progress lines, blank lines, `x`×300 over-long tails), which is `SZ-2(a)`'s premise
realised by the same artifact.

The other three adapters are **refused for this promotion**, each with its reason:
**`hermes_record`** needs a recorded run (producing a capture is the orchestrator's act, HAND-10 §11.4)
and its payload sizes cannot be steered to a bucket, and it carries §14.7.1's privacy boundary;
**`hf`** and **`github`** need the network **and** an external licence probe at build time, against a
$0/offline round, and they graft a third party's bytes into a body whose frame is this repository's own
captured traffic — one checkable provenance chain becomes two (plus a credential, for `github`).

**`@1`'s observable output must not change**: existing suites are pinned by generator version, so the
new classes arrive as a **new version**, never as an edit to `@1`.

**Where the licence evidence lives.** The signed item rows are a closed key set with **no** `licence`
field and no payload-span field (`replay.py:156-159`), so the payload's licence evidence lives in (i)
the auto suite's own row, which does carry `licence` (§14.7), and (ii) the promoted item's `note` plus
the freeze disclosure, quoting the same revision string. Registered as `R39-1-F2` rather than worked
around.

### D7. The composer is demoted, and the loop's sources package is granted to the round's build card

`_compose_request` (`sources/__init__.py` ≈the same file, its model write at the same file) is what synthesised the
refused conversation. Its role under this ADR: **the auto tier only**, where its mock-only base is
correct (ADR-034's finding). On the signed tier **no path that writes a body may call it**, and
`:273`'s `model = f"deepseek-{protocol}/…"` must not be reachable from the graft. The graft is a **new
module** (`harness/sources/graft.py`) exposing a **pure function** of (donor body bytes, authored
payloads, stable spec) that inserts no member, removes no member, and touches no byte outside the
payload values it is given.

`R35-1-F7` registered that the loop's sources package lies in no card's write set. **This ADR
resolves it by grant, not by improvisation**: the round's build card (R39-2) owns
the loop's sources package for this round, scoped to the new graft module (+ tests) and a **new
generator version** — never a change to `@1`'s deterministic output, never a call into the composer
from the graft path. **The grant expires with the round.**

### D8. The live attempt's shape and its disclosures

R39-3's one invocation is R38-3's, unchanged: `run_kind = "live"`, `order = "cold"`, the ADR-028 wire
recorder, the rig committed **before** the run, the transcript outside the repo, the five conditions
adjudicated from the row, `edited_paths` counted by hand, the pair's own difference attributed
separately, and **one** invocation spent once. Cap **$1.00** under the standing authorisation (STATE
row 12, the human's 2026-09-23 08:25 ruling), with the round's fresh go-ahead being the owner's
2026-09-24 `〈确认〉`.

Four disclosures travel **in the same breath** as any figure from this corpus, and this is the one place
this ADR **narrows** ADR-032 D5: (i) D5's composed-bytes caveat applies to the **payload half** — the
frame is captured; (ii) `R23-F6` — the freeze is **delegated** (`human:flyer103` = the human decided,
not that a human ran §11.3 at a terminal); (iii) the pair's own difference at its own address (0 B at
the payload nodes, 524 B in the frame); (iv) the payload is **synthetic**, so what is measured is the
transform on *authored* tool output — which is what the lever claims to act on and is **not** a claim
about a client's real tool output.

## Alternatives considered

**Byte-rewriting the composed bodies to insert a `reasoning` item (the owner refused it; recorded here
with its measured reasons).** It authors **model thinking** — the one byte class a corpus has the least
right to invent — while fixing only the *field the vendor named*, leaving the vendor's other
requirements untested (`tools`, `instructions`, `prompt_cache_key`, `include`, `store` are all absent
from the composed bodies), and it leaves the payload at 1.6–2.4 KB, i.e. too small for the lever it is
supposed to exercise. Rejected on three independent grounds; any one would be sufficient.

**Deriving the gate's rule from the vendor's prose instead of from accepted bytes.** Rejected: the
diagnosis this round starts from is a claim about **bytes**, and the accepted bytes are the only
evidence in the repository that the vendor accepted *these* shapes. A prose rule would also be
unfalsifiable — nothing would tell a future reader which sentences were load-bearing. The artifact
therefore records the source's digests, and `--derive-accepted` re-derives from the bytes.

**A stronger echo rule** ("a `reasoning` item immediately precedes each run of calls"). **Refuted by
the accepted bytes**: `it-14`'s member `[15]` is a `function_call` after a `message`. Rejected because a
gate that refuses bytes the vendor accepted is worse than no gate.

**Refusing a body whose top-level key set differs from the skeleton in *either* direction.** Rejected:
the accepted bodies prove a superset requirement, not equality, and a stricter rule would refuse
plausible future traffic on no evidence. (The one accepted-body key asymmetry, if it ever appears, is
handled by the intersection — the weakest claim the evidence supports.)

**Making the vendor gate a second verb inside `corpus.py`, or a mode of the route gate.** Rejected:
`corpus.py`'s draft-side CLI is frozen by §11.2.1, and folding two different questions into one command
would make a single exit code mean two things — precisely the ambiguity that let R38's corpus reach the
wire.

**Re-freezing the composed corpus a second time instead of changing the corpus's shape.** Rejected:
the defect is not a scalar; it is the whole conversation shape, and the fixed field was one of nine
missing ones. The shape rule (D1) removes the class of defect rather than the instance.

**Taking `hermes_record`, `hf` or `github` payloads (D6).** Rejected individually with the reasons in
D6; the common thread is a second, uncheckable provenance chain, or a size class that cannot be steered
to the tier's own buckets.

**Reading HAND-10 §11.4's enumeration as exhaustive, so that a graft is a contract change.** Considered
seriously and **not taken** (registered as `R39-1-F1`, blocking only under this reading). §11.4's
prohibitions — create, move, grow, freeze or re-freeze a corpus; write a `created_by`; choose which
items a corpus covers; produce the capture — are **all respected** by a graft that authors only
disclosed payload bytes and hands the result to a human to freeze or refuse. Its "entire mechanical
surface" sentence is read as the **judgement-free half** described by class, on the authority of the
owner's own ruling (i), which places the payload inside the loop's authorship. The text gap is
registered for the next contract revision rather than amended here.

## Consequences

**What becomes possible.** A corpus can be **capture-shaped and lever-sized at once**: the frame is
bytes a vendor demonstrably accepted (with the acceptance witness committed), the payload is authored,
disclosed, and large enough and shaped enough that the deciding rule has material to drop. The gate
makes "the vendor will accept this shape" a **pre-freeze, offline, $0** question instead of a live
one — the R38 mistake (discovering a composition defect at the vendor's validator, after the round's
one invocation was spent) cannot recur in that form.

**What it costs.** A graft is a **new mechanical act on the promotion route** (`R39-1-F1`): it authors
bytes inside a body a human will freeze, and it moves the bytes a `draft_digest` was computed over. Its
honesty cost is paid in disclosure (D1) and in a checkable invariant (the growth prefix), not in a
signature — which is why the disclosure obligation is part of the clause and not a nicety.

**What it constrains for everyone downstream.** Any corpus built this way is `source = "synthetic"`
(`R39-1-F2`) with a captured frame, so a reader filtering by `source` will read it as fully synthetic;
the disclosure is the only place the difference lives. And every figure from such a corpus carries the
four disclosures of D8 — the narrowed D5 caveat, the delegated signature, the pair's own difference at
its address, and the authored payload.

**What does not move.** The byte boundary's two mutations (this route needs neither); the label ladder
and its five conditions; the tier boundary that keeps the auto layer from reaching `verified`; the
frozen corpus bytes of R23 and of R38's two freezes, which stay byte-untouched as standing controls.

**The one thing this ADR cannot promise.** That the vendor will accept a corpus built to this rule.
`R38-3-F1` closes when a live row carries `usage` in both arms, not when a gate reads GREEN — and the
gate says so in its own vocabulary.

## What this ADR does not decide

- **The contract text's revision** (`R39-1-F1`): whether §11.4's enumeration and §11.2.1's CLI gain the
  graft. The human's, with the file:line registered.
- **The signed vocabulary's gap** (`R39-1-F2`): whether a capture-framed, payload-authored item needs
  its own `source` word or a disclosure key. The human's / `harness-conv`'s.
- **Anything in the loop charter**: no direction, gate or threshold is calibrated here, and the direction
  pool's D3 entry is untouched.
- **The nomination route's step table** (`R39-1-F4`, carrying `R38-2-F3`): the loop's freeze note is not this
  round's write set.
- **`R35-1-F5`'s reading** (whether a *composed* corpus may carry `verified`): not reached on this route,
  because every item's `session` identity here is a **capture fact**, and not re-adjudicated.

## References

- the loop's freeze note — this round's clauses (`CS-1`, `VG-1`, `VG-2`, `SZ-1`, `SZ-2`,
  `PS-1`, `CG-1`, `LA-1`), the gate's exact command and control set, the capture route's steps.
- the loop's evidence record + the loop's evidence for that decision — every measurement this ADR
  cites, with the artifact each number came from.
- `design/decisions/ADR-034-promoted-corpus-must-resolve-on-the-declared-live-base.md` — the layer above,
  and the source of the honesty boundary D4 mirrors.
- the loop replay contract §11 (HAND-10: the capture → freeze handoff, §11.3's four
  commands, §11.4's prohibitions), §14 (CORP-12: tiers, the coverage rubric's `size` row, the four
  adapters), §9/ARM-3.8–3.9 (`external`, and why `paired-sessions` is the ladder's shape).
- the loop replay driver (the item key set), the same file (`created_by`), the same file (the
  `source` vocabulary), the same file (the COR-2.3 digest recipe).
- the loop corpus tool (`draft`/`verify`/`score`), the loop auto-corpus tool,
  the same file (`size_bucket`, `_metric`), the loop route check (the gate's shape).
- `rules/tool_output.toml:163-197` (`[filters.bash-log-noise]`, its stages and parameters);
  `crates/router-core/src/transform.rs:154-183` (the selection paths).
- `config.example.yaml:147-160` (`deepseek-flash`, off-peak, source-named); the loop execution model
  (the promotion path, where this ADR's step lands).

## Publication note (2026-10-03, R62-2)

The `R<n>` labels and finding ids in this document name iterations of the project's own
private analysis loop — a loop that is not part of this repository, so no label here is
resolvable by a reader of it; they are kept as the provenance of the decision. This
publication pass removed only the dead-pointer class: every reference into that loop's
working tree (its file paths and round-record names, its state record, charter, execution
model and replay contract, its scripts and module names, and the kanban card ids), each
replaced by the neutral phrase the sentence needs. Nothing else moved — no figure,
threshold, `§`/`ADR`/`CONF` id, code sample or contract sentence.
