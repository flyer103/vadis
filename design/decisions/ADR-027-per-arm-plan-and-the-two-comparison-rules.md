# ADR-027 — the run plan is a function of the arm, and the two rules the live path needs to be honest

- Status: accepted
- Date: 2026-09-22
- Related: AGENTS hard constraints **1** (the byte boundary — this ADR settles *which* bytes each arm
  sends, and it exists because the byte audits are the reason the two comparison rules below are written
  down), **4** (no unverified savings — none of these rulings prints, invents or claims a figure),
  **5** (no fabricated prices — the value rule exists so that an unchanged price table cannot read as
  changed), **6** (tests assert relations, not snapshots — the dialect's witness is a *type* assertion
  over the shipped example, not a snapshot of it), **9** / **ADR-012 item 2** (the measurement is not
  part of the search space: **condition 0, the fixed corpus, every threshold and `tests/conformance/`
  are untouched by this ADR**), **7** (English only); the harness contract's **COR-2.5** (the two
  orders), **ARM-3.3.7** (a live arm's providers are verbatim), **ARM-3.5** (the driver), **ARM-3.9**
  (the two pairing kinds), **VER-4.1.1** / **VER-4.4 rev 3** (the five conditions), **PROV-8 §9.3.0**
  (the declaration ban), **REAL-9 §10.1** (the printed call count), **ACC-11 §12.4** (`ACC-12.2`); the
  first real-upstream run's findings, the loop live-run findings (defects **L1**, **L2**,
  **L3**; defect **L4** deliberately left to R26); the frozen corpus
  the recorded codex-pair manifest; the user's ruling recorded in commit `e49fcf4`
  (2026-09-22 23:53 +08:00) and in the loop state record; the round's plan of record (**R24** = L1 + L2 +
  L3, **R26** = L4).

## Background

On 2026-09-22 21:01 the operator ran the first real-upstream replay (cold then warm, 32 upstream calls,
$0.000781 actually spent, the loop live-run findings §"Operator declaration used"). It
produced no `verified` label — and, more importantly, it **could not have**: the three defects it exposed
are in the apparatus, not in the product, and each one is a blocker of a different kind.

**L1 — the plan contradicted the contract's own condition 0.** REAL-9 §10.1 defined the printed call count
as `len(corpus.run_plan(order)) × 2 × repeats`, i.e. *every* planned item is sent to *every* arm; the
driver did exactly that (`replay.py:1428` builds the same plan for both arms). Condition 0 (VER-4.4 rev 3)
is derived from the sessions the harness **actually sent** (`replay.py:2162-2176`): `shared = sessions_a ∩
sessions_b; sessions_distinct = bool(a) and bool(b) and not shared`. Sending the same measured items to both
arms makes that intersection non-empty whenever any measured item has a session identity, so condition 0 was
**unsatisfiable on the live path by construction**, and with it `verified` was unreachable for *every*
corpus. Observed in both rows: `[fail] provenance.session_namespace_distinct: both arms sent session
'01a0c7a5-1b1f-7b83-8c7e-b6eb51b527fa'`, `verified_ineligible_reason: condition-0 shared-cache-namespace`.

Two facts decide the direction:

1. **The corpus already described the plan the driver was not running.** The frozen manifest's
   `labelling_rule` says it-01…it-06 and it-08…it-13 are prelude (each warming *its own session* before its
   own measured turn) and that it-07 / it-14 are "the last request of its own session"; the `[[pair]]`
   block's `session_note` says the two items are the last request of the same script run twice, in two
   distinct client sessions. The corpus is the human's frozen act; the *plan* violated it, and the plan is
   the thing that has to move.
2. **The old plan did not merely fail the check — it created the confound the check exists for.** Under it,
   arm a sent *both* sessions' preludes (warming S2 before arm b measured in S2), so on a live path the
   second arm would have been measured against a prefix cache the first arm had just warmed, i.e. exactly the
   bias `ARM-3.9.1` was written to exclude and `ARM-3.9.2` names for `paired-arms`.

