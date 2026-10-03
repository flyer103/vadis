# ADR-046 — `router setup` lands the rule file its own template names

- Status: accepted
- Date: 2026-10-03 (round R60's contract card, `R60-1`; the owner's authorization recorded in
  `autowork/STATE.md`'s paragraph *\"Refreshed 2026-10-03 11:19\"*, itself committed on the round branch
  **before** any card of the round was cut)
- Kind: **a product-behaviour contract, docs-only in this card.** It adds no gate, changes no threshold, moves
  no corpus, touches no conformance assertion, and changes no product byte *here* — `R60-2` implements it and
  `R60-3` verifies it. The measurement (the gate definitions, the corpus, the L1 envelope) is not part of this
  decision's search space (`AGENTS.md` constraint 9 / ADR-012), and no saving, latency or cost figure of any
  kind is minted by the round.
- Related: **ADR-025** (the write strategy this ADR extends without contradicting: the anchored edit on a
  verbatim template — *the file is **edited**, never reproduced*); **ADR-038** (the pair, and D6's
  *backed-up-then-overwritten* shape and D1's *the run's shape is always the pair*, whose lane semantics this
  ADR's third file follows); **ADR-037** (the roster as its own named file; §4.1's one resolution rule);
  **ADR-003** (saving cost is a declarative, revertible, individually accounted pipeline — the rule set is
  **data**, the fact that declines the compiled-in alternative below) and **ADR-008** (the three-level rule-file
  override and its trust gate); **ADR-019** (content determinism and the mode the client asks for); `AGENTS.md`
  constraints **1**
  (the byte boundary — this file is not on a request path), **2** (content determinism), **5** (no fabricated
  prices — the rule file carries none), **8** (docs before code) and **9** / ADR-012; spec **§4.4** (the
  rule-file format), **§4.11** (the writer and its boundary), **§4.12** (where a path written in the file
  lands), **§4.14** (the pair, and its *Shipped* paragraph); DESIGN **§12.14** (the writer's landing);
  `tests/conformance/tests/conf_86_setup_check_roster_fact.rs` (the only case that pins `router setup`'s
  output — it pins `--check`'s **stdout**, which this ADR does not move).
- Numbering note: the register holds **45** ADRs (`ADR-001` … `ADR-045`), so **046** is the next free number.

## Background — the measured problem

**Every fresh `router setup` landing starts with a named absence, and the absence is load-bearing.** Measured
by the opener (2026-10-03), not inferred, and reproduced verbatim against a copy of the owner's own
`~/.config/router/config.yaml` on another port:

```
router: transform_rules: rule file <home>/.config/router/rules/tool_output.toml:
        No such file or directory (os error 2); not loaded
        (a request that asks for transform mode runs with an empty ledger)
```

The server starts normally afterwards (`router listening on 127.0.0.1:8793 … [store open], auth: required`),
which is what makes the absence a **silent** one: nothing fails, so nothing tells the operator. Its
consequences are concrete: `Forwarder.transform_engine` is `None`; a request carrying
`X-Router-Transform: transform` is *asked, not applied*, with an empty ledger; and `/health`'s `plugins`
member still reads the declared entry, because that member reports the **declared config**, not the mounted
set — so the one surface an operator would check agrees with the config file and not with reality.

**The cause, from the code, is one missing write.** The shipped template's plugin entry names the rule file:

```yaml
  - id: tool-output-rules
    kind: builtin/transform_rules
    config:
      rules_file: ./rules/tool_output.toml        # resolved against THIS config file's directory
```

That value is resolved by spec §4.1's in-file rule (`crate::config_load::resolve`,
`crates/router-cli/src/config_load.rs:219`) against **the directory containing the config file**, never the
CWD. The file it names exists **only in the repository** (`rules/tool_output.toml`, 16 897 B, sha256
`97c1f272…`, 4 rules / 13 inline tests). `router setup` writes the root and the roster from its embedded
templates — spec §4.14's *Shipped* paragraph says so in terms: *\"`router setup` writes both files from its
embedded templates\"* — and materializes **nothing** for the third file that same template names. So the
command the operator runs to get a working configuration is precisely the command that produces the dangling
reference.

**What is *not* broken, said so the fix stays narrow.** The documented no-command path is fine: a config
copied into the **repository root** resolves `./rules/tool_output.toml` correctly, because §4.1 resolves it
against the config file's own directory and the repository's `rules/` sits beside it. The trap is exactly the
`setup` landing — the XDG install, where nothing sits beside the config that the template's relative path can
find.

**The owner's ruling (2026-10-03, verbatim):** 〈规则文件跟随 roster 语义（--force 可替换，替换前 .bak）〉 —
the rule file follows the **roster's** semantics: created when absent, untouched by a routine run, replaced by
`--force` when the `plugins` section is selected, the previous bytes kept at `<file>.bak` before the write.
The method is the owner's too (「选择方案 2，通过 kanban + profiles 来解决」): a fix to the repository, driven
through the board and the profiles. This ADR is the semantics; `R60-2` is the code.

## Decision

**D1 — The rule file is a third write target of `router setup`, read from the base config.** A writing run owns
the file its **base config** names at `plugins[id=<entry>].config.rules_file` on its `builtin/transform_rules`
entry (the shipped template's entry is `tool-output-rules`, naming `./rules/tool_output.toml`), resolved by
spec §4.1's rule against the directory containing the config file — never the CWD. The path is **read**, never
prompted and never invented: the `plugins` section's askable key set is unchanged (the shipped section table
carries `plugins[id=<entry>].disabled` only — `crates/router-cli/src/setup/sections.rs:314-328`), and the run
composes no key of its own. A base that names no rule file — no `builtin/transform_rules` entry, or one
without `config.rules_file` — has **no** third target and materializes nothing.

**D2 — Its bytes are the template embedded in the binary.** The bytes written are the repository's own
`rules/tool_output.toml`, embedded by path beside the root example and the roster example
(`crates/router-cli/src/setup/mod.rs:28`/`:35` today) — the third embedded template, embedded for exactly the
reason spec §4.11's `--from` row already gives for the other two: **an installed binary with no example beside
it must still land a working bundle.** No `--from` names it: there is one embedded rule set, and a selectable
rule template would be an additive change with its own card.

**D3 — The lane follows the roster lane's semantics, exactly.** Absent → **created**, reported. Present →
**untouched** by a run that does not replace it: bytes and mtime unmoved, `no change` the most it says.
`--force` **with the `plugins` section in the run's section list** → **replaced** by the embedded template,
the previous bytes kept at `<file>.bak` **before** the write (spec §4.11's `--force ⇒ --backup` row; the shape
step's own D6 shape). `--force` without the `plugins` section, and every run without `--force`, replaces
nothing. This is the roster lane's own rule read for the third file —
`replaced = !exists || (force && <the section that owns it is selected>)`
(`crates/router-cli/src/setup/mod.rs:299-306`), whose *create* arm is likewise unqualified by section: a
`router setup server` over a fresh target lands a complete, working bundle, for the same reason it lands the
pair.

**D4 — A roster-scoped replacement is not a rule-file replacement.** `router setup providers --from <roster>
--force` over an existing split root swaps the **roster** as a unit (spec §4.11's target-file row, ADR-037 D9)
and does not replace the rule file: the `plugins` section is not in that run's section list, so only D3's
create arm can touch the third file there, and it fires only if the file is absent.

**D5 — The run never edits a rule's content.** The rule file's bytes are the embedded template's own, always:
the wizard composes no rule, no value, no comment inside it, and its report says `0 edits applied` for that
lane truthfully. A rule's numeric knobs (spec §4.4's L1 envelope) are the file's own business and are edited
by hand, exactly as a price or a `source:` citation is.

**D6 — Modes: whatever the run creates is `0600`, and the directory `0700`.** Set explicitly, never left to the
umask; a directory or file that already exists is **not** re-moded, and a rule file the run **replaces** keeps
the mode it had (spec §4.11's landing rule — the `0600` is a **creation** mode — read for the third file).
The property is stated as what the filesystem holds afterwards, not as a claim about a code branch.

**D7 — Determinism is unqualified for this lane.** G7 holds trivially and absolutely: the bytes written are
the embedded template's own, so they are a function of the template alone — no timestamp, no answer, no CWD
dependence beyond §4.12's documented candidate (which selects the root, and thereby the directory the rule
file lands in), and nothing the run writes carries a time.

**D8 — Idempotence holds, and it is CONF-69's property read for the third file.** A second run with the same
inputs leaves the rule file's bytes **and** its mtime unmoved — over a file the first run created and over one
that was already there — exactly as `CONF-69` pins the target's own pair of hashes.

**D9 — The read-only surfaces do not move, and the dry run names the file.** `--check`'s stdout changes **not
one byte**: a rule file names no environment variable and is not one of the facts that surface states, and
`conf_86_setup_check_roster_fact.rs` is the only case that pins `router setup`'s output — it pins that
surface, so that surface is frozen. `--dry-run` **names the file it would create or replace** (the same
per-lane `# <path>` / `would write …` report the other two lanes already print). `--print` states **nothing
new**: the rule file's existence is a filesystem fact about a file beside the config, not a fact about the
config's content, and `--print`'s subject is the latter. The materialization is reported by the run's own
report and by `--dry-run`; the `--check` surface deliberately does not gain a rule-file line (a decision
recorded here rather than left to an implementer).

**D10 — The boundary is stated, not implied.** Spec §4.11's *boundary of what `setup` does not do* table gains
the row this makes true (what the command may materialize, and that a routine run does not clobber the
operator's rule file), keeping every existing row; its *\"what must be asserted when this lands\"* list names
the new assertions, which are in-crate tests — **no conformance case is added and none is amended** (the gate
side is the owner's: ADR-012 / `AGENTS.md` 9), because the lanes and their assertions live where `CONF-67` /
`CONF-68` / `CONF-69` / `CONF-70` / `CONF-79` already live (`R22-F4`'s in-crate table).

## Alternatives considered, and why each was declined

| Alternative | Why it is not the decision |
|---|---|
| **Leave the reference dangling** — do nothing | The measured defect **is** this alternative: every fresh XDG install starts with a named absence, the transform silently does not run, and `/health` agrees with the config rather than with the process. A named absence that nothing reports is the failure this ADR exists to end |
| **Document a manual copy step only** (`cp rules/tool_output.toml …` in `book/`) | Documentation does not produce the file, and the wizard's whole job is to turn the template into your configuration. R44 ended exactly this ceremony for the roster (ADR-038's *Background*: *\"documenting the ceremony does not produce it\"*); repeating it for the third file would make the trap a documented trap. It also fails the installed-binary case outright: a user with no clone has no `rules/` to copy from |
| **Ship the example's entry `disabled: true`** | This hides the absence instead of fixing it, and it changes a capability the user did not ask about: the shipped example would mount no rule engine, so `X-Router-Transform` would be a no-op **even on a site whose operator later enables the entry** — `disabled` is the start-up switch (spec §4.3), not a file-existence guard. It would also make the shipped example disagree with the roster it ships beside, for a reason that is about a missing file |
| **Embed the rule set as a built-in default instead of a file** | Declined by **ADR-003** and **ADR-008**: the rule set is **data** — a declarative, revertible, individually accounted pipeline (ADR-003) with a three-level override and a trust gate (ADR-008) — and a compiled-in default would (i) create a second source of truth for the rules, (ii) make `plugins[].config.rules_file` name a file nothing reads (a lie in the config, which `deny_unknown_fields` would not catch), and (iii) move the rules outside the operator's reach, which is the opposite of what ADR-003 bought |
| **Write the rule file on every run** (no roster semantics) | It would clobber an operator's own rules on a routine reconfigure — the one thing ADR-025's *refuse rather than guess* discipline and the owner's ruling both forbid. The roster semantics the owner chose (create when absent; replace only under `--force`) is what makes the file the operator's after the first landing |

## Consequences

- **A fresh landing is complete.** After one `router setup`, the config's `rules_file` reference resolves, the
  transform engine mounts, and the startup log carries no `transform_rules: … No such file or directory`
  note. This is the round's whole claim when it closes.
- **The wizard now writes a file whose content it never edits.** That is a new *class* of write for this
  command (a wholesale copy of an embedded template that is neither the target nor the pair's roster), and it
  is why D5 states the `0 edits applied` truth explicitly: the report must not imply an anchored edit where a
  copy happened.
- **One more surface a `.bak` can appear on.** Under `--force` with `plugins` selected, a third
  `<file>.bak` is written beside the pair's own — by name, before the write, the operator's bytes kept.
- **`--check` is the one surface that stays silent about it**, deliberately (D9). An operator who wants to
  know whether the rule file is there reads the run's report, `--dry-run`, or the file itself; `serve`'s own
  startup note remains the last word on whether the engine mounted.
- **Nothing else moves**: the section table's askable key set, the anchored-edit strategy, the refusal ladder,
  `--print`'s rendering, the location rule, the pair's target-file column, the identity digest's derivation,
  `TRACE_SCHEMA_VERSION` (2), the dependency allowlist (no new dependency), and every existing conformance
  assertion.

## What this ADR changes in the existing record

- **spec §4.11** gains the rule-file lane paragraph (D1–D9 in contract form), the boundary-table row and the
  new assertions in *\"what must be asserted when this lands\"*; its `--force` ⇒ `--backup` row is **cited, not
  changed**.
- **spec §4.14**'s *Shipped* paragraph's *\"writes both files\"* becomes the truth of the **trio** — the root,
  the roster it names, and the rule file the same template's `plugins` entry names. The roster's own
  paragraphs (the key, the ladder, the identity) are **untouched**.
- **spec §4.12**'s relative-path paragraph already names `plugins[*].config.rules_file` as a path resolved
  against the config file's directory (`docs/spec.md:1371-1373`); it is **checked, not restated** — it reads
  true as written and gains no sentence.
- **spec §4.11**'s *\"Shipped since the example's split\"* paragraph — the one beside this ADR's own lane, whose
  sentence read *\"the wizard embeds **both** templates, and a fresh run writes both files\"* — is extended by one
  clause naming the third template and the third file. It is the same claim as §4.14's *Shipped* paragraph read
  from the writer's side; leaving it would put a sentence the tree contradicts two paragraphs above the lane
  that contradicts it.
- **DESIGN §12.14** gains the lane as a numbered step beside the shape step (`1c`, labelled so that no step
  number and no cross-reference moves), the third embedded template in the module map, and the lane's
  assertions in *The rig*.
- **`config.example.yaml`** gains a **comment** on the `rules_file:` line (comments only; no value moves — the
  file is the template the run copies, so its bytes are what a fresh landing writes).
- **`book/getting-started.md`**, **`book/plugins.md`** and **`README.md`** each carry the third file on the
  sentence that already states the pair, **linking** this ADR rather than copying its wording (single source).
- **ADR-038 stands unamended.** Its D6 (backed-up-then-overwritten), D1 (the run's shape is always the pair)
  and D10 (idempotence) are what D3/D4/D8 above read for a third file; nothing in it is falsified by this ADR
  — checked sentence by sentence, and no dated note is appended.

## Evidence this ADR rests on (re-runnable at the round's base `442c9b1`)

- The shipped template's plugin entry and its relative value: `config.example.yaml:140-148` (`rules_file:
  ./rules/tool_output.toml`), read as the base the run starts from.
- The file it names and its bytes: `rules/tool_output.toml`, **16 897 B**, sha256
  `97c1f272811f48e90fa4b6da7f1cc737dd78e8a1b94a57414211d59fe3318102`, 4 rules / 13 inline tests (its own
  header, `rules/tool_output.toml:1-40`).
- The two templates the run already embeds, and the constants that carry them:
  `crates/router-cli/src/setup/mod.rs:28` (`EMBEDDED_TEMPLATE`) and `:35` (`EMBEDDED_ROSTER`).
- The roster lane's semantics this ADR's lane follows, read from the code:
  `crates/router-cli/src/setup/mod.rs:299-306` (`replaced = !exists || (force && targeted)`), `:162-196`
  (the `Lane` and its `lands()`), `:542-561` (the `<file>.bak` copy before a replacement of a file the run
  did not write), `:642` (`backup_path`).
- The section table's current askable set for `plugins` (no `rules_file` row):
  `crates/router-cli/src/setup/sections.rs:314-328`.
- The resolution rule the named path obeys: `crates/router-cli/src/config_load.rs:219`, cited by spec §4.1
  and restated in §4.11/§4.12.
- The measured defect: `autowork/STATE.md`'s *\"Refreshed 2026-10-03 11:19\"* paragraph (the opener's
  reproduction, the cause, and the collision surface), which this card is bound to and by which its own body
  is overridden where the two disagree.
- The round's own probes (this card, `$0.00`, offline, loopback only): `autowork/harness/r60-1/base-check.txt`
  (the shipped example still loads; 9 names, all absent under `env -i`, exit 4) and
  `autowork/harness/r60-1/base-surfaces.txt` (what `--dry-run` / `--print` / `--check --json` say today).

## What this ADR does not decide

- **Whether the wizard should ever *ask* about `rules_file`.** The `plugins` section's askable key set is
  unchanged; spec §4.11's section table names `plugins[id=<entry>].config.rules_file` among the keys the
  section *may* write while the shipped section table carries no such row (`sections.rs:314-328`) — a
  divergence registered by `R60-1` as a finding and left standing here, because widening the askable set is a
  different decision from landing the file.
- **A selectable rule template** (`--from` for the rule file). One embedded rule set, one behaviour; a second
  is an additive change with its own card.
- **Rule-content anchoring.** The wizard does not edit a rule's knobs (D5); if the L1 envelope ever wants a
  rule parameter editable through the wizard, that is a section-table change, not a lane.

## Redaction note (2026-10-03, R62-1)

The owner home path inside the quoted rule-file warning log block was
replaced with `<home>`. The logged behaviour and the decision are unchanged; only
the machine-local path prefix was redacted for publication.
