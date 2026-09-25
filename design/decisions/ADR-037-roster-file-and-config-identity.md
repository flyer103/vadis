# ADR-037 — the roster leaves the server's file: `providers` as its own configuration class (exactly one of two shapes, a named roster, a byte-digest identity)

- Status: accepted
- Date: 2026-09-25
- Related: AGENTS hard constraints **1** (the byte boundary), **2** (content determinism), **3** (the
  observation boundary), **4** (no unverified savings), **5** (no fabricated prices), **8** (docs before
  code) and **9** / ADR-012 (the measurement is not part of the search space); **ADR-025** (the write
  strategy this ADR may not contradict: the anchored edit on a verbatim template, and the
  one-file-not-a-layer ruling it recorded as Q23); **ADR-036** (the house shape this ADR follows, and
  D3's *an unconsumed primitive is the documented-but-unreachable defect*); ADR-002 (the declared
  `plugins:` list and its keyed diff), ADR-005 (the trace is the analysis truth), ADR-009 / ADR-010
  (the event log, and intent before effect), ADR-018 (one `currency` per amount — the type trick the
  digest is deliberately **not**), ADR-020 / ADR-021 (the citations and the banded tables whose
  provenance lives in the roster's comments); spec §4, §4.0, §4.1, §4.11, §4.12, **§4.14** (new), §6,
  §9.1; DESIGN §12.5, §12.6, §12.8 (**CONF-85**), §12.9 (**Q21**, **Q23**), §12.10.2, §12.10.5 row 13,
  §12.14; `config.example.yaml` (the `providers:` block); `tests/conformance/tests/conf_85_roster_file.rs`
  (the case that pins this ADR's assertions).
- Numbering note: the register holds **36** ADRs (`ADR-001` … `ADR-036`), so **037** is the next free
  number; DESIGN §12.8's occupancy paragraph names **`CONF-85`** as the next free conformance ID
  (`design/DESIGN.md:1376`).
- Scope note: **this ADR writes no code.** It freezes a contract whose first line of Rust lands in
  R43-2 (the split), R43-3 (the identity) and R43-4 (the wizard and the shipped example). At this commit
  the two-file shape is **specified, not served**, and every sentence below about the tree is a statement
  about a contract to land, not about a behaviour to observe.

## Background

**The measured problem is that two configuration classes share one file, and the class that shares it is
the one that changes.** `config.example.yaml` is **1162 lines**; its `providers:` block
(`config.example.yaml:100`) is **960 of them** — nine provider entries, **91** of the file's 92 `source:`
provenance citations (the 92nd is the header's own convention line, `config.example.yaml:15`), every
`api_key_env` name, every endpoint in `urls`, and every listed price. Server settings — address,
timeouts, session identity, trace parameters, plugins, aliases, fallback, `plan_policy` — are the other
202 lines. The owner's ask is a **structural** one: *server* and *provider* are two configuration
classes, so provider data can later be managed on its own (dynamic management, a control plane), while
the file a human hand-edits for the deployment stops carrying a thousand lines of roster.

**What already exists, and is reused rather than invented.** `RouterConfig::providers: Vec<ProviderCfg>`
(`crates/router-core/src/config.rs:1050`) is the single representation the serving path reads — seven
read sites inside `router-proxy` (`accounting.rs:67`, `availability.rs:109`, `forward.rs:1641`,
`forward.rs:1662`, `stream_forward.rs:574`, `stream_forward.rs:1221`, `stream_forward.rs:1256`), plus
`router-cli/src/lib.rs:185` and `:228` and `router-cli/src/setup/report.rs:54`. There is **no
multi-file config loading**: `config_load::load` (`crates/router-cli/src/config_load.rs:41`) reads
exactly one path, and `validate_text` (`:31`) parses one text. The exactly-one-of shape is the
repository's own idiom — a price block that carries both the flat shape and `tiers` is refused with the
message *"a price block is exactly one shape (spec 4.10 rule 1)"* (`config.rs:1191`) — and a known key
whose presence must produce a **precise** refusal rather than a generic unknown-field error already has
a type built for it (`StateKeyForbidden`, `config.rs:1026`). The hash convention is fixed: first 16 hex
of SHA-256, *"the one hash convention … and any future digest"* (`crates/router-core/src/prefix.rs:21`,
`body_sha16`). And the trace's vocabulary already designs a config identity: DESIGN §12.10.5 row 13
(`design/DESIGN.md:1876`) specifies `config.applied`'s payload as *"config digest, changed keys"*, while
the event as written today carries `{config_path, schema_version}`
(`crates/router-cli/src/lib.rs:168-176`) and `TRACE_SCHEMA_VERSION` is **2**
(`crates/router-core/src/trace.rs:28`).

**Two couplings, both found by grepping the tree rather than assuming it, fix the round's shape.** (i)
`crates/router-cli/src/setup/mod.rs:27` embeds `config.example.yaml` with `include_str!` as the wizard's
default template, and two live tests assert that the section table's anchors resolve **inside that
file** (`crates/router-cli/src/setup/sections.rs:353-367`, `crates/router-cli/src/setup/anchor.rs:929-956`);
so the shipped example cannot split in the parser card. (ii) `DecisionRecord` is constructed at **four**
sites inside `router-proxy` (`body_limit.rs:68`, `accounting.rs:487`, `auth.rs:133`, `forward.rs:124` →
the constructor at `:131`), so an additive trace field is not a `router-core`-only change.

**What this ADR is not.** It is not a configuration *layer*: there is still exactly one root file, at
most one roster, no precedence ladder and no key-by-key overlay. It is not a reload: R43's gate is *no
behaviour change* (D7). And it is not a price decision: no figure, no quota and no URL is decided,
quoted or changed here (AGENTS constraint 5).

## Decision

### D1. Exactly one of two shapes, per root file — the inline form stays a shape, not a legacy mode

A root config carries **either**

```yaml
providers:        # the roster inline (today's shape, unchanged)
  - name: …
```

**or**

```yaml
providers_file: ./providers.yaml      # the roster is its own file (spec §4.14)
```

*Both written* is a **refusal**; *neither written* is a **refusal** too. This is not a merge and not a
precedence rule: there is still exactly **one** place a given key can be written in any one effective
config, and the refusal is what makes that boundary checkable. The shape is enforced where the shape is
visible — the loader that reads the root (`config_load::load`) — not inside `RouterConfig::validate`,
which sees a joined config and can no longer tell the two shapes apart.

Consequences, stated because they are the reason this form was chosen over a hard cut: every existing
inline configuration stays **byte-identical**, so the 34 files under `tests/conformance/**` that carry a
top-level `providers:` block (measured at `c4ae04f`) and every recorded or maintained harness config —
including `autowork/harness/live-base.yaml:100`, the base the live-replay generator copies verbatim —
keep parsing with no edit at all. A pre-R43 rig re-run against a post-R43 binary therefore still loads;
what changes is only that a *new* config may name its roster.

### D2. The roster is named, never searched

`providers_file` is **required** when it is used, and its value is resolved by §4.1's existing in-file
rule, unchanged and reused rather than re-specified: an absolute value wins; otherwise the value is
joined to **the directory containing the config file**, never the CWD; the result is made lexically
absolute; `~` is **not** expanded, so `~/providers.yaml` is a relative path whose first component is a
literal `~`. That is the rule `trace.dir` and every `plugins[*].config.rules_file` already obey
(`config_load.rs:68`, `resolve`).

§4.12's discovery table gains **no row**. Its four candidates are ways of *finding the* config file; the
roster is not found, it is **named** by a key inside that file. A reference is a key, not a candidate: a
typo'd `providers_file` names itself in the refusal, whereas a missing implicit default
(`<config dir>/providers.yaml`) would name nothing — and a hidden default is, in this repository's own
words, *"the most expensive silent failure a hand-written config can have"* (`config.rs:1041-1042`).

### D3. The roster file is the same block, byte-moved

The roster file carries exactly **one top-level key, `providers:`**, holding the same `Vec<ProviderCfg>`
entries under the same `deny_unknown_fields` strictness. The move is `sed`-able: the bytes cut out of
`config.example.yaml:100-1059` are the bytes that land in the new file, comments and all, so the 91
citations travel with their entries and nothing is re-typed. One block type, one parser, one set of
per-entry rules.

A file whose top-level key is not `providers:` is **refused** — a `server:` block, a bare sequence, a
second top-level key, an empty file, or a document that fails the per-entry rules. The roster file is
**not** a config file: it has no `server`, no `plugins`, no `aliases`, and it is never discovered,
never defaulted and never merged.

### D4. The join is one-directional and happens once, at load

`RouterConfig::providers` stays **the** representation the serving path reads. The shape that lands in
`router-core` is: a root-file type carrying every section as today **plus** `providers: Option<Vec<ProviderCfg>>`
and `providers_file: Option<String>`; a roster-block type `{ providers: Vec<ProviderCfg> }`; and a
**pure** join producing today's `RouterConfig`, whose `providers` field is always populated. `config_load::load`
does the I/O — read the root, read the named roster when the root names one, join, then the **existing**
`validate()` — and `validate()` stays the one validator, running *after* the join so cross-file
references (`aliases`, `fallback`, `plan_policy.primary` / `.overflow`, `quota.models`) are checked in
one pass, in the existing message order, with the existing key paths.

What that buys, and what it forbids:

- **Nothing on the request path moves.** The seven `router-proxy` reads above keep reading
  `RouterConfig.providers`; there is no second accessor, no second resolution rule and no roster service
  key. A `ROSTER` service slot would be an unconsumed primitive — the documented-but-unreachable defect
  ADR-036 D3 names, and the exact shape this repository just finished paying for in P9. The seam is the
  **type**, and the type already exists.
- **`router-proxy` never learns a second file exists.** It never opens, reads, resolves or hashes a
  config file: the two absolute paths and the digests are resolved once, in `router-cli`, and travel
  exactly as `trace_dir` and `state_db` do today (DESIGN §12.10.2's `ResolvedConfig`). The only
  `router-proxy` edits this round makes are `health.rs` (a member carrying strings it is handed) and the
  four one-line, compile-driven additions to the `DecisionRecord` constructors listed in Background.
- **The join is not layering.** A join happens once, from two files whose keys cannot collide, into one
  document; a layer merges many files key by key with a precedence ladder. Nothing here has a
  precedence order to get wrong.

### D5. The refusal ladder — five shapes, each naming its key

**The keys and the shapes are the contract; the final wording of each message is the implementer's.**

| # | Shape | Refused by/at | What the message must name |
|---|---|---|---|
| 1 | **both** `providers:` and `providers_file:` written | the loader, before the join | both keys, and the root path |
| 2 | **neither** written (an empty roster is a decision, not a default; `providers` is not optional today and stays not optional) | the loader, before the join | both keys, and the root path |
| 3 | `providers_file` names a path that cannot be read — missing, unreadable, not UTF-8 | the loader, reading the roster | `providers_file`, the value **as written**, and its **resolved** path |
| 4 | the roster file does not parse as the roster block — its top-level key is not `providers:`, it carries a second top-level key, or an entry breaks an existing per-entry rule | the roster parse | the **roster's own resolved path** and the offending key/entry |
| 5 | a root key that references the roster does not resolve there: `aliases.*`, `fallback[i]`, `plan_policy.primary` / `.overflow`, `quota.models` | the existing `validate()`, after the join | the key path, the value found, **and the roster file** the reference failed to resolve in |

Shape 5 is the one that makes "every load error's provenance" true rather than aspirational: an error
about a roster key must say which file the roster is, because the reader of the message may be holding
only the root.

An **empty roster list** is legal (`providers: []` is written by several conformance fixtures today);
an **empty roster file** is not, because a document with no `providers:` key is shape 4.

### D6. Identity is a byte digest, and `schema_version` does not move

```
root_sha16    = sha16(bytes of the root config file)
roster_sha16  = sha16(bytes of the roster file); the EMPTY STRING when the roster is inline
config_digest = sha16(root_sha16 + ":" + roster_sha16)
```

`sha16` is the first 16 hex characters of SHA-256 — the repository's one hash convention
(`prefix.rs:21`). The recipe is recomputable by an outsider from the two files alone, in one shell line:

```sh
printf '%s:%s' <root_sha16> <roster_sha16> | shasum -a 256 | cut -c1-16
```

**Three surfaces, one value, no new meaning for any existing field:**

1. `DecisionRecord.config_digest` — an additive top-level trace field (spec §6; DESIGN §12.6), so a
   decision and its money are attributable to the revision that priced them.
2. `config.applied`'s payload — the digest lands beside the two **absolute** paths and the two file
   digests, next to today's `config_path` and `schema_version` (`crates/router-cli/src/lib.rs:168-176`).
   That is row 13's *designed* "config digest" half (`design/DESIGN.md:1876`); the "changed keys" half
   is R44's (D7), and the `schema_version` already in that payload is the **store's**, which no part of
   this round moves.
3. `/health`'s config member (spec §9.1) — both paths, both file digests and the composed digest, so
   "which configuration is this process serving?" is answerable from a surface rather than from argv.
   `setup --print` / `setup --check` print the same triple (spec §4.11; DESIGN §12.14).

**`TRACE_SCHEMA_VERSION` stays `2`** (`crates/router-core/src/trace.rs:28`). The version exists to warn
a consumer that a record would be **misread**; the v1→v2 move happened because `cost.currency` changes
how the money fields must be read (DESIGN §12.6). A digest changes the reading of no existing field, and
every consumer on the autowork side already tolerates an unknown key — which is precisely the
additive-field rule §12.6 states. A bump would instead create two vintages that mean the same thing,
which is what a version is not for. A reader must therefore treat a **missing** `config_digest` as "this
record predates the field", never as an empty digest.

**A byte digest, not a digest of parsed values.** A canonical form of the parsed config needs a
`Serialize` for every config type — a **second definition of the field set**, which ADR-025 refused for
exactly this reason — and it would normalize the document (quoting, order, indentation). A byte digest
needs neither, is recomputable from the files by hand, and is **stricter** in a way this repository
means: a comment edit moves it, and the comments are where the price citations live (ADR-018,
ADR-020). Two files with identical bytes have one identity, which is the point.

**The identity is attribution, not a score.** It may not become a gate input, and no round may cite a
cost delta "explained by the config revision" without the same-run baseline the gates already require
(constraints 4 and 9, ADR-012).

### D7. R43 lands the data the next round reloads; the reload is R44, with its decider named

No watcher, no SIGHUP, no atomic revision switch, no write path, no management API. R43's gate is
**no behaviour change**, and a watcher would falsify it by construction.

**What decides the rest:** the reload is **R44**, its own round with its own gate; its named
pre-requisites are the watcher decision (a file-watch crate — a change to the dependency allowlist of
DESIGN §12.1, and therefore a **human** decision — versus a std-only mtime poll), the atomic-swap
decision, and a fresh p99 ladder run. R44 also owns the "changed keys" half of row 13's payload. The
write path — *who may change providers* — is an **owner** question and not a round's: R43 adds a file
the operator writes, nothing more (AGENTS constraint 3: the loop never holds that path).

### D8. The shipped example and the wizard's template split in R43-4, with the couplings that force it

The shipped `config.example.yaml` **does not move** in the contract card and not in the parser card.
`setup/mod.rs:27` embeds it as the wizard's own default template, and two live tests assert the section
table's anchors resolve inside it (`setup/sections.rs:353-367`, `setup/anchor.rs:929-956`); splitting it
in the parser card would break two tests in a card forbidden to touch the wizard. **R43-4** therefore
lands, together: the split example (the roster block leaves the root **byte-moved**), a new
`providers.example.yaml` — **one roster, one shipped copy**, because §4.0's no-two-copies rule forbids
shipping the same roster twice — the second embedded template, and the section table's *target file*
column.

Exactly-one-of is what makes this sequencing safe: the inline example stays **legal**, so the shipped
example parses at **every commit of this round**, and §4's rule ("the shipped example is always
loadable") holds at every step. The split path is exercised from R43-2 on by `CONF-85` and by the
round's own byte-identity arm, not by the example. `providers.example.yaml` **does not exist in the
tree at this commit**; it lands with R43-4.

`autowork/harness/live-base.yaml` **stays inline** for the same species of reason: it is copied verbatim
into every generated arm config (`r39-3/run.toml:49`), and teaching the generator about a second file is
a harness change no card of this round needs — the round's arms build their own pair instead.

### D9. What Q23's four consequences get here, and what stays unlanded

DESIGN §12.9's **Q23** row (`design/DESIGN.md:1432`) puts a *layered* configuration behind its own ADR
**because** it would change four things. R43 merges nothing — one root, at most one roster, no
precedence ladder, no key-by-key overlay, no second discovery candidate — but it gives those four their
**first minimal answer**:

| Q23's consequence | R43's minimal answer | What is still **not** answered |
|---|---|---|
| **which file a key came from** | the exactly-one-of rule *is* the answer: a key is written in exactly one place, and the place is a property of the key, not of a search order | per-key provenance for a *merged* config; there is no merge to provenance |
| **every load error's provenance** | D5: root errors name the root path; roster errors name the roster's own resolved path; a cross-file reference error names the roster as well as the key | a precedence-aware "this key lost to a higher layer" message — no layer exists to lose to |
| **`/health`'s "what was loaded"** | the config member carries **both paths, both file digests and the composed digest** (spec §9.1) | a member for a *set* of files, or for a managed source |
| **`setup`'s write strategy** | ADR-025's strategy exactly as it stands — verbatim base plus anchored single-line edits — applied to **the file that owns the key**: the `providers` section edits the roster when the root uses the split form and the root otherwise (spec §4.11's target-file column; DESIGN §12.14). `--from <roster> --force` **replaces the roster as a unit**, under the same strategy, touching no other byte | a merge has no single base to edit, so nothing here anticipates one |

**Q23 stays open, and its row must now say exactly that.** The layering has not landed; what landed is a
one-root/one-roster shape whose four consequences are answered *minimally*.

**Q21 is not closed either** (`design/DESIGN.md:1430`). Replacing a roster as a **unit** is **not** an
insertion: no anchor is created, no position is chosen and no block style is reproduced — a whole file is
handed to the command. Q21's trigger stands unchanged: a round that wants a **wizard-created entry**
must freeze the insert rule first (position, indentation, the block's own style). **What decides:** the
owner's ruling on that insert rule, taken by the round that wants the capability — not by R43, whose own
card is forbidden to invent it.

## Alternatives considered

- **The plain hard cut — provider data may live only in its own file, and the inline form is removed.**
  Rejected: it spends the fixtures. Every inline configuration would have to be split into a pair before
  the suite could run, in the same round that moves the product type — the 34 conformance files and the
  maintained harness config included — and it converts a shape into a legacy mode with no gain: the
  owner's ask is that there be **one** place a key can be written, and exactly-one-of enforces exactly
  that with one new refusal instead of 71 edits.
- **A merge — the root plus the roster merged key by key (later: global + project + managed).** Rejected:
  a merge has **no single base** for an anchored edit, and that single base is the reason `setup` can be
  trusted at all (ADR-025 decision 1); it makes "which file said that?" the first question of every load
  error; and it is Q23's own registered candidate, which the register already commits to a round with its
  own ADR. One round cannot land half of it.
- **Discovery of the roster — `<config dir>/providers.yaml` as a fifth candidate of §4.12.** Rejected: the
  four candidates are ways of finding *the* config file; a hidden default is the most expensive silent
  failure a hand-written config can have (`config.rs:1041-1042`). A reference names itself in the refusal;
  a default names nothing.
- **A `providers.d/` directory, one file per provider entry.** Rejected: the identity becomes a digest
  over a *sorted set*, every error becomes a "which file?" question, and it is a discovery rule §4.12 does
  not have. It becomes the natural shape only once a manager — not a hand — writes the files, which is
  later than R44.
- **A digest of the parsed config (a canonical form, i.e. a `Serialize` for the config types).**
  Rejected: a second definition of the field set (ADR-025's own reason for refusing `Serialize`), it
  normalizes the document, and it would not move on a comment edit — where the comments carry the price
  citations.
- **Bumping `TRACE_SCHEMA_VERSION` to 3.** Rejected: the version warns a consumer that a record would be
  misread; a digest changes the reading of nothing, and the same-semantics-two-vintages outcome is worse
  than the additive field §12.6 already permits. (A bump would be an owner decision in any case, never a
  card's.)
- **Landing the reload in R43 (a watcher over the two files).** Rejected: R43's gate is no behaviour
  change and a watcher falsifies it by construction; the watcher's dependency question is an allowlist
  change and therefore a human decision, and the atomic swap is a second contract. D7 names R44.
- **A `ROSTER` service key, or a plugin-mediated roster.** Rejected: an unconsumed primitive is the
  documented-but-unreachable defect (ADR-036 D3) this repository just finished paying for. The type
  `RouterConfig.providers` is already the single representation, so no service slot is needed.
- **A compatibility layer, a deprecation window, or dual-read.** Rejected: the inline form is a shape, not
  a legacy mode; a window would be a second read path with no owner and no end date, and it would leave
  "which shape is documented?" open for a round that does not need the question.
- **A wizard-side *insertion* of a new provider entry, so the roster is manageable entirely from
  `setup`.** Rejected for this round and registered (Q21, D9): an insertion needs a position rule, an
  indentation rule and the block's own style reproduced, inside a code path whose failure mode is a
  corrupted price table. The unit replacement (`--from <roster> --force`) is the capability genuinely
  reachable today, and it is what D9 lands.

## Rationale

- **The class boundary is enforced where the shape is visible.** A root file that has not been joined
  with its roster cannot be mistaken for a complete config: the loader refuses before the join, and the
  join is the only place the two shapes differ. One function decides the shape; if a second site learns
  about it (a plugin, a health member reporting "inline", a stats subcommand), the abstraction has
  failed.
- **A named reference is checkable; a default is not.** The refusal ladder is the whole boundary: five
  shapes, each naming a key, all of them decidable without reading a byte of the roster's contents.
- **An identity you can recompute by hand is an identity an operator can use.** The digest answers "did
  the config change between these two windows?" from the files alone, without parsing them — and it
  answers it the way this repository means it, comments included.
- **R43 is the data round, not the behaviour round.** Everything it lands is either a shape, a name or a
  value: no request-path change, no reload, no write path. That is what keeps its gate (*no behaviour
  change*) honest and its byte-identity arm meaningful.

## Consequences

- **Code and docs (the round's shape).** `router-core` config types and the trace field; the loader
  (`config_load.rs`); the four `DecisionRecord` constructors in `router-proxy` plus the two fixtures
  (`router-core/src/trace.rs:402`, `router-store/src/trace_sink.rs:162`); `config.applied`'s payload
  (`router-cli/src/lib.rs:168-176`); `router-proxy/src/health.rs`; `router-cli/src/setup/**` and the two
  example files (R43-4); `tests/conformance/tests/conf_85_roster_file.rs` (new, owner-allocated — one ID
  carrying **both** halves: the refusal ladder **and** the identity); spec §4, §4.11, §4.12, §4.14, §6,
  §9.1; DESIGN §12.5, §12.6, §12.8, §12.9, §12.14; this ADR; `book/getting-started.md`,
  `book/operations.md`, `book/cost-and-caching.md`; `README.md`.
- **Nothing on the wire moves, and that is measured, not asserted.** The join happens at load and the
  serving path reads the same `RouterConfig.providers` it reads today, so the round's byte-identity arm
  is: the shipped example, split into a root and a roster, served against a recording mock upstream,
  produces request bytes **identical** to the same example served inline — with the base arm built in a
  throwaway worktree of the base commit, never by reverting inside the round's tree.
- **No per-request accessor is introduced.** The digest is computed once, at load, and carried as an
  immutable value that the trace writer stamps; nothing behind a request opens, reads or hashes a file.
  If the diff adds or moves a per-request read of provider data, the no-behaviour claim is
  self-refuting, and the round must say so rather than run a latency ladder to find out.
- **What this round may not claim:** any saving figure or latency figure (nothing in the request path
  changes); that provider data is **dynamically managed** (it is not — the split is the *reason* it can
  be, later); that Q23's layered configuration landed; that the wizard can create an entry; or that a
  config change takes effect without a restart.
- **The measurement apparatus is untouched** (AGENTS constraint 9 / ADR-012): no gate definition, no
  threshold, no corpus, no L1 envelope, and **no existing conformance assertion**. `CONF-85` is a new
  file over a surface that did not exist, and the occupancy statement in §12.8 records that.
- **The register rows are dispositioned rather than left to drift.** DESIGN §12.9's **Q23** row says the
  layering has not landed *and* that its four consequences have their first minimal answer; **Q21** says
  a unit replacement is not an insertion and keeps its trigger; §12.8's occupancy paragraph spends
  `CONF-85`.

## Honest boundaries

- **Nothing here is served at the time of writing.** `providers_file` is a contract; the tree's shipped
  example is still inline and splits in R43-4, and `providers.example.yaml` does not exist yet.
- **The digest identifies the files, not the behaviour.** Identical bytes are one identity (that is the
  point); a one-character comment difference is two. A reader must use it as "which revision of the
  inputs", never as "which behaviour".
- **The refusal ladder freezes shapes and keys only.** Which shape is refused by which key is the
  contract; the wording of each message belongs to the card that writes it.
- **The join's typing is the implementer's choice within D4's constraints.** Whether the root file's
  sections are carried by a dedicated root type or by a mutation of `RouterConfig` after parsing is a
  code shape, not a contract; what D4 fixes is that `RouterConfig::providers` remains the only
  representation the serving path reads and that exactly one function decides the shape.
- **No price, quota, URL or cost figure is decided, quoted or changed by this ADR**, and none may be
  derived from it.

## Reversibility

Reversible, and cheaply in the direction that matters. Removing the key and the join returns the tree to
one file; because every inline configuration stayed byte-identical under D1, the cost of the revert is
an edit to the files that *used* the split — the round's own fixtures, and after R43-4 the shipped
example — and nothing else. **No stored data has to move:** `config_digest` is a field on records already
written, an older record simply lacks it, and `TRACE_SCHEMA_VERSION` staying `2` is what keeps that true
in both directions. Toward layering, the shape is deliberately *not* reusable: a join is not a precedence
ladder, so if Q23 ever lands, the roster *file* survives and the join is replaced — the seam is the type,
not the file. The one thing a later round must revisit rather than inherit is the recipe itself: a second
roster file would change what `config_digest` hashes, and that is a decision for the round that wants it.