The plausible alternative — keep the plan, derive condition 0 from the pair mapping — is the one direction
that must not be taken: it converts a byte-level predicate derived from what was sent into a read of the
human's declaration, which is the class of defect R12-F1 was (a row that says `verified` because it *says*
it is a verified-shaped run). The user's ruling settles it: **the plan honours the corpus's own `[[pair]]`,
so condition 0 is satisfied by what was sent; condition 0, the corpus, the thresholds and
`tests/conformance/` are untouched** (`e49fcf4`).

**L2 — the round-trip between the harness's own emitter and parser was asymmetric.** The live audit
`config.live_providers_verbatim` compares each arm's generated `providers[]` entry with the base's. The base
is parsed by `parse_simple_yaml` (`load_base_config`, `replay.py:833-844`); the generated config is *written*
by `yaml_scalar`/`yamlify` and parsed by the same function. `yaml_scalar` renders a float with `str(v)`
(`replay.py:585-586`), so the base's `input_hit: 0.000022` is emitted as `input_hit: 2.2e-05`, and
`_yaml_scalar_parse` (`replay.py:675-707`) does not read exponent notation, so it reads that value back as
the **string** `'2.2e-05'`. Measured at this HEAD: `yaml_scalar(2.2e-05) → '2.2e-05'`,
`_yaml_scalar_parse('2.2e-05') → '2.2e-05'` (type `str`), and the two are not equal. The audit then reported
`[fail] config.live_providers_verbatim: arm a: providers differ from the base: deepseek: changed ['models']`
on providers that had not changed at all. Because the audit's failure sets the same lumped flag the ladder
reads, this single asymmetry also marked condition 3 as failed on both R23 rows. The real vadis accepts
`2.2e-05` (the arms ran), so this was purely the harness comparing two things it had rendered differently.

**L3 — a flow collection nested in a flow mapping was read as a string.** `parse_simple_yaml`'s flow-mapping
branch reads each member with `_yaml_scalar_parse` (`replay.py:675-707`), which never re-enters
`_yaml_value`, so `- { days: [Mon, Tue, Wed, Thu, Fri], from: "01:00", to: "04:00", tz: UTC }` yields
`days` = the **string** `'[Mon, Tue, Wed, Thu, Fri]'` while the block spelling yields a real list (both
measured at this HEAD). The generated arm config then carried a string where the vadis requires
`expected a list of three-letter weekdays`, the arm never became ready, and the run ended at exit 3 with zero
spend. The consequence is wider than the one line the findings named: **every** `peak: { multiplier: …,
windows: [] }` in the shipped `config.example.yaml` also comes back with `windows` as the string `'[]'` — a
read-only probe of the parser at this HEAD counts **48** string-typed flow leaves in that file (44 `windows`,
4 `days`), i.e. the shipped example is not usable as a base as shipped, and
the operator's live base was a hand-rewritten copy of it (block style), disclosed in a comment rather than in
the declaration.

