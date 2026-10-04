# ADR-025 — `vadis setup` edits the config by anchored text edits on a verbatim template, never by re-serializing it

- Status: accepted
- Date: 2026-09-22
- Related: AGENTS hard constraints 5 (the price/provenance rule this decision exists to protect), 2 (the written
  file must be a function of content, never of an instant), 1 (nothing here is on the request path), 7 (English
  only), 8 (the book precedes the implementation), 9 / ADR-012 (no gate, corpus or existing assertion moves);
  ADR-005 (the trace is the analysis truth — this decision is about the *other* file), ADR-018 / ADR-020 /
  ADR-021 (the `source:` citations and the banded price tables whose provenance lives in this file's comments),
  ADR-019 I3 (a transform is a request fact, never a config side effect of a wizard); spec §4, §4.0, §4.1
  (the in-file path rule this ADR declines to change), §4.5 (the store's fixed path, ADR-009 item 6), §4.7,
  §4.10, **§4.11** and **§4.12** (the contracts this ADR justifies: the writer, and where the file it writes is
  found); DESIGN §12.1, §12.5, §12.8 (CONF-67…70 and **CONF-79**), §12.9 (the new **GAP-Q21**, **Q22**, **Q23**),
  §12.10.2, §12.11, **§12.14** (the landing); the round's survey
  the survey record (2026-09-22) — hermes-agent / opencode / codex / `docker init`,
  measured locally, every claim sourced).

*(The number R21 declined to spend. §12.8's R21 close-out note records that R21 needed no ADR and therefore
wrote none — so `ADR-025` was free, and this round takes it.)*

## Background

The user's direction (2026-09-22) is a usability one: add a `setup` command that guides a user through the
config blocks, with **most items taking a default**. A second direction (the same day, while this round was
being written) fixes where the result should land: a config produced this way belongs in the platform's own
config directory — `~/.config/vadis/` — not in the directory the operator happens to be standing in. That one
default drags three contract questions behind it, which decisions 8–10 answer: how the gateway finds the file
that was written (spec §4.12), what the file's own relative paths then resolve against (§4.1, unchanged, with its
consequence registered as Q22), and whether a configuration may be assembled from several locations (it may not:
Q23). The survey then established the constraint that decides this ADR's shape: `config.example.yaml` is 1,162 lines whose **comments** carry the price authority —
92 `source:` citations (official page URL + read date) and 17 `TODO verify against official source` markers —
and AGENTS constraint 5 makes that provenance non-optional: prices and quotas enter the file only after reading
the provider's official page, and the citation travels **on the line the figure sits on**.

Two facts about the toolchain turn that into a design constraint, not a detail:

- `serde_yaml` does **not** preserve comments. Deserializing `config.example.yaml` into `VadisConfig` and
  serializing it back yields a valid, complete, *comment-free* document: every `source:` URL, every read date
  and every "what it was before" note is gone. There is no `VadisConfig → YAML` path in this repository today,
  but `serde_yaml` could provide one, and its output is what "just modify the struct and write it back" means.
- The parser has `deny_unknown_fields` **18 times** (plus `vadis-plugins`' rule table), and §12.5's defaults
  row says only the three defaults spec §4 states explicitly are defaults: "everything else is not enabled
  unless written". There is no in-code default set (hermes-agent's `DEFAULT_CONFIG`) to fall back on, and
  nothing corresponding to its `_config_version`.

So the question this ADR settles is not "how do we write a YAML file" but: **what is a config file allowed to
lose when a command writes it, and what is the command's reach?** The survey's own anti-pattern list (its §4)
confirms the direction from the other side: hermes-agent's `config set` rewrites the file through a
serializer, and opencode's managed layer is a full-JSON-merge worldview; both would zero this repository's
provenance. `docker init` shows the opposite failure: a hard TTY error with no path forward, over prompts that
have no defaults at all.

## Decision

**1. The default write path is a verbatim base plus anchored, single-line edits — and only two edit kinds.**
The base is the target's own bytes when the file exists, and the template's when it does not (or under
`--force`). Each answer that differs from the value at its anchor becomes one edit:

- `set-value` — replace the value's **byte extent on a single line** whose key anchor resolves **uniquely**;
- `set-enabled` — add or remove the leading comment marker on the key's **own line** (the template ships
  `server.auth_token_env` and `plan_policy.overflow_monthly_cap_usd` commented out).

