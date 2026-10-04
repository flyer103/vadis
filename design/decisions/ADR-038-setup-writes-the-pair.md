# ADR-038 — `vadis setup` writes the pair: an inline roster is moved, never preserved

- Status: accepted
- Date: 2026-09-26
- Related: **ADR-025** (the write strategy this ADR extends without contradicting: the anchored edit on a
  verbatim template — decision 1/7's *no insertion, no deletion, no reformatting* is about **the wizard's
  answers**, and D3 below states exactly which bytes of the file the shape step may move); **ADR-037** (the
  pair this ADR completes: D9's *one root, at most one roster, no precedence ladder, no key-by-key
  overlay, no second discovery candidate*, and the identity digest whose value changes **once**, at the
  migration); AGENTS hard constraints **1**
  (the byte boundary), **2** (content determinism), **8** (docs before code) and **9** / ADR-012 (the
  measure is not the search space — this ADR changes no gate, no corpus and no conformance assertion);
  spec §4.11 (the *target file* column), **§4.14** (the two shapes); DESIGN §12.14 (the writer);
  ADR-020 / ADR-021 (the citations and the price tables the moved bytes carry as comments).
- Numbering note: the register holds **37** ADRs (`ADR-001` … `ADR-037`), so **038** is the next free number.
- Scope note: **the reader is untouched.** `serve`, `stats` and `vadis setup --check` load an inline root
  exactly as they do today — §4.14's *"the inline shape is not deprecated"* keeps its full meaning for the
  file that is there, and loses its meaning only for the file **this wizard** writes. No parse rule, no
  loader message, no `TRACE_SCHEMA_VERSION`, no dependency and no conformance assertion moves.

## Background — the measured problem

**Two inline roots and one wizard, measured on one machine on 2026-09-26.** Every file the discovery order
of spec §4.12 can find on the owner's machine carries the roster **inline**, and neither is ever given a
roster file:

| File | The roster's block | Lines | Bytes | `<roster>` beside it |
|---|---|---|---|---|
| `~/.config/vadis/config.yaml` (the XDG candidate, and the file a bare `vadis setup` resolves) | `providers:` at line 100 … line 1058 | 959 | 71 069 | **none** |
| `<repo>/config.yaml` (the CWD candidate) | `providers:` at line 78 … line 739 | 662 | 36 517 | **none** |

The two halves of the wizard's behaviour, measured at `ddd7a75` with the installed
`<home>/bin/vadis` (a build of 2026-09-26 10:01 that **does** embed the split template — `strings` finds
`providers_file:`, so the observation is not a stale binary):

- **A fresh target writes the pair.** `vadis setup --non-interactive --config <a path that does not
  exist>` writes `config.yaml` (14 960 B) **and** `providers.example.yaml` (71 070 B), exit 0, both `0600`.
- **An existing inline root writes nothing and creates no roster.** `vadis setup --non-interactive
  providers --config <a copy of the inline root>` prints `no change: … left as it is`, exit 0, and the
  directory still holds exactly one file.
- **A bare `vadis setup` in the repository root edits the XDG file** (`vadis setup --print` reports
  `# <home>/.config/vadis/config.yaml`), because §4.12's second candidate outranks its third:
  the file a user thinks they are configuring is not always the file the wizard opens. Both candidates are
  inline, so the shape outcome is the same either way.