**The defect this freeze found while writing it (the second half of L4's ground).** `audits_pass` is
computed as `not mechanism_failed` (`replay.py:2204`), and `mechanism_failed` is set only by a *failure*
(`:2331` and the audit loop), never by a check that did not run. On a live run no mock listener records the
wire, so both byte audits are reported `[not_run] bytes.control.only_two_mutations` /
`[not_run] bytes.treatment.only_declared_edits` ("no measured items reached the upstream") — and a `not_run`
check leaves `mechanism_failed` false. With L1 and L2 fixed and a clean tree, a live run would therefore
reach `saving_label: verified` with **both byte audits `not_run`**: AGENTS constraint 1's checkability
claim, failing in the one place it is load-bearing. VER-4.1 already says a check that did not run "is never
a pass", and VER-4.4 condition 3 says the audits must **pass on every item**; the implementation's lumped
flag is the deviation, not the clause.

## Decision

**1. The run plan is a function of the arm.** When the corpus's `[[pair]]` is `kind = "paired-sessions"`, the
plan is per-arm: arm `policy_a` sends `pair.item_a` and — in `warm` — the prelude items **of that item's own
session**; arm `policy_b` sends `pair.item_b` and its own session's prelude; in `cold` each arm sends only its
own measured item. The two arms therefore send **disjoint item sets in disjoint sessions**, so
`provenance.session_namespace_distinct` is satisfied by what was sent and condition 0 is untouched, character
for character. `paired-arms` keeps today's behaviour exactly (both arms send the whole plan) and keeps the
label it has always had (`inferred`, reason `unpaired-session`). The plan reads the corpus — it must, since it
decides what to send — while the label still reads only the sent bytes; the two stay separated because the
label derives its sessions from the arm's own evidence file (`sessions_sent`), never from the manifest.
REAL-9 §10.1's printed count becomes the per-arm sum, `n_calls = repeats × Σ_arm |plan(arm, order)|`, which
reduces to the old formula for `paired-arms`. The frozen `codex-pair-2026-09-22` corpus prints **cold 2
(1+1)**, **warm 14 (7+7)** where R23's driver printed 4 and 28.

**2. `config.live_providers_verbatim` compares parsed values, to the leaf.** Both sides are parsed by the one
parser, the comparison is recursive, and it reports every differing leaf with its path, both values and both
types. There is **no tolerance and no coercion**: numbers compare by value, a `str` never matches a number (a
type change is a change), and a missing or extra key, list element or provider entry is a difference. The
emitter/parser pair is required to **round-trip by value** — for every scalar the generator writes, the parser
reads back an equal value — which is the invariant today's `2.2e-05` violates. The failure detail names the
leaf, e.g. `arm a: providers[deepseek].models[0].price.peak.windows[0].days: base ['Mon', …] (list) !=
generated '[Mon, …]' (str)`, instead of `changed ['models']`.

**3. The accepted base dialect is the shipped `config.example.yaml`'s own dialect.** Nested block maps; block
lists of scalars and of maps; flow mappings `{…}`; flow sequences `[…]`, including **nested inside a flow
mapping** and inside another flow sequence, recursively; empty flow collections; quoted and bare scalars;
`#` comments. Anything else — anchors/aliases, tags, block scalars, document markers, tab indentation — is a
refusal naming the line, never a silent coercion. The witness is a type assertion over the shipped file, not
a snapshot: the example must parse **and** its `peak.windows` / `windows[].days` leaves must be lists (today
they are the strings `'[]'` and `'[Mon, Tue, Wed, Thu, Fri]'`).

**4. Condition 3 is satisfied by three `pass` verdicts, never by an absence (fail-closed clarification).**
`bytes.control.only_two_mutations`, `bytes.treatment.only_declared_edits` and `ledger.edit_counts_match_wire`
must each be `pass` on every paired item; a `not_run` byte audit **fails** condition 3 and the reason names
the ids. The lumped `not mechanism_failed` flag may not stand in for the three verdicts. This changes no
threshold and no condition; it is the sentence VER-4.1 and VER-4.4 condition 3 already imply, written so that
a live row cannot pass condition 3 by never attempting it.

## Alternatives considered

**Plan unchanged, condition 0 derived from the pair mapping (option (b)).** Rejected: it makes the label read
a declaration (`[[pair]].kind` / `item_a` / `item_b`) instead of the sent bytes, which is R12-F1's class, and
it would leave the live wire carrying the exact bias ARM-3.9.1 exists to exclude (the later arm measured
against a prefix cache the earlier arm warmed). It is also unnecessary: the corpus's own `labelling_rule`
already states the per-arm shape, so honouring it costs nothing and changes no corpus byte.

**Relax condition 0 to "the two arms' *declared* sessions differ".** Same defect, one level down: the
declaration is written by the same actor whose run is being labelled, and PROV-8 §9.3.0 explicitly bans it.
Rejected.

**Keep text comparison and force the emitter to write literals the base would have written (L2).** Rejected
as the primary rule: it makes the audit a self-consistency test of the harness's own renderer rather than a
statement about the value the vadis will load, and it still fails a base whose (perfectly legal) spelling
differs from the emitter's — which is precisely L2's false failure. The emitter half is kept, but as an
*invariant* (parse(emit(v)) == v), not as the comparison's premise.

**Compare floats with a tolerance (L2).** Rejected: the entry this audit protects is a **price table**
(AGENTS constraint 5). A tolerance would silently accept a real price change smaller than itself. Exact
numeric equality is attainable because `str(float)` is the shortest round-tripping decimal, so the tolerance
would buy nothing and cost the one property the check has.

**Declare the dialect to be "block style only" and require operators to rewrite their base (L3).** Rejected:
the shipped `config.example.yaml` is the committed contract file the runbook tells a reader to check bases
against (ARM-3.3.5); requiring a hand-rewritten copy makes the measured input a transformation nobody can
reproduce from the tree — the undisclosed-difference class ARM-3.3.2 refuses — and R23's live base was exactly
that copy, disclosed only in a comment. Extending the parser to read what the tree already writes is the
smaller, checkable move.

**Look the other way on the `not_run`-counts-as-`pass` seam (Decision 4), and let R26 close it.** Rejected:
after L1+L2 a clean live run would carry `verified` with no byte audit at all. A false `verified` is worse
than an unreachable one, and the fix is one sentence plus one comparison — no threshold moves, and the
consequence (condition 3 fails until a live byte audit exists) is the *honest* state.

## Rationale

- **The plan is the arm's, because the measurement is the sent bytes.** Condition 0 is a predicate over what
  the two routers actually sent; the only way to make it satisfiable without weakening it is to send what the
  corpus describes. That is why the ruling keeps condition 0, the corpus and the thresholds untouched and
  moves the plan instead.
- **The plan may read the corpus; the label may not.** The corpus is the human's frozen act (COR-2.4), and the
  plan is a function of *inputs*; the label is a function of *evidence*. Keeping those two apart is what lets
  §9.3.0's ban stay exactly as written while the plan becomes corpus-aware.
- **The count must equal the requests the run can really make.** §10.1's line exists so an operator sees the
  spend-side magnitude before anything starts; `len(plan) × 2` was a lower-bound-shaped number that matched
  the old send pattern and no longer describes any run. The per-arm sum is the same statement for both pairing
  kinds.
- **A verbatim audit must compare values, because the base is human-written YAML.** The harness is not a YAML
  writer; it is a subset emitter. Comparing its output *textually* against a human's file tests the emitter,
  not the config, and it fails on a legal spelling of an unchanged number — the observed L2 failure.
- **The dialect must cover the file the tree ships.** The alternative makes every operator's base a
  hand-edited artifact and turns "what was measured" into something a reader cannot reproduce.

## Consequences

- `provenance.session_namespace_distinct` becomes satisfiable on the live path — and testable for free: a
  **mock** run of a `paired-sessions` corpus now sends disjoint item sets, so the check passes while the label
  stays `inferred` (`mock-upstream`), because condition 1 and not condition 0 is what a mock run fails. No
  live call is needed to test the L1 fix.
- The two arms' rows are no longer index-aligned by item id: the paired unit becomes the **pair**
  (`item_a ↔ item_b`), and every per-arm check (session-vs-trace, byte audits, ledger byte counts,
  `rows == items`) evaluates an arm over **its own** items. VER-4.1.2 freezes this; the row's arithmetic
  (VER-4.2.1) keeps its shape, with `i` ranging over paired units.
- `config.live_providers_verbatim` stops firing on an unchanged provider; the message it prints when a
  provider *did* change names the leaf.
- `parse_simple_yaml` gains exponent notation and recursive flow collections; `config.example.yaml` becomes
  usable as a base as shipped, and a base no longer has to be a block-style rewrite.
- Condition 3 now fails on the live path (both byte audits `not_run`), so **`verified` remains unreachable
  after R24** — by design and by name. R26 (L4) makes the audits run on a live wire; until then a live row
  says `condition-3 byte-audits ([bytes.control.only_two_mutations, …])` instead of claiming a pass it did
  not attempt.
- Nothing in `crates/`, `tests/`, `docs/`, `book/` or the corpus moves. The corpus digest is unchanged
  (`607ffac6…`, COR-2.2.6's recipe covers the item rows, which no clause here touches).

## Honest boundaries

- **This ADR mints no `verified`, and the round it belongs to claims none.** What it does is remove two
  structural impossibilities (condition 0 by construction, the verbatim audit by a rendering asymmetry),
  extend the accepted base dialect, and close one seam through which a *false* `verified` would have been
  reachable. The cost gate's criterion (the loop charter D3, a `verified` net gain > 0) is untouched and, on the
  frozen corpus, still unmet: the corpus's own `exclusion_note` says it does not exercise the compression
  target, and R23's measurement found `delta.input_total = 0` in both orders — so even a fully eligible run
  of *this* corpus would report a delta of zero (AGENTS constraint 4: no figure is presented as a saving).
- **L4 is not closed here.** The live byte audits still do not run (there is no recording of what a real
  upstream received); Decision 4 only stops their absence from counting as a pass. Whether a live byte audit
  is possible at all — from the vadis's own trace plus an `upstream/` equivalent — is R26's question.
- **The per-arm plan is frozen for one pair.** A `paired-sessions` corpus declares exactly one `[[pair]]`;
  the generalisation (several pairs folded per arm) is deliberately not taken, and a corpus that declares
  more is refused rather than measured approximately.
- **The plan rule is a harness clause, not a gate.** No gate definition, no fixed corpus, no conformance
  assertion and no envelope is touched (AGENTS 9 / ADR-012 item 2); the change is registered as a contract
  revision whose text is reviewable in the tree.
- **What was not executed while writing this**: no arm was started, no mock was run, no upstream was called.
  The two probes behind the L2/L3 statements were read-only calls to the harness's own parser and emitter at
  this HEAD (`e49fcf4`), and their outputs are quoted in the contract's rev-5 facts table.
- **One residual seam is registered and not ruled on** (contract §15.5): condition 0's derivation counts an
  *absent* session identity as a distinct one (`sessions_a = [None]` is truthy and `shared` filters `None`
  out, `replay.py:2162-2176`), so a run whose measured items carry no session would pass the check while
  reporting "distinct observed client session identities". The per-arm plan makes that shape unreachable for
  a `paired-sessions` pair (COR-2.5's refusal 2 requires a non-null session), but the derivation itself is
  untouched here — condition 0's sentence is not this ADR's to edit, and the round that re-opens a live label
  is where it belongs.

## Reversibility

- **The plan rule — reversible only by re-measuring.** It is text in the loop replay contract, so a later
  revision can change it; but every row produced under it describes what *was* sent, so reverting means the
  affected runs are no longer comparable to newly produced ones. Rows produced before it are not invalidated
  (they remain honest records of the old plan), which is why the old formula and the R23 numbers are kept in
  the text as history.
- **The value rule and the dialect — cheaply reversible.** Both live entirely inside the harness's parser,
  emitter and comparator; reverting changes no artifact format, no row field and no exit code.
- **Decision 4 — reversible, and that is its point.** It is one comparison; if a future live byte audit
  makes the audits run, the clause is satisfied rather than relaxed.

## Publication note (2026-10-03, R62-2)

The `R<n>` labels and finding ids in this document name iterations of the project's own
private analysis loop — a loop that is not part of this repository, so no label here is
resolvable by a reader of it; they are kept as the provenance of the decision. This
publication pass removed only the dead-pointer class: every reference into that loop's
working tree (its file paths and round-record names, its state record, charter, execution
model and replay contract, its scripts and module names, and the kanban card ids), each
replaced by the neutral phrase the sentence needs. Nothing else moved — no figure,
threshold, `§`/`ADR`/`CONF` id, code sample or contract sentence.