A plan is a list of `(line, byte range, replacement)` triples, sorted by offset and asserted disjoint, spliced
into a **copy** of the base. The replacement is encoded in the file's **own style** at that key (quoting,
bare booleans and numbers, the §12.5 duration grammar): the command changes a value, never a style. Hand-written
keys, ordering and every comment in the file survive verbatim, because nothing is regenerated.

**2. An edit that cannot be made exactly is a refusal, not a best effort.** An anchor that resolves to no line,
or to more than one; a value that is not a single-line scalar (a flow mapping, a block scalar, a multi-line
string); overlapping ranges; or a candidate that fails the loader — each **writes nothing**, names the key and
the reason, and exits 2. An anchor that does not resolve with **no** requested change to that key is only a
warning. The locator's accepted YAML subset is *block style, one scalar per line*, and that subset is stated
rather than inferred: a near-miss here is a corrupted price table, and the strategy's whole value is that the
bytes which are not the answer are not touched.

**3. No re-serialization, at any verbosity, behind any flag — and the escape hatch is a template, not a
serializer.** The way to *replace* a file wholesale is `--from <path> --force`: the operator hands the command
a **file** they authored and can read. "Regenerate the file from the parsed struct" is not offered as a mode,
an option or a recovery path, because its only user-visible effect is the loss of the provenance the file
exists to carry.

**4. The default a question shows is the file's own value, else the template's — never a constant in code.**
There is no in-code default set, so there is no `--reset` (the shipped example *is* the default set, and
`--force` is therefore both "reset" and "start from another template") and no `_config_version` (there is no
key the wizard owns: `deny_unknown_fields` would make one an unservable file). A price, a `source`, a
`context`, an endpoint or a model id is **displayed and never prompted** — the wizard does not invent a vendor
fact and does not ask a user to recall one. "Only ask what is missing" (`--quick`) therefore means the *named
environment variables* that are absent, which is the only thing that can be missing at a site once the example
is the baseline.

**5. Names only, and no `.env`.** The two keys the command may write carry **variable names**
(`providers[*].api_key_env`, `server.auth_token_env`). It never reads a key value (presence is probed as a set
of names, `var_os(name).is_some()`), never prints one, never writes one, and prints an `export` snippet for each
missing name. It does not create a `.env`: nothing in this product reads one (§4.7 reads the process
environment), so a `.env` it wrote would be a file whose presence does not make a key present.

**6. No terminal is a refusal with the working command in hand.** With stdin not a terminal and
`--non-interactive` absent, the command prompts nothing, prints the exact non-interactive command line plus the
export snippets, and exits **2**. It writes no file: a run that did not do the work is not a success to a
script (`docker init`'s hard error is right about the exit code and wrong about the guidance; hermes-agent's
`rc=0` with nothing written is right about the guidance and wrong about the exit code).