**The cause is a decision this ADR reverses, not a defect.** R43 (§4.14's split) preserved the shape of an
existing file on purpose, and DESIGN §12.14 said so in terms: *"it creates no key: a root that carries the
roster **inline** is not converted to `providers_file:` by `setup` — that edit would be an insertion into a
file that never had the key, and it stays hand-work."* Two consequences followed, and both are
user-visible:

1. **The roster file exists only for users whose first run happened after the split.** A config written
   before it stays inline **forever**: a bare `vadis setup` over it is the no-op measured above, and the
   only documented migration (`--force`) replaces the whole root with the template — every hand edit that
   is not in the template survives only at `config.yaml.bak` (spec §4.11's `--force` row: *"What it
   discards is any note **you** wrote into your own file"*). That is a reset, not a migration.
2. **The silence is indistinguishable from a failure.** After a full `vadis setup`, nothing appears beside
   the config, and no line says why — which is how this ADR's own problem was reported (〈我发现最后不会自动
   生成 provider 文件〉, 2026-09-26).

**The owner's ruling (2026-09-26).** 〈加上 `--split` 这种参数会给用户带来使用负担，要求默认 setup 时
config 和 providers 文件是跟开的〉— the wizard's **default output is the pair**, and the shape is not a
user-facing choice. When this ADR's first draft offered a flag, a section-scoped split and a
refuse-on-conflict rule as the alternatives, the owner chose: the name is the shipped one, **any** writing
run splits, and a conflicting roster is **backed up and overwritten** rather than refused.

## Decision

**D1 — The run's shape is always the pair.** No `vadis setup` run — bare, `all` or one section, interactive
or `--non-interactive`, over a fresh target or an existing file — leaves the root carrying the roster
inline. A root whose **parsed** shape is inline-with-`providers` is normalized **before anything is
planned**, and the normalization is part of the run's candidate like every other change.

**D2 — The roster file's name is the shipped root's own.** When the root carries the roster inline there is
no `providers_file` value to obey, so the name comes from the **embedded template's own line** — the value
`config.example.yaml` itself carries (`providers.example.yaml`), read at run time and never written as a
constant in code (ADR-025's decision 4: the default a question shows is the file's own value, else the
template's — *never a constant in code*). Fresh run and
migration therefore name **one** file, and `G1`'s byte identity for a fresh target is unaffected.

**D3 — The move is a byte move, and the wizard writes no prose.** The moved span is the `providers:` header
line through the **last line the block owns** (the last non-blank line before the next top-level key;
trailing blank lines stay in the root, where they separate the root's remaining keys — and comment lines
never terminate a block, per the locator's own rule). That span's bytes **are** the roster file's bytes,
verbatim: entries, comments, `source:` citations and all. In the root, the header line's place is taken by
the **shipped root template's own `providers_file:` line** (terminator and all, the file's own `\n` or
`\r\n`), and every other byte of the root is untouched. This is a **move of the file's own bytes**, not an
insertion of new content: `ADR-025`'s rule forbids the wizard *composing* a line, and this line is the
template's, already written and already the reason §4.0's price numbers live where they do.

**D4 — The root's replacement line is derived, never literal.** It is read from the embedded template at the
anchor `providers_file` — the same `resolve_typed` the section table uses. A build in which the template
does not name its roster is a build whose wizard has no default name; the run refuses rather than inventing
one (and a unit test pins that the embedded pair resolves).

**D5 — A file that does not load is not repaired.** The normalization fires only for the shape the loader
parses as **inline with `providers`**. A root that writes **both** keys, one that writes **neither**, and one
that does not parse are left alone: no extraction, no roster lane, and the loader's own refusal stands —
exit 2, exactly as before. Silently converting a both-written refusal into a servable pair would be the
wizard *fixing* a file it was asked to configure, and §4.14's ladder is a contract about which shapes load.

**D6 — A conflicting roster is backed up, then overwritten.** When the root is inline **and** a file already
exists at the resolved roster name, the run takes `<roster>.bak` **unconditionally** (not only under
`--backup` / `--force`) and then writes the moved block over it. Nothing of the operator's is destroyed
silently, and the run does not stop for a condition it can make safe — the owner's ruling, and the reason
this ADR does **not** inherit ADR-025's *refuse rather than guess* shape here: the file is not being guessed
at, it is being replaced, and the old bytes are kept by name.

**D7 — The root gets no automatic backup from the shape step.** The block is not lost — it is the new file's
content — so the migration alone does not imply `.bak`. `--backup` copies the root when asked, `--force`
still implies it, and the refusal ladder is unchanged.

**D8 — Landing order is unchanged, and so is the atomicity.** The roster lands **first**, the root second
(an orphan roster is harmless; a root naming a missing roster is not), the pair is validated before either
lands, and a failure lands neither (G4 read for two targets).

**D9 — The move is reported, never silent.** The run's report and `--dry-run` name the span and the target
(`split: providers: lines 100–1058 (959 lines, 71 069 bytes) → …/providers.example.yaml`), and `--print`
over an inline root states that the file carries the roster inline **and** what a writing run would do with
it. Nine hundred lines moving without a line saying so is the failure mode this ADR exists to end.

**D10 — Idempotence is preserved.** The second run over the migrated pair changes nothing: the root is now a
pair, there is no span to move, and both files' bytes and mtimes stand (G3). A roster whose bytes already
equal the moved block is not written and not backed up at all.

## Alternatives considered, and why each was declined

| Alternative | Why it is not the decision |
|---|---|
| `vadis setup --split`, the migration as an explicit flag (this ADR's first draft) | **Owner-declined**: it makes the user carry a shape distinction the tool exists to decide. ADR-025 already argues the class (its `--reconfigure` lesson: no flags that restate behaviour), and spec §4.11 states it in terms: *"a flag that merely restates the default is a lie in a help text."* A wizard that writes two shapes needs a paragraph to explain which one you are in; one that writes one shape needs a sentence |
| Split only on a bare `vadis setup` / only when the run includes the `providers` section | **Owner-declined**: the shape must not depend on which section you happened to ask for. Two runs over one file would leave two shapes, and "did my config get split?" would become a question about argv |
| Keep the shape, document the hand-split in `book/` | The measured problem is that the roster file never appears; documenting the ceremony does not produce it. The wizard's whole job is to turn the template into your config, and the template has been a pair since R43 |
| Refuse when a roster file already exists beside an inline root | Declined by the owner in favour of D6's backup-and-continue: refusing turns a condition the run can make safe into a manual chore, and the operator's bytes are kept by name either way |
| Normalize a both-written or unparseable root too (extract the inline block, drop the other key) | **D5**: that is the wizard repairing a file that does not load. The refusal is a contract about shapes, and a run that "fixes" it writes a config the operator has never seen |
| Name the extracted roster `providers.yaml` | **D2**: a fresh run must stay byte-identical to the shipped pair (G1), so its roster is named `providers.example.yaml`; two naming rules for one wizard would be a worse defect than the `.example` spelling, which §4's `# Usage:` line already teaches for the pair |

## Consequences

- **A run that used to be a no-op now writes.** Over an inline root, a bare `vadis setup` (or any
  section-scoped run) prints a `split:` line and lands two files where it previously printed `no change`.
  `--dry-run` reports a write where it reported none. This is the decision, not a regression: the wizard's
  output shape is now a function of the tool, not of the file's history.
- **One operator-visible identity event.** `config_digest` (ADR-037 D6) and both files' `sha256` change at
  the migration and never again unless the content does. An operator tracking the digest sees exactly one
  step, on the run that migrated them.
- **The failure modes stay loadable.** Roster-first landing means every partial state still loads: root
  inline with an extra roster beside it (a stray file §4.12 never reads), or root naming a roster that is
  already written. No state exists in which the root names a file that is not there.
- **The `.example` spelling now appears in live configs.** `~/.config/vadis/providers.example.yaml` is a
  user's own roster. The name is D2's consequence; §4's `# Usage:` line and the book already use it for the
  pair, and §4.14's *named, never searched* rule means the name is an operator's to change by hand.
- **Nothing else moves**: the anchored-edit strategy, the refusal ladder, `--check`'s probes, the section
  table's target-file column (which now **always** resolves Roster for `providers`, because the run's shape
  is always the pair), the location rule, the modes, and the dependency allowlist.

## What this ADR changes in the existing record

- **`design/DESIGN.md` §12.14** loses the sentence this ADR reverses (*"it creates no key: a root that
  carries the roster **inline** is not converted to `providers_file:` by `setup`"*) and gains the shape step
  in the run's order, the module row for `setup/split.rs`, and the two new tests' obligations.
- **spec §4.11**'s target-file paragraph and *"what is shipped since the example's split"* paragraph are
  restated: the column no longer collapses for an inline root, because an inline root does not survive a
  writing run.
- **spec §4.14**'s *"the inline shape is not deprecated"* keeps its reading side and gains the write side's
  statement.
- **One assertion of R43's own test set is amended, and named here rather than edited quietly**:
  `providers_edit_lands_in_the_root_under_the_inline_form` (`crates/vadis-cli/src/setup/mod.rs`) asserted
  that an answered `providers` question over an inline root moves the **root's** line. Under D1 the same
  question lands in the roster the run created; the test becomes that case, with the root's own bytes
  asserted *not* to carry the answer. The loader's inline control (CONF-85's `(f)` arm, `conf_85_roster_file.rs`)
  is not touched and stays green — the reader's contract is what this ADR preserves.

## Evidence this ADR rests on (all re-runnable at `ddd7a75`)

- The two spans above: `providers:` at `config.yaml:78` (662 lines / 36 517 B) and at
  `~/.config/vadis/config.yaml:100` (959 lines / 71 069 B), measured by scanning the files' own bytes for
  the header line and the last line the block owns.
- The three wizard probes quoted under *Background*, run with `<home>/bin/vadis` (2026-09-26 10:01 build) on
  throwaway copies under the scratch directory; the fresh-target probe's two file sizes are its own output.
- `providers.example.yaml` measured at 71 070 B / 960 lines, its 959 content lines byte-equal to the XDG
  root's inline span (71 069 B) — the inline configs on this machine carry the roster the split moved,
  which is why D3's byte move is the migration those files need rather than a rewrite.

## Dated note — 2026-09-26 (R46-0; the accuracy items **R44-F1** and **R45-1-F1**)

**This note is appended because this ADR is append-only: not one line above it is edited.** Two of its own
sentences are corrected here, and both corrections are pointers rather than restatements.

**1. The attribution — `R44-F1`.** The `Related` bullet (`:13`) and the third bullet of *What this ADR changes
in the existing record* (`:164`) both speak of *"the inline shape is not deprecated"* as **spec §4.14's**
sentence, and the second of the two claims that §4.14 *"gains the write side's statement"*. Neither is true:
the phrase is **this ADR's own**, §4.14's bytes are **untouched by R44** — §4.14 begins at
`docs/spec.md:1401` in the file as R46-0 reads it (it began at `:1382` at R44's own commit), and R44's spec
hunks were all at or below `:1111` then — and the write-side statement landed in **§4's preamble** and in
**§4.11**'s shape step. At HEAD the two readings live at `docs/spec.md:312-313` (*"…is not deprecated **on
the reading side**"*) and `docs/spec.md:1061` (the shape step). A reader who follows either bullet to §4.14
finds the load-side contract, which is a different sentence.

**2. The scope note's reading (`:16`–`:19`) — `R45-1-F1`.** *"the reader is untouched. `serve`, `stats` and
`vadis setup --check` load an inline root exactly as they do today"* was true of the **load** and of the
**exit codes**, and R45-1 (2026-09-26) measured the one thing it read as covering and no longer describes:
over an inline root, `--check` prints one roster-fact line above its names (a `roster` member in `--json`) —
the fourth surface **D9** did not name. The current reading, as the contract carries it since R45:
**the load is the whole of what is unchanged** — `docs/spec.md:1025` (the `--check` row), `:1079-1083` (D9's
surface list, extended by a dated note) and `:1108-1111` (the qualifier) — while the printed surface gains
that one fact for an inline root and moves nothing else: not a name, not an export snippet, not an exit code.

**What this note does not do.** It re-opens no decision (D1–D10 stand), changes no behaviour, and restates no
contract: it points at the sections as R45 left them. Both items stay in the register until the round that
reads them marks them closed; a future reader of `:16`–`:19` or `:164` should read this note with them.


## Redaction note (2026-10-03, R62-1)

The owner home path inside the quoted `vadis setup --print` log line was
replaced with `<home>`. The logged behaviour and the decision are unchanged; only
the machine-local path prefix was redacted for publication.

## Publication note (2026-10-03, R62-2)

The `R<n>` labels and finding ids in this document name iterations of the project's own
private analysis loop — a loop that is not part of this repository, so no label here is
resolvable by a reader of it; they are kept as the provenance of the decision. This
publication pass removed only the dead-pointer class: every reference into that loop's
working tree (its file paths and round-record names, its state record, charter, execution
model and replay contract, its scripts and module names, and the kanban card ids), each
replaced by the neutral phrase the sentence needs. Nothing else moved — no figure,
threshold, `§`/`ADR`/`CONF` id, code sample or contract sentence.