**7. The wizard's reach is bounded by the template's own shape.** Value replacement on an existing line only:
no insertion, no deletion, no reordering, no reformatting. A new provider entry, a new alias, a new `fallback`
entry and a new plugin entry are hand edits, and the section that owns the block **says so**. Registered as
**GAP-Q21**, with the trigger for revisiting it (a round that wants a wizard-created entry must freeze the
insert rule first — position, indentation, and the block's own style).

**8. One file, found by a written order — never merged, and the default site is the XDG location.** `setup`'s
default output is `${XDG_CONFIG_HOME:-$HOME/.config}/vadis/config.yaml`, and `serve` / `stats` resolve the file
they read by the **same** order (explicit `--config` > that location > `./config.yaml` > refuse; the writer alone
falls back to creating the XDG location) — spec §4.12. Two decisions are packed here. **(a) The default is
outside the repository**, so a config cannot be committed or `git clean`-ed by accident, it survives a fresh
clone, and it is the convention the user's other tools already agree on; `--config` keeps the name `serve` and
`stats` already use (the `--out` a card proposed is rejected: the path is the *same file* those commands read,
and two names for one path is how a CLI begins to contradict itself). **(b) There is no layered configuration** —
no global/project/admin merge, no remote or managed file: exactly one file is read, and the candidates are ways
of *finding* it, not sources that combine. The order is one rule in one place (`config_path::resolve`, called
from `main`), and the serving path gains no default *value* from it: the listen address, the plugin set and the
roster still come only from the file (CONF-25). The selection is printed — absolute, with the rule that chose it
— so "which file did that go to?" is never a question.

**9. What the command creates, and with what permissions.** The target's directory is created when it is missing
(`mkdir -p`), because the default location does not exist on a fresh machine. A file this command creates is mode
`0600` and a directory it creates is `0700` — set explicitly rather than left to the umask, and never applied to
a directory that already exists. Justification: no key **value** is ever written to the file, but it names the
operator's key variables and their whole roster, which is private data with a zero-cost restriction; and the
absolute path printed at the end is what makes a mistyped `--config` visible instead of silent.

**10. The paths inside the file keep their anchor.** §4.1's rule — a relative path resolves against the
**config file's own directory**, never the CWD, with an absolute value winning — is **not** changed, so a config
at the XDG location keeps its traces and its store beside itself under `~/.config/vadis/`. The alternative
(config under `~/.config`, state under `$XDG_STATE_HOME` / `$XDG_DATA_HOME`) is rejected here rather than
deferred, because it needs a **second** anchor: the store's path is not a config key in v0.1 (fixed at
`<config dir>/state/vadis.db`, spec §4.5, ADR-009 item 6), so the split cannot be expressed in the file at all
and would become a rule that depends on where the config happens to sit — the drift DESIGN §12.10.2's "one rule,
in one place" exists to prevent. It would also move where every existing installation reads and writes, which is
a migration, not a default. The consequence is registered as **GAP-Q22** (the additive `state:` key §12.5
anticipates is the way to move the store), and the answer available today for a trace directory elsewhere is an
**absolute** `trace.dir` — already supported, no new contract, and `~` is not expanded.

## Alternatives considered

- **Structured rewrite: `load` → mutate the struct → serialize (the "modern" approach).** Rejected, and this
  is the ADR's main trade-off. It is 30 lines of code and it is correct as YAML — and it destroys the file's
  provenance on **every** run, including a run that changes one key: 92 `source:` citations, 17 TODO markers
  and every recorded deviation vanish, which is precisely what AGENTS constraint 5, ADR-018 and ADR-020 spend
  their text keeping. It also needs a schema-complete `Serialize` for every config type (a **second**
  definition of the field set, which can drift from the parser's, and which must be kept in step with
  `deny_unknown_fields` by hand), and it normalizes the document — quoting, ordering, indentation — so any diff
  of a run is the whole file and loses the "one logical change per commit" property that makes a config change
  reviewable. **The cost we accept instead:** a hand-written locator with an explicitly bounded YAML subset,
  and no guided insertion (Q21).
- **A patch format / a YAML DOM with a patch library.** Rejected: a DOM is the previous alternative's problem
  (comments die at parse, before any patching), and both a patch library and a DOM crate are outside §12.1's
  allowlist.
- **A comment-preserving DOM of the `toml_edit` kind.** Rejected: it is the right idea in the wrong language.
  The config is YAML (spec §4) and the Rust YAML ecosystem has no comment-preserving editor; writing one is
  *this scanner plus a complete YAML grammar*, i.e. strictly larger than the decision above and with a much
  bigger failure surface.
- **`--from` written by the wizard, i.e. "generate a complete file from answers" as a second mode.** Rejected:
  it is the structured rewrite with a different name — the same information is missing (prices, `source`,
  endpoints) so the generated file would be either incomplete or invented, and "invented" is constraint 5's
  one absolute prohibition.
- **Write an `.env` (or a `secrets.yaml`) next to the config.** Rejected: it creates a second secret location
  that nothing reads, so a user who filled it would still get "provider unavailable" — a silent failure with a
  file as its alibi. It also walks into the class §4.7/§12.11 exists to forbid: a value in a file.
- **Environment-variable overrides for the answers (`VADIS_SETUP_ADDR=…`).** Rejected: the written bytes
  would become a function of the shell that ran the command (above a file that is supposed to be the single
  source of truth), and there is no env-shaped *value* here to override — the env-shaped facts are variable
  **names**. The CI story is `--non-interactive` plus `--from` / `--config`.
- **hermes-agent's `--reset` + `_config_version` + recursive "fill the missing keys" model.** Rejected: the
  whole mechanism rests on an in-code `DEFAULT_CONFIG` that does not exist here and should not — it is a
  second source of truth for the file and the obvious place for a price constant to appear (constraint 5).
  Under `deny_unknown_fields` plus a complete example there are no missing *keys* to fill.
- **Silently accept defaults when there is no TTY (`--yes` semantics by inference).** Rejected: the write is a
  file write the user asked for interactively; materializing a config in a script's CWD is exactly the
  "where did this file come from / my edit did not take effect" failure class §12.5 calls the most expensive
  silent failure. `--non-interactive` is the explicit form of that intent.
- **Networked onboarding: fetch `/v1/models`, or re-read the cited pricing page and refresh the date.**
  Rejected: it makes the command's output depend on an instant (constraint 2) and turns "check the price"
  into the vadis's guesswork (constraint 5). The survey's anti-pattern 2 records the same conclusion from
  hermes-agent's `--portal` / `model --refresh` and codex's browser login.
- **Delegate to a prompt/TUI crate (`dialoguer`, `inquire`) for menus, masked input and arrows.** Rejected for
  now: it is a dependency-allowlist change (§12.1) bought for menus; the questions are lines with defaults,
  and the one place a masked input would be wanted (a key value) is a place this design refuses to go. A PTY
  *test* dependency is the same species and is declined the same way (a script drives the interactive path).
- **Keeping `--config` required on `serve` and `stats` (today's `vadis serve --config config.yaml`).** Rejected,
  and it was the genuinely close call of this round: it is safe — no discovery rule, no new behaviour on the
  serving path — and it leaves the new default write site pointless, because a user asked to type a
  forty-character XDG path at every start is exactly the friction the default exists to remove. Its "half"
  version (require the flag, print a hint) is a rule the *user* must obey rather than one the tool knows, and it
  fails the same way a copied path always fails: silently, by reading a different file than the one that was
  written. The order is written **once** (spec §4.12) in the CLI argument layer, the entry points and their
  messages do not move, and the cost is one resolver plus one case (CONF-79).
- **A layered configuration — opencode's eight layers, codex's project / `--profile` / managed files.** Rejected
  for now and registered as **GAP-Q23**, not as a defect: a merge has **no single base** for an anchored edit
  (decision 1 is the reason this command can be trusted at all), it makes "which file said that?" the first
  question of every load error, and it would change what "the file is the single source of truth" means in four
  places at once — spec §4's usage note, §12.5's `deny_unknown_fields`, `/health`'s "what was loaded", and this
  command's write strategy. One round cannot land half of it, so it waits for its own ADR.
- **`--out <path>` on the writer, `--config` on the readers.** Rejected: one file, one name. The flag that names
  a path should not change with the verb, and the difference between "the file I write" and "the file the
  gateway reads" is precisely the difference this round exists to make invisible.
- **XDG state separation: config under `~/.config`, traces and store under `$XDG_STATE_HOME` / `$XDG_DATA_HOME`.**
  Rejected as a **second** anchor (decision 10): the store's path is fixed and not a config key in v0.1, so the
  split is inexpressible in the file and would become a rule about where the config happens to sit. The cost of
  refusing it is visible and small — a dotfile-managed `~/.config` would carry a WAL database and hourly traces
  — and it is registered as **GAP-Q22** with the additive `state:` key as its way out.

## Rationale

- **A config file is a record, and the cheapest way to keep a record's bytes is to not reproduce them.** Every
  alternative that regenerates the file spends the provenance, and the provenance is what makes a price
  checkable by a human with a browser. The strategy's cost (a bounded locator) is paid once in code; the
  alternative's cost (an uncheckable price table) is paid by every reader of every report.
- **An edit that cannot be exact must not happen.** The two-line summary of this ADR is "the bytes that are not
  the answer are not touched — or nothing is written". A wizard that "does its best" on a hand-edited file is a
  wizard that can silently mis-price traffic, which is the failure mode the whole `deny_unknown_fields` stance
  exists to make impossible (§12.5's reason line).
- **The defaults must come from the artifact, not from the code.** Sourcing every question's default from the
  file (else the template) keeps one source of truth for the field set and removes the place where a copied
  price or URL would otherwise appear — the same argument as spec §4.0's "this file copies no price figure".
- **A wizard may reach the environment's *names* and nothing else.** §4.7/§12.11 already froze that the token's
  value never enters a struct, a log line, a trace field or an event payload; a guided command is a natural
  place for that rule to be quietly broken, so the rule is restated as the command's own contract — with a
  canary assertion (CONF-70) rather than a promise.
- **Where two comparable tools disagree, take the half each got right.** hermes-agent's guidance-not-guessing
  branch and `docker init`'s non-zero exit are each half of the no-TTY answer; the section granularity and
  "show the current value, keep it on Enter" are hermes-agent's, and its `--reconfigure` (a flag that came to
  mean the default) is the lesson for **not** adding flags that restate behaviour.
- **A default location is a usability decision with a contract tail, and the tail is the interesting part.**
  Choosing `~/.config/vadis/` forces three answers a "just write `./config.yaml`" default never has to give:
  how `serve` finds what `setup` wrote (spec §4.12's one order), what the file's relative paths now mean (§4.1's
  anchor, unchanged, with its consequence registered as Q22), and why the file is not merged from several
  locations (Q23). Answering them in the round that introduces the location is far cheaper than discovering them
  the first time a user's gateway reads a different file than their wizard wrote — and it keeps the platform's
  own convention (`~/.config`) as the thing a new user sees first.

## Consequences

- **Code.** `crates/vadis-cli/src/setup/` (the writer: sections table, line locator, edits, prompt channel,
  report paths — DESIGN §12.14) plus one mechanical extraction in `config_load`: the parser call and
  `validate()` become a shared entry point so the `serve` startup and `setup` cannot drift. No new crate or
  dependency (stdin, stdout and `std::io::IsTerminal` are std); `vadis-cli`'s allowlisted set is unchanged.
  Nothing on the request path is touched: no store event kind, no projection, no trace field,
  `TRACE_SCHEMA_VERSION` stays **2**, and no config key is added (`deny_unknown_fields` would refuse a
  wizard-only key).
- **Client-visible.** One new subcommand and its exit codes (`0` wrote / no change / printed; `2` refused,
  nothing written; `4` a named variable is missing — the code `serve` refuses the start on; `1` I/O), plus
  `--print` / `--check` / `--dry-run` as read-only surfaces. **`serve` and `stats` gain one thing and lose one.**
  They gain the discovery order of decision 8 (spec §4.12): `--config` becomes optional, and "no argument" means
  the XDG location, then `./config.yaml`, then a refusal that names the setup command — so the flag they already
  had keeps working and their messages keep their shape; they lose nothing, because absence now resolves to a
  path rather than erroring in the parser. That resolution is the **only** serving-path behaviour this round
  changes, it lives in the argument layer (`main` resolves once and passes an absolute path), and the entry
  points `vadis_cli::serve(&str)` / `stats::stats(&str, …)` are untouched — which is why the existing rigs
  (CONF-23, CONF-25, CONF-43) cannot observe it.
- **The file's contract.** `vadis setup` will not write a file it cannot first load with the same loader
  `serve` uses, and the produced file differs from its base only inside the lines it was answered about — the
  two properties CONF-67 and CONF-69 pin, with the refusal ladder (CONF-68), the canary (CONF-70) and the
  location rule (CONF-79).
- **Docs.** spec §4.11 (the writer's contract) and §4.12 (the location rule, with §4.1's in-file rule restated
  where a user meets it), DESIGN §12.14 (the landing), §12.1 (the crate row), §12.8 (the allocation of
  CONF-67…70 and CONF-79), §12.9 (**GAP-Q21**, **Q22**, **Q23**), §12.10.2 (the second writer),
  `book/getting-started.md` (the user-facing walkthrough — the default location, the permissions, and where the
  file's own paths land — written ahead of the command per AGENTS constraint 8), and `book/operations.md` (a new
  "Where the config comes from" subsection, so an operator running `serve` meets the discovery order too). **The
  book is written to keep CONF-43 green while the command does not exist**: every `vadis setup` mention sits in
  a paragraph carrying one of that case's five deferral markers, verified at this round's tree. The implementing
  round owes the other half — `README.md`'s CLI block gains the command (CONF-43's direction 2 requires every
  served subcommand to be mentioned in the docs) and the deferral markers are retired.
- **The measurement apparatus is untouched** (AGENTS 9 / ADR-012): no gate definition, no corpus, no L1
  envelope and no existing conformance assertion. The five IDs (CONF-67…70, CONF-79) are new files over a
  surface that did not exist; CONF-25's row and its loader test are unchanged; and CONF-43's docs↔CLI relation is
  **kept** rather than edited — the round's own chapter is marker-honest, and that case was run at this round's
  tree (1 passed).

## Honest boundaries

- **Nothing here is implemented at the time of writing.** `vadis setup` is not served; §4.11 and §4.12 are
  contracts, and both `book/` chapters say so in their own text where a user would otherwise look for the
  command. The G1–G8 properties are what the implementing round must assert, not measurements this ADR reports.
- **The location rule is the one serving-path behaviour this round changes, and it is deliberately the smallest
  version of it**: absence **resolves**, it does not default a *value*; the resolution is a pure function of
  `--config`, `$XDG_CONFIG_HOME` / `$HOME` and the CWD; the rule sits in the argument layer; and the entry points
  keep the signatures their rigs drive. Its one CWD-dependent step — the `./config.yaml` candidate — is exactly
  why the reported selection carries the rule's name: from a different directory, a run with no `--config` and no
  XDG file writes to the XDG location instead, and the print is what makes that visible rather than surprising.
  CONF-79 is the case that will measure all of it.
- **The default location puts state under `~/.config/vadis/` too** (Q22): the traces the file names and the
  fixed `state/vadis.db`. That is the price of keeping §4.1's single anchor; it is stated in the book, and the
  operator has a supported answer today (an absolute `trace.dir`). Moving the *store* is the additive `state:`
  key's job, not a second resolution rule.
- **The byte-identity claim (G1) is against the binary's own embedded template.** A stale binary carries a
  stale example; a rebuild is what re-syncs them, and `config_load`'s shipped-example test is the existing
  guard that the example parses with the parser of its own commit. The embedded template's cost is ~85 KB of
  binary.
- **The locator is not a YAML parser, and its subset is narrow on purpose.** A hand-rewritten file using flow
  style, block scalars or multi-line values refuses the keys it cannot delimit (and warns when nothing was
  asked of them). This is a deliberate limitation, not a gap to be closed by teaching the locator more grammar
  — that is alternative 3 above.
- **The interactive path's assertions are script-driven (a PTY), not Rust unit tests**, because a PTY test
  dependency is an allowlist change and therefore a human decision. What a PTY script cannot see is a
  terminal-specific rendering detail; the decisions it *can* see (which answer reaches which line) are the ones
  that matter.
- **No price, quota, URL or cost figure is decided, quoted or changed by this ADR**, and none may be derived
  from it: it settles how a file is written, never what a figure is (AGENTS constraint 4).

## Reversibility

Reversible in both directions, cheaply. **Toward a structured rewrite:** the change is contained to
`setup/` — the locator module is deleted, `VadisConfig` gains a `Serialize` (with the field-set drift that
implies), and an existing file is rewritten once; what is *not* recoverable is the comments of a file a user
already rewrote, so the recovery is `--backup`/`.bak` plus git. **Away from the command entirely:** `setup` has
no request-path footprint, no store row and no config key, so removing it is deleting one subcommand and its
module. **Within the decision:** the edit kinds, the anchor grammar, the exit codes and the section table are
all stated in spec §4.11 / DESIGN §12.14, so a later round can widen the reach (an insert rule, Q21) without
reopening the trade-off this ADR settles. **The location rule, separately, is the most reversible part of the
round and the only piece with a client-visible default:** reverting is deleting `config_path::resolve` and
restoring `--config`'s `required = true`, after which the serving path refuses an absent flag as it did before —
and no stored data moves, because the rule never wrote anything (only the *writer* creates, and only the file the
operator named). The asymmetry to keep in mind: a config a user already put in `~/.config/vadis/` does not move
if the default is reverted, so such a revert is a flag's strictness, never a migration.

## Publication note (2026-10-03, R62-2)

The `R<n>` labels and finding ids in this document name iterations of the project's own
private analysis loop — a loop that is not part of this repository, so no label here is
resolvable by a reader of it; they are kept as the provenance of the decision. This
publication pass removed only the dead-pointer class: every reference into that loop's
working tree (its file paths and round-record names, its state record, charter, execution
model and replay contract, its scripts and module names, and the kanban card ids), each
replaced by the neutral phrase the sentence needs. Nothing else moved — no figure,
threshold, `§`/`ADR`/`CONF` id, code sample or contract sentence.
