# ADR-052 — the wizard edits policy families: the anchor grammar's `a.b[i].k` row, the per-family row set
over a `plan_policies:` root, and the shipped root's flip to the list

- Status: accepted
- Date: 2026-10-06 (round **R70**'s contract card, `R70-0`)
- Kind: **docs-only in this card.** It adds this ADR and amends `docs/spec.md` (§4.11) and
  `design/DESIGN.md` (§12.8's occupancy note, §12.9's register line, §12.14's grammar table and its
  askable-key-set clause). The **flip of the shipped root** to `plan_policies:` (§2.5) and the two
  rewritten contract sentences in `config.example.yaml` (§2.4) are *contract text here*: this card
  writes neither file. It changes **no product byte** — `crates/` is untouched by `R70-0` — and it
  adds no gate, moves no corpus, changes no threshold, mints no saving/latency/cost figure and touches
  no price.
- Authority: **the due-round record this card instantiates**, quoted verbatim in §1.1 —
  `design/DESIGN.md` §12.9's register line of 2026-10-06 (R69-2b) and ADR-051 §2.5.3 with its dated
  note. The round number (`R70`), this ADR's number (`ADR-052`) and the next free conformance id
  (`CONF-105`) are fixed by the card's own "number constraints" section. The card's body also fixes
  what this card may not do (no `crates/` change, no live upstream call, no push, no merge).
- Related: AGENTS hard constraints **1** (the byte boundary — the wizard edits lines, never
  re-serializes), **2** (content determinism), **5** (no fabricated prices — why a *vendor fact* is
  never prompted), **6** (no change-detector tests), **8** (docs before code), **9** (the measurement
  is not part of the search space: no gate, corpus or existing assertion moves here); **ADR-012** (a
  gate change is a human decision — why §2.6 allocates nothing); **ADR-014** (plan-first routing —
  per-family state and the walk); **ADR-025** (the write strategy: a verbatim template plus anchored
  single-line edits); **ADR-037 / ADR-038** (the split pair, the shape step); **ADR-046** (the
  rule-file lane); **ADR-049** (§4 the list spelling and the exactly-one-of ladder, §5.5/§5.6 the
  metered candidate set and the ranking, §7(a) the pool's `--check` rows); **ADR-051** (§2.5.3 the
  second family, §5.1 the §4.11 change it deliberately did **not** make, and its two dated notes);
  spec §4.6, §4.6.1, §4.8, §4.11, §4.12, §4.14, §9.3; DESIGN §12.5, §12.8, §12.9, §12.14.
- Cases: **none allocated, and that is a decision** (§2.6): `CONF-105` is **not** spent by this
  round. The wizard's row set is witnessed in-crate, by the section table's own test module read over
  the **shipped root** — the witness R68 could not have (its two shapes did not appear in the shipped
  pair at all) and the one this round makes possible. No existing ID is reused or renumbered, and
  **`CONF-98`/`CONF-99` remain the parked branch `round/67-abandoned-attempt`'s** (that branch is
  never read, switched, merged or deleted by this round).
- **Line-number convention.** Every `path:line` below resolves at **this branch's HEAD** (the commit
  that carries this ADR). The implementing round's edits shift the numbers in the files it touches;
  where that matters the drift is stated rather than left to be discovered.

---

## 1. Why

### 1.1 The due-round record, verbatim

`design/DESIGN.md` §12.9, register line of 2026-10-06 (R69-2b), verbatim:

> **Register line (2026-10-06, R69-2b): ADR-051 §2.5.3 — the shipped root's second `plan_policies:`
> family — deferred.** Reason: the obstacle is the wizard's anchoring contract, not the parser (the
> list spelling has parsed since ADR-049, `stats.rs:846`); the shipped pair's own contract sentences
> (`config.example.yaml:179-180` 「at most one in v0.1」, `:218-221` 「shown, never edited」) and the
> five existing CLI assertions that stand on them (`config_load.rs:379`, `setup/anchor.rs:1069`,
> `setup/sections.rs:551/770/788`) are the witness, and flipping the shipped root now would make a
> fresh `vadis setup` silently unable to edit any family. Due round: the one that teaches the wizard's
> anchor syntax its support list and edits it (spec §4.11 + the `sections.rs` line construction).

ADR-051 §2.5.3 states the deferred edit's shape (`plan_policy:` -> `plan_policies:` with the two
families `glm-5.3` and `glm-5.3-flash`); ADR-051's dated note of the same date states the mechanism
("the wizard never prompts for the list, so the failure is a quiet no-op"). This card is that due
round's contract.

### 1.2 The measured state at HEAD, and the one premise it corrects

Everything in this paragraph was **measured on this branch's HEAD** (`6bc48bf`) by the offline probes
committed with this ADR under `autowork/harness/r70-0/` — the shipped grammar was lifted out of
`crates/vadis-cli/src/setup/anchor.rs` and driven directly
(`anchor_probe.rs` / `anchor_probe.txt`), and the wizard was driven as the real binary
(`make_list_root.py` / `wizard-over-a-list-root.txt`). No provider was dialled and no credential was
read; `--print` and `--non-interactive` are read-only and `serve` was only used to observe a
load-time refusal.

1. **The row set is the defect, and only the row set.** Over a root writing `plan_policies:`, the
   routing section builds **zero** key rows (`setup/sections.rs:315-317` returns early), so
   `vadis setup --print` prints, for `routing`, exactly two display lines —
   `plan_policies: family glm-5.3 — overflow_selection declared` and the same for `glm-5.3-flash` —
   and **no** editable row; and a writing run over the same root is a **byte-identical no-op**
   (`rc=0`, `no change`, the file's bytes unchanged). Both halves are reproduced in
   `wizard-over-a-list-root.txt`.
2. **The anchor grammar is not the defect — it already reaches every key.** The shipped resolver
   answers `plan_policies[i].<key>` for **all eight** keys of both families, including a commented-out
   `overflow_monthly_cap_usd` (`enabled=false`, which is what makes a per-family `set-enabled` edit
   possible). Measured rows (`anchor_probe.txt`): `plan_policies[0].family` -> `glm-5.3` (line 5),
   `.primary` -> `zai-plan/glm-5.3`, `.overflow` -> `zai/glm-5.3`, `.on_primary_exhausted` -> `spill`,
   `.recover` -> `probe`, `.cooldown` -> `15m`, `.overflow_selection` -> `declared`,
   `.overflow_monthly_cap_usd` -> `20.0 (commented out)`; `plan_policies[1].family` -> `glm-5.3-flash`;
   a trailing index -> `NoSuchKey`. **No grammar extension is needed for reachability**, and
   DESIGN §12.14's sentence "a `plan_policies[i]` entry is a multi-line mapping, so `a.b[i]` cannot
   reach inside it" is imprecise in exactly one way: the *terminal* form `a.b[i]` cannot, while
   `a.b[i].k` — a row the grammar table does not yet list but the resolver already implements — can.
3. **One trap the row set must never touch.** `plan_policies[0]` (a terminal index on a mapping
   entry) does **not** refuse today: it resolves to the dash line's own first key text, extent
   `4..19` of `  - family: glm-5.3`, value `"family: glm-5.3"` (measured). A `set-value` edit on such
   a row would rewrite **structure** — the class spec §4.11's write strategy refuses everywhere else.
   No row may be built on it, and §2.1 requires the resolver to refuse it.
4. **The flip loads, and one sentence of the round's card is wrong.** A root whose `plan_policy:`
   block is replaced by the two-family `plan_policies:` block of §2.5 loads and starts (the only
   complaints are the expected offline ones: no API keys in the environment, and the cache-guard
   plugin). **But the card's premise that "both families are single-currency" is false**, and the
   measurement says so plainly (`list-root-currency-measurement.txt`): with `overflow_selection`
   left at `declared` nothing compares anything — that is why the flip is legal — while switching
   **either** family to `cheapest` is **refused at load**, naming that family's own key
   (`plan_policies[0].overflow_selection` for `glm-5.3`, `plan_policies[1].overflow_selection` for
   `glm-5.3-flash`) with "family '...' mixes currencies across its metered candidate set: provider
   'zai-cn' is currency CNY". The second family mixes because §4.8 makes the family tag default to
   the model's own id, so the CN metered entry `zai-cn/glm-5.3-flash` (CNY) joins family
   `glm-5.3-flash`'s candidate set without carrying a `family:` line. Nothing in the decision below
   depends on the wrong premise — `declared` is kept (§2.5) — but the shipped comment that states the
   refusal for `glm-5.3` alone is corrected to say **either** family (§2.4(c)), because that is what
   the loader does.

---

## 2. Decision

### 2.1 (a) The anchor grammar's support list: one new row, one refusal, and a per-key adjudication

**The grammar row, normative.** The anchor grammar (DESIGN §12.14) gains exactly one documented form,
which the shipped resolver **already implements**:

| Form | Resolves to |
|---|---|
| `a.b[i].k` | the key `k` of the **i-th entry** (0-based) of the block sequence `b`, when that entry is a block mapping — including an entry whose first key rides its own dash line (`- family: x`) |

**The terminal forms are declared unreachable, and one of them becomes a refusal.** `plan_policies`
(the header) is already `NotSettable` ("the value is not on this line (a block or empty value)").
`plan_policies[i]` and `providers[name=X]` as **terminal** selectors answer with the dash line's own
text today (§1.2 item 3) — which is the documented grammar's *not* answer ("when that entry is a
single-line scalar"; a mapping entry is not one). The implementing round therefore **narrows the
resolver** to match the frozen grammar: a terminal `[i]` / `[name=]` / `[id=]` selector whose entry
is a **mapping** is `NotSettable("a mapping list entry is not a single-line scalar")`. No existing row
uses such a path (every row ends in `.k`), so no current behaviour moves; and the rule becomes
machine-checked instead of remembered.

**The askable key set over a `plan_policies:` root, adjudicated per key.** For **each** declared entry
`i` (declaration order), the `routing` section's row set is these eight, in this order:

| # | key path | state | why |
|---|---|---|---|
| 1 | `plan_policies[i].family` | **editable** (`Ask::Line`) | a single-line scalar, measured to resolve; the single spelling asks `plan_policy.family` today, so a list root that did not ask it would be the very asymmetry this round removes. Its real risks are the loader's, not the wizard's: a tag no roster entry carries leaves the family's plan tier empty, and two entries on one tag is a **load refusal** (`config.rs:1358-1371`, naming `plan_policies[i].family`) — so a mistaken answer is refused, never written. |
| 2 | `plan_policies[i].primary` | **editable** (`Ask::Line`) | a `RouteSpec` scalar; an unresolvable route is refused at load naming `plan_policies[i].primary`. |
| 3 | `plan_policies[i].overflow` | **editable** (`Ask::Line`) | same, and the loader additionally refuses `primary == overflow`. |
| 4 | `plan_policies[i].on_primary_exhausted` | **editable** (`Ask::Enum(&["spill","block"])`) | closed enum, one of two spellings; parity with the single spelling's row. |
| 5 | `plan_policies[i].recover` | **editable** (`Ask::Enum(&["probe","none"])`) | closed enum; parity. |
| 6 | `plan_policies[i].cooldown` | **editable** (`Ask::Line`, duration-validated) | parity; requires the table's `is_duration` test to recognise the list path shape (§2.7 item 3) — otherwise a malformed duration would be encoded and only the loader would catch it. |
| 7 | `plan_policies[i].overflow_selection` | **editable** (`Ask::Enum(&["declared","cheapest"])`) | parity; `cheapest` over these rosters is refused at load naming that family's key (measured, §1.2 item 4), so the wizard cannot write a ranking the loader will not accept. |
| 8 | `plan_policies[i].overflow_monthly_cap_usd` | **editable** (`SetValue` **and** `SetEnabled`) | the shipped block carries it commented out per family, and a commented key inside a list entry resolves with `enabled=false` (measured) — so both edits are available, exactly as they are for the single spelling. |
| — | `plan_policies` (header) | **unreachable** | a block value: `NotSettable`. |
| — | `plan_policies[i]` (entry, terminal) | **unreachable by rule**, refused after §2.1's narrowing | it would rewrite structure (the dash line's own text) rather than a value. |
| — | the list's **membership** — how many families exist, and in what order; adding or dropping one | **display-only** (one line per family, §2.2) | the writer has no insert and no delete (spec §4.11's *what is deliberately not a section*; DESIGN §12.9's Q21). A family is *added* by hand, like an alias or a `fallback` entry. |

The three states are therefore all inhabited, and the rule that decides between them is one sentence:
**a key the locator can reach as a single-line value is editable; a structure is displayed; a
structure's member treated as a value is refused.**

### 2.2 (b) The row set over a `plan_policies:` root, and what the read-only surfaces then print

**The row set.** `rows_for(Routing, text)` returns, when `text` writes `plan_policies:`, the eight
rows of §2.1 **for each declared entry, family-major** (all eight of entry 0, then all eight of entry
1, ...). Family-major is chosen because one family is one unit of intent (ADR-049's families are
independent), and because the answers are read in one sitting per family; key-major was considered
and rejected. Over a `plan_policy:` root the row set is unchanged: today's eight singleton rows. The
two spellings therefore ask **the same questions, one family at a time** — which is the property
whose absence made the list spelling a no-op.

**The display lines.** `show_entries(Routing, text)` over a list root returns:

- one line per declared family, before the key rows — the membership fact:
  `plan_policies[<i>] — family <tag>`, with the note that membership (add / drop / reorder a family)
  is a hand edit and that the keys below **are** prompted; and
- the section's ordinary display lines — the `aliases` header, one line per alias, and the `fallback`
  count line — **which today's early return drops** (`sections.rs:416-418` returns before them). A
  read-only surface that hides the aliases a file carries, because the policy is spelled as a list, is
  the same class of defect as the no-op this round removes; the union is the fix.

**What `--print` prints for the two families.** One row per key per family, in the §2.1 order, then
the plugins rows — e.g. `plan_policies[0].family = glm-5.3`, ..., `plan_policies[1].overflow_selection
= declared (commented out)`, ... — with the membership lines above them. Nothing else on that surface
moves.

**What `--check` prints for the two families: nothing, and that is the assertion.** `--check`
enumerates the environment-variable **names** the file writes (one row per provider credential, one
per name of a pool, plus the token; `report.rs:67-78`) and no config key at all. The policy spelling
names no environment variable, so over the shipped root after the flip `--check`'s stdout is
**byte-identical** to what it is over the shipped root today. No per-family row exists on `--check`
and none is added.

**spec §4.11's amendment, in the document's own terms.** The `routing` row of the section table gains
the list spelling's key set; the *two spellings* paragraph's `plan_policies` bullet becomes two
bullets — the **per-family value rows** (editable) and the **membership line** (display-only) — each
naming its witness; and the sentence that says both ADR-049 shapes are "witnessed by the section
table's own unit tests ... rather than by a conformance case" is **kept and strengthened**: over the
list spelling the witness is no longer a fixture the shipped pair cannot supply, it is the shipped
root itself (§2.6).

### 2.3 (c) The five existing CLI assertions, and the shape each takes after the flip

These are the assertions that stand on the shipped pair and therefore move when the shipped root
flips. The **scan** that produced the set — a repo-wide read of every `plan_polic` occurrence in
`crates/` and `tests/`, base vs HEAD — found **exactly these five** reading the shipped example;
every other occurrence reads its **own** fixture (the `anchor.rs` unit tests' `DOC`, `config.rs`'s
constructed configs, `conf_41/55/85/93`'s rigs) or is product code, and the singular `plan_policy:`
stays a **legal** key, so those do not move. The semantics below are the contract; the literal diff
is the implementing card's.

| # | site (at this ADR's HEAD) | what it asserts today | its shape after the flip |
|---|---|---|---|
| a | `config_load.rs:369-391` (`the_shipped_example_parses_and_carries_the_plan_first_keys`) — the `.plan_policy` `expect` at `:379`, the `family == "glm-5.3"` at `:380` | the shipped example **exercises** `plan_policy` (one family), and the roster has both a `CodingPlan` and an `Api` entry | the example exercises the **list**: `plan_policies` is `Some`, its length is 2, and the family tags are exactly `{glm-5.3, glm-5.3-flash}`; the `.plan_policy` arm is deleted (a `plan_policy` Some **and** a `plan_policies` Some is the ladder's refusal). The two roster assertions are unchanged. |
| b | `setup/anchor.rs:1049-1073` (`every_row_of_the_real_example_resolves`) — the path list `:1061-1067` | each of the eight `plan_policy.*` paths resolves in `config.example.yaml` | the same property over the list: for **each declared index i** of the shipped root and each of the eight keys, `plan_policies[i].<key>` resolves. The index list is read from the file (`entry_names`-style enumeration), never hard-coded — so a third family added by hand moves the test with it. |
| c | `setup/sections.rs:547-558` (`every_static_row_resolves_in_the_example`) | every `STATIC_ROWS`/`ENABLE_ROWS` path resolves in the shipped example **of its target file** | the property is unchanged for the static rows; the routing section's list rows are **dynamic** (built per declared entry, like the `providers`/`plugins` rows), so the witness is `rows_for(Routing, example())`: every path it builds must resolve in the shipped root. The two tests stay two tests: one over the static table, one over the built row set. |
| d | `setup/sections.rs:766-778` (`a_plan_policies_root_is_shown_never_edited`, its second half) — `:770-772` the `overflow_selection` and cap rows over `example()`, `:776-778` the `aliases` display line over `example()` | over the shipped root, the routing section builds the eight `plan_policy.*` rows **and** shows the `aliases` line | over the shipped root (now a list root) the routing section builds the eight rows **per family** — asserted as `plan_policies[0].*` and `plan_policies[1].*` and as the relation "8 x the number of declared families" — and builds **no** `plan_policy.*` row; and it still shows the `aliases` line (the union of §2.2). The first half of the test — the list-root **fixture**'s display lines — is re-pointed: the fixture no longer witnesses "shown, never edited", it witnesses the **membership** lines and the per-family rows. |
| e | `setup/sections.rs:785-803` (`commented_spellings_are_inert`) — `:788` `!top_key_present(&root, "plan_policies")` over `example()` | the shipped root's **commented** `plan_policies:` spelling is inert (and the shipped roster's commented `api_keys:` is) | the property inverts into two: (i) the shipped root **does** write the live list — `top_key_present(&example(), "plan_policies")` is **true**, and the section builds the list rows; (ii) a **commented** spelling is still inert — witnessed by a fixture text carrying `# plan_policies:`, since the shipped root can no longer be the inert carrier. The roster half (a commented `api_keys:` line is not a live pool, one `api_key_env` row for `deepseek`) is unchanged. |

**Nothing else may move**, and a sixth site is an escalation, not a wider table: if the implementing
card finds another assertion reading the shipped example and depending on the singular spelling, it
**escalates** (a new owner decision) rather than migrating it quietly.

### 2.4 (d) The two contract sentences in `config.example.yaml`, verbatim

The file itself is **not** written by this card; these are the sentences the implementing card lands,
and no other wording is authorised by this ADR.

**(a) `config.example.yaml:178-182` today** (inside the `plan_policy:` block's header comment):

```
plan_policy:
  # spec §4.6 / ADR-014 — the subscription first, the metered account as the
  # spill. Optional, and **at most one** in v0.1 (a second family is an additive
  # future key, never a reshaped section). The keys and this example land
  # together (GAP-Q15): `deny_unknown_fields` makes an example that carries a key
  # the parser does not know an unservable file.
```

becomes, at the head of the `plan_policies:` block:

```
plan_policies:
  # spec §4.6 / ADR-014 / ADR-049 §4 — the subscription first, the metered account
  # as the spill. A **list** of families, one entry each: the shipped default, and
  # the shape `vadis setup` walks family by family (spec §4.11). `plan_policy:`
  # (one family, no list) stays a legal alternative spelling, and exactly one of
  # the two keys is written — a second family in one file is an entry here, never
  # a second `plan_policy:` section. The keys and this example land together
  # (GAP-Q15): `deny_unknown_fields` makes an example that carries a key the
  # parser does not know an unservable file.
```

**(b) `config.example.yaml:210-221` today** (the *fan-out's other keys* banner; the sentence to
rewrite is its last paragraph):

```
  # --- the fan-out's other keys (spec §4.6.1 / ADR-049) — written nowhere on purpose -------------
  # overflow_selection: declared   # the default is this file's behaviour. `cheapest` ranks the family's
  #   metered candidates by price (input_miss, then output, then roster order) instead, and it **refuses to
  #   load** over this roster: every family below pairs an intl (USD) metered route with a cn (CNY) one, and
  #   a ranking is a comparison — two currencies are never compared (spec §4.8). Measured 2026-10-06:
  #   `cheapest` on family `glm-5.3` exits 2, "family 'glm-5.3' mixes currencies across its metered
  #   candidate set". A single-currency family is what the mode needs; drop the other currency's route, or
  #   keep `declared`.
  #   The other two spellings are roster/policy shapes, not keys of this block: a per-provider credential
  #   `api_keys:` pool (roster) and a `plan_policies:` list of families in place of the one `plan_policy:`
  #   above. `vadis setup` shows a pool and shows the list — neither is prompted, and neither is written by
  #   a run (spec §4.11).
```

becomes:

```
  # --- the fan-out's other key (spec §4.6.1 / ADR-049) — written nowhere on purpose ----------------
  # overflow_selection: declared   # the default is this file's behaviour. `cheapest` ranks a family's
  #   metered candidates by price (input_miss, then output, then roster order) instead, and it **refuses to
  #   load** over this roster: **either** family in the list above mixes an intl (USD) metered route with a
  #   cn (CNY) one (the family tag defaults to the model's own id, §4.8, so `zai-cn`'s GLM-5.3 entries join
  #   both candidate sets), and a ranking is a comparison — two currencies are never compared (spec §4.8).
  #   Measured 2026-10-06: `cheapest` on the second family exits 2 too, "family 'glm-5.3-flash' mixes
  #   currencies across its metered candidate set", naming `plan_policies[1].overflow_selection`. A
  #   single-currency family is what the mode needs; drop the other currency's route, or keep `declared`.
  #   The one remaining alternative spelling is a roster shape, not a key of this block: a per-provider
  #   credential `api_keys:` pool. `vadis setup` shows a pool — never prompted, never written by a run. The
  #   families above **are** prompted and written, one family at a time, by the keys inside each entry
  #   (spec §4.11).
```

The rewrite also carries the measurement of §1.2 item 4, which is why the corrected sentence says
**either** family and names the second family's refusal.

### 2.5 (e) The shipped root's flip, verbatim

`config.example.yaml`'s `plan_policy:` block (its header comment through the last line it owns,
`:177-226` at this ADR's HEAD) is replaced by:

```
plan_policies:
  - family: glm-5.3                    # required: the model id both routes carry — and the key of the family's state.
    primary: zai-plan/glm-5.3          # required: a roster route whose provider is `account: coding_plan`.
    overflow: zai/glm-5.3              # required: a distinct roster route whose provider is `account: api`.
    on_primary_exhausted: spill        # spill | block (default spill).
    recover: probe                     # probe | none (default probe).
    cooldown: 15m                      # default 15m: the floor between the move away from `primary` and the first admitted probe
    # overflow_monthly_cap_usd: 20.0   # optional: a ceiling on this family's **metered** spend in a UTC calendar month.
  - family: glm-5.3-flash              # the second family; its routes already exist in the roster (zai-plan, zai)
    primary: zai-plan/glm-5.3-flash
    overflow: zai/glm-5.3-flash
    on_primary_exhausted: spill
    recover: probe
    cooldown: 15m
```

**What the flip does and does not touch.** (i) The eight keys of `glm-5.3` are migrated **as they
are** — same values, same defaults; the per-key prose the single block carried is collapsed to the
short annotations above (the long explanations belong to the two contract sentences of §2.4 and to
spec §4.6/§4.6.1, which this file links rather than duplicates: AGENTS 5's single-source rule for
vendor facts, read as a rule for comments). (ii) `overflow_selection` is written **nowhere**, as today
(its commented line stays in the fan-out banner), so both families take `declared`. (iii) Both
families' routes are **already** in `providers.example.yaml` — `zai-plan/glm-5.3-flash` at `:140`
(`family: glm-5.3-flash`) and `zai/glm-5.3-flash` at `:205` (tag = its own id) — so the flip adds no
roster row and the pair still loads (measured, §1.2 item 4). (iv) The flip is **not** a licence to
write a currency claim: §2.4(b) records what the loader actually does.

### 2.6 (f) The witness: **no `CONF-105`**, and what witnesses what

**Decision: `CONF-105` is not spent by this round.** The reasons, in order of weight:

1. **The strongest available witness is in-crate and only exists after the flip.** R68's reason for
   declining a `CONF` id was that the shipped pair could not witness either shape — both appeared
   only inside comments. That reason is *dissolved* by this round: after the flip the **shipped root
   is a list root**, so `sections.rs`'s own test module asserts the row set **over the shipped file
   itself**. A conformance case would have to re-create that root in its own temp dir, which is
   strictly weaker than reading the file the round actually ships.
2. **The half a conformance case cannot reach is the half that matters.** The interesting behaviour
   is that an answer **lands** on a `plan_policies[i]` line — and that path is the *interactive* one,
   which spec §4.11 pins to a **PTY script**, not a Rust test harness ("the interactive path driven by
   a PTY script rather than by a Rust test harness"). A conformance case driving the real binary can
   therefore only observe the read-only surfaces, and on those the honest, checkable facts are
   `--print`'s row set (a table behaviour, §2.2) and `--check`'s byte-identity (asserted as an
   unchanged surface, not as a new row). A case whose only discriminating limb is "the row set is the
   one the table builds" duplicates the in-crate witness.
3. **Spending an ID is irreversible and allocating it needs the owner's act** (ADR-012; ADR-025's
   precedent that a gate-side id is allocated by the human). A card that cannot write the stronger
   assertion must not burn the id to approximate it.

**What witnesses the decision, then** — all of it **existing** assertions, migrated per §2.3, i.e.
this round adds no new gate and touches no gate definition:

| behaviour (observable) | witness | where it lives |
|---|---|---|
| the shipped root writes the list, both families parse, the family tags are exactly the two names | `the_shipped_example_parses_and_carries_the_plan_first_keys`, re-pointed (§2.3 a) | `crates/vadis-cli/src/config_load.rs` (unit) |
| every key of every declared family resolves in the shipped root | `every_row_of_the_real_example_resolves`, re-pointed (§2.3 b) | `crates/vadis-cli/src/setup/anchor.rs` (unit) |
| the row set over a list root is eight rows per declared family, and the shipped root's; no `plan_policy.*` row over it; the membership lines and the aliases lines are both shown | `every_static_row_resolves_in_the_example`, `a_plan_policies_root_is_shown_never_edited`, `commented_spellings_are_inert`, all re-pointed (§2.3 c/d/e) | `crates/vadis-cli/src/setup/sections.rs` (unit) |
| the embedded pair is a pair and validates together; the fresh all-defaults run is byte-identical to the embedded template (so the flipped template is proven to be what a fresh run writes, and to load) | `g1_defaults_write_the_template_verbatim`, `the_embedded_pair_is_a_pair` | `crates/vadis-cli/src/setup/mod.rs` (in-crate integration; CONF-67/CONF-69's shape) |
| the migrated template still refuses nothing it refused before (the shapes of the refusal ladder, over the flipped template as the `--force` base) | `g4_the_refusal_ladder_leaves_the_target_untouched` | `crates/vadis-cli/src/setup/mod.rs` |
| `--check`'s stdout does not move | `conf_86_setup_check_roster_fact` (pins that surface) + the byte-identity reasoning of §2.2 | `tests/conformance/tests/conf_86_setup_check_roster_fact.rs` |

**CONF-67/68/69/70/79's four-of-five check, item by item** (the card asks for this explicitly; those
rows' *files* are owed by R22-F4 and their witnesses are the in-crate integration tests above):

- **CONF-67** (the file is edited, never reproduced; the all-defaults output is byte-identical to the
  template): **holds.** The "template" is `EMBEDDED_TEMPLATE = include_str!("../../../../config.example.yaml")`
  (`setup/mod.rs:28`) — the *same file*, so editing `config.example.yaml` moves the embedded template
  in the same commit by construction, and the test compares the run's output to that same constant.
- **CONF-68** (the refusal ladder leaves the target untouched): **holds.** Every limb mangles the
  target's **own** bytes (`addr` renamed, duplicated, the `server:` block flow-mapped, an
  unloadable answer); none reads the policy block.
- **CONF-69** (idempotence; the second run writes nothing, bytes and mtime): **holds.** Its base is
  the run's own output over the embedded template; the assertion is a relation over the run's two
  hashes, never a count of rows.
- **CONF-70** (the canary; `--check`'s `0`/`4`/`2` exit codes; *names only*): **holds, and no new key
  name appears.** The flip introduces no environment-variable name (both families' routes are roster
  entries that already name their keys), and `--check`'s rows come from `key_pool()` +
  `server.auth_token_env` (`report.rs:67-78`). Its enumeration is a **relation** over the file's own
  names, not a list of config keys, so "covering the new keys" is vacuous here — which is the correct
  answer, and the reason §2.2 states `--check`'s byte-identity as an assertion rather than adding a
  row.
- **CONF-79** (the location rule): **untouched** — no path is involved.

### 2.7 (g) The implementing round's write set (`R70-1`, not this card's)

1. `crates/vadis-cli/src/setup/sections.rs` — `rows_for(Routing, ..)`: the list root's per-family
   rows, family-major, appended as **dynamic** paths (the `Box::leak(format!(...))` shape the
   `providers`/`plugins` rows already use, `sections.rs:341`), and the per-family `SetEnabled` row for
   `overflow_monthly_cap_usd` (the `ENABLE_ROWS` const can hold only the two fixed keys, so the
   dynamic enable row is built here); `show_entries(Routing, ..)`: the membership lines plus the
   aliases/fallback lines (the union); the `routing` fixture test's re-pointing (§2.3 c/d/e).
2. `crates/vadis-cli/src/setup/anchor.rs` — the terminal-selector narrowing of §2.1 (a mapping entry
   under a terminal `[i]`/`[name=]`/`[id=]` is `NotSettable`), its unit test, and the re-pointed
   shipped-example test (§2.3 b).
3. `crates/vadis-cli/src/setup/sections.rs` — `KeySpec::is_duration` (`:122-130`) must match the list
   path **shape** for `cooldown` (today it is an equality list of literal paths), or the per-family
   duration answer is encoded unvalidated.
4. `crates/vadis-cli/src/config_load.rs` — the re-pointed shipped-example test (§2.3 a).
5. `config.example.yaml` — §2.5's block and §2.4's two sentences.
6. `book/` + `README.md` — §2.8's sentences (AGENTS 8: the same change, not a later one).
7. `docs/spec.md` §4.11 and `design/DESIGN.md` §12.14 — the landed text this ADR drafts, plus
   DESIGN §12.9's register line and §12.8's occupancy note (`R70-0` writes the register forms; the
   implementing card carries the code).

### 2.8 (h) The user-facing prose the flip falsifies

The book is user-facing and links to the contracts rather than copying them (AGENTS 8); the shipped
template's spelling is the one place its prose can go stale silently. The flip falsifies these
sentences, each of which the implementing round rewrites **in the same change**:

| file:line | the sentence | why it moves |
|---|---|---|
| `README.md:26` | "`vadis setup` shows a provider's `api_keys:` pool and a root's plan-family keys, **and writes neither**" | false the moment the wizard edits a family: it now prompts and writes each family's keys, one family at a time; only the pool stays shown-never-written |
| `README.md:26` | "while the shipped template still carries the single `plan_policy:`" | the template now carries the list |
| `book/cost-and-caching.md:227` | "`plan_policy` appears **at most once** in v0.1 (one family)" | replaced by "one of the two spellings is written; the list carries one entry per family" |
| `book/cost-and-caching.md:214-221` | "One optional top-level `plan_policy` section then names the pair" + the `plan_policy:` sample | the shipped shape is the list; the sample becomes an entry of it |
| `book/cost-and-caching.md:330-339` | "the shipped template still carries the one `plan_policy:`" (the `plan_policies:` paragraph) | it no longer does |
| `book/getting-started.md:228` | "name that string in `plan_policy.family`" | true of the singular spelling; the shipped file's key is `plan_policies[i].family` |
| `book/observability-and-accounting.md:139` | "when a `plan_policy` is configured" | a family named by either spelling; the sentence should say so rather than name one key |

Sentences that stay true and must **not** be touched: `README.md:120` ("when a `plan_policy` is
configured" — the concept, not the key), `book/cost-and-caching.md:450-453` (the links to §4.6/§4.6.1),
and every sentence that describes both spellings.

### 2.9 (i) The number constraints, restated

- The round is **R70**; this ADR is **ADR-052**; the ADR file is
  `design/decisions/ADR-052-wizard-edits-policy-families.md`.
- **`CONF-105` is not allocated** (§2.6). The next free conformance id stays **`CONF-105`**;
  `CONF-98`/`CONF-99` remain the parked branch `round/67-abandoned-attempt`'s and are not touched —
  that branch is not fetched, merged or read by this round.
- The `DESIGN.md` §12.8 heading therefore **stays** `CONF-01…CONF-104`; its occupancy paragraph gains
  a dated note recording that R70 spends no id and why. `design/DESIGN.md` §12.9's R69-2b register
  line is **closed** by a new register line naming this round as the due round, its landed state, and
  the two things it decided *not* to do (no CONF id; the premise correction of §1.2 item 4).

---

## 3. Alternatives considered

- **Keep the list "shown, never edited" and flip nothing** (i.e. decline the round). Rejected: it is
  the state R69-2b registered as a defect, and the shipped template would keep a default whose
  families no run can edit — the silent no-op. The measured read-only surface (§1.2 item 1) is
  exactly the surface the repository calls its most expensive silent failure.
- **Extend the anchor grammar with a new syntax** (e.g. `plan_policies[family=glm-5.3].cooldown`, in
  the `[name=X]` family). Rejected: the index form already resolves (measured), an index is stable
  under the wizard's own edits (the wizard never adds or drops an entry, so `i` cannot drift under
  it), and a second selector is a second way to name one line — the drift class ADR-020 rejected for
  URLs and ADR-051 for `wire_api`/`supports`. The `[name=]` form remains available to any human who
  wants it; no row uses it.
- **Make `family` display-only** (it is the state key, so renaming it orphans the family's
  `plan_state` row). Rejected: the single spelling asks it today, and the wizard is not the
  configuration's validator — the loader is (spec §4.11's step 3/8). Making the list spelling *less*
  capable than the single one per key is the asymmetry this round exists to remove. The risk is
  stated in §2.1's table rather than answered by a refusal the writer does not own.
- **Ask one question per *key* across families** (key-major order: all families' `family`, then all
  families' `primary`, ...). Rejected as the default: one family is one unit of intent and the answers
  are given in one sitting per family; family-major also keeps a question's index (`plan_policies[i]`)
  constant for a whole block of prompts.
- **Witness the round with a new `CONF-105` on the real binary.** Rejected in §2.6 — it would be
  strictly weaker than the shipped-root unit witness for the row set, and blind to the interactive
  half a PTY owns. Registered as a possible owner choice with its draft in Appendix A.1.
- **Rename the "shown, never edited" sentence instead of changing the behaviour.** Rejected: the
  sentence is true of the membership and false of the keys; the fix is to split it, not to weaken it.

## 4. Rationale

- **The file already said what the software could not do.** The roster carries both families' routes
  and the parser has accepted the list since ADR-049; only the wizard's row set was blind to it. This
  round makes the writer agree with the file — the same direction ADR-020 (URLs), ADR-049 (the list)
  and ADR-051 (the declared cell) each took.
- **A row set is a contract a human can check.** "Over a list root the wizard asks these eight keys
  per family, and shows the membership instead" is checkable at `--print` and at the table; "the
  wizard edits families" is not.
- **The witness moves to the shipped file, which is the strongest place it can be.** R68's fixture
  existed because the shipped pair could not witness the shapes. After the flip it can, and a fixture
  that duplicates the shipped root is exactly the drift the split's own tests avoid.
- **Nothing here invents a saving, a price or a measurement.** No transform runs, no figure is
  produced, D3 stays unmet, and AGENTS 4/5 are untouched.

## 5. Consequences

### 5.1 Docs (this card)

- `design/decisions/ADR-052-wizard-edits-policy-families.md` — this file.
- `docs/spec.md` §4.11: the `routing` row of the section table gains the list spelling's key set; the
  *two spellings* paragraph's `plan_policies` bullet splits into the per-family value rows and the
  membership line; the witness sentence is strengthened to name the shipped root; the *what must be
  asserted when this lands* list gains the row-set line. §9.3 is **unchanged** (checked and said so:
  the flip adds and removes no surface — `setup` was served before and after, and no listed deferred
  surface is affected).
- `design/DESIGN.md` §12.14: the grammar table gains the `a.b[i].k` row; the askable-key-set
  paragraph's clause (iii) is rewritten from "shown, never edited" to §2.1/§2.2 (with the terminal
  selector's refusal); its dated note records R70.
- `design/DESIGN.md` §12.8: the heading does **not** move (no id allocated); the occupancy paragraph
  gains this round's dated note.
- `design/DESIGN.md` §12.9: a new register line closing R69-2b's, with the landed shape, the two
  decisions *not* taken (`CONF-105` unspent; the currency premise corrected and the corrected comment
  recorded).
- The two `config.example.yaml` sentences and the flip are contract text here (§2.4, §2.5).

### 5.2 Code (`R70-1`'s write set)

§2.7's seven items. The two files of the shipped pair are written in the commit that lands the row
set, because the wizard's own `--print`/`--check` surfaces and its fixture tests read them in the
same `cargo test` run.

### 5.3 The register (non-blocking)

| id | item | owner | due |
|---|---|---|---|
| `R70-0-F1` | `CONF-105` is unspent (this ADR §2.6). If the owner prefers an on-wire case for the row set, Appendix A.1 holds the draft; allocating it is the owner's act (ADR-012). | owner | open |
| `R70-0-F2` | the round's card states both families are single-currency; the measurement says **either** family mixes once ranked, because a family tag defaults to the model id (§1.2 item 4). Corrected in §2.4(b); nothing else depended on it. | — (recorded) | n/a |
| `R70-0-F3` | the resolver answers a terminal `[i]`/`[name=]` selector on a mapping entry with the dash line's text (the trap of §1.2 item 3). `R70-1` narrows it; until then the path is declared out of the row set but is not refused by the locator. | `R70-1` | this round |
| `R70-0-F4` | the wizard's *membership* edits (add / drop / reorder a family) remain hand edits (Q21's boundary). A guided insert would need its own contract (position, indentation, block style) and its own round. | owner (if wanted) | open |
| `R70-0-N1` | Cost: this card is docs-only, offline, `$0.00` — no provider dialled, no credential read. The probes ran the workspace's own binary and the shipped grammar locally. | — (note) | n/a |

### 5.4 The R69-4b low findings, dispositioned (not repaired)

R69-4b's independent verification (`autowork/harness/r69-4b/EVIDENCE.md`, item 1 and item 3) left
three low findings open against R69's artefacts. This card **registers their disposition and repairs
none of them** — ADR-051 is append-only, and none of the three is `R70`'s to change:

| # | finding | disposition |
|---|---|---|
| 1 | a **third** `forward.rs` unit test (`declared_cell_with_foreign_wire_api_serves`) was added, where ADR-051 §5.3 authorised "each gains an arm" on two named tests | **not ratified here, and not repairable here**: an architect contract card cannot widen an authorisation the owner granted (AGENTS 9 / ADR-012), and `R70`'s write set does not contain `forward.rs`. Registered as an **open item for the owner** — one sentence either way — with the measured facts (strictly additive; in the same shape as the two authorised arms; green in R69-4b's own verification). |
| 2 | ADR-051's commit `b8374c7` edited **one body line** (a `docs/spec.md:1132 → :1166` pointer refresh) although the ADR is append-only | **registered as a known defect.** The edit predates the file's first dated note, it is content-correct (a stale pointer), and append-only forbids rewriting it now; the commit is named so a reader can see the deviation rather than infer it. |
| 3 | the orchestrator's `fn`-count pair (`1408`/`1418`) could not be reproduced under four counting bases | **the counting convention is fixed to R69-4b's**: the **unique function-name set**, whole-file extraction, per ref — `base 1034 / HEAD 1037`, added = `{family_guard_reentry, provider_with, declared_cell_with_foreign_wire_api_serves}`, removed = none. The claim the numbers were used for (zero removals, every addition attributable) is reproduced on that basis and on all four R69-4b tried. This ADR's own figures quote the same convention. |

## 6. Honest boundaries and verification owed

- The four measurements of §1.2 were taken **offline on this branch's HEAD**; the probes and their raw
  outputs are committed under `autowork/harness/r70-0/`. No upstream was dialled and no credential
  read; `serve` was invoked only to observe two load-time refusals.
- The **flip itself is not measured here** — this card does not write `config.example.yaml`. What is
  measured is that the block of §2.5, placed in the shipped root's position, **loads** (and that
  ranking either family is refused; §1.2 item 4). That the *landed* pair still passes the four gates
  is `R70-1`'s instrument, and the six witnesses of §2.6 are the assertions it must keep green.
- §2.2's `--check` byte-identity is a **derivation from the enumeration's source** (`report.rs:67-78`
  reads environment-variable names only) plus the fact that the flip introduces no name; the
  implementing card owes the confirmation on the landed pair, and `conf_86` pins the surface.
- §2.3's scan is the card's own: a repo-wide read of `plan_polic` in `crates/` and `tests/` at HEAD,
  which yields the five sites of that table and no sixth. A sixth site is an escalation (§2.3's last
  paragraph), not a wider table.
- Nothing in this ADR is a saving statement and none may be derived from it: no transform runs, no
  token delta is produced, and the round mints no figure.

## 7. Reversibility

Cheap in both directions. Reverting the row set is restoring the early return at `sections.rs:315-317`
and the display-only line; reverting the flip is replacing the `plan_policies:` block with the
single `plan_policy:` one (one block, one file, no roster change — the roster's second family's routes
already existed and stay declared either way); reverting the two example sentences is a two-hunk edit;
reverting the resolver narrowing is deleting the guard (and the trap returns, which is why the guard
is a fix to the documented grammar rather than an extension). What is **not** reversible is a served
request — nothing here touches the serving path, so this round cannot produce one. No user data is
migrated by anything in this ADR: the flip changes a template, and an operator's own file keeps
whatever spelling it has (both load).

---

## Appendix A

### A.1 A draft for `CONF-105`, **not allocated** (`R70-0-F1`)

Held here so that the owner can authorise an on-wire case without a new contract round. It is **not**
a proposal and nothing in this ADR depends on it; §2.6 states why the round prefers the shipped-root
unit witness. If allocated, the case would drive the real binary as a subprocess (CONF-86's shape) and
its limbs would be:

| limb | assertion | red baseline (measured at this HEAD) | discriminator |
|---|---|---|---|
| 1 | over a root that writes `plan_policies:` (the case's own temp dir, the shipped pair copied in), `vadis setup --print` emits, for `routing`, one row per declared family per key — `plan_policies[i].<key>` for all eight keys and both entries — and **no** `plan_policy.*` row | the same command emits **only** the two membership lines and no key row (`wizard-over-a-list-root.txt`) | the row count is a **relation**: `8 x (the number of entries the file writes)`, never a literal — a third family added by hand moves the case |
| 2 | over the same root, `vadis setup --non-interactive` exits 0, prints `no change`, and leaves the root's bytes identical | identical at the base — **this limb discriminates nothing** and is a control only | n/a (recorded so the case cannot be read as witnessing the write path, which a PTY owns) |
| 3 | over the shipped root itself, `vadis setup --check`'s stdout is byte-identical before and after the flip | green by construction | the flip introduces no environment-variable name |

### A.2 The R69-4b findings in long form

See §5.4. The primary evidence is `autowork/harness/r69-4b/EVIDENCE.md` (items 1 and 3) with its
scripts (`fnset_diff2.sh` for the name-set convention, `fncount*.sh` for the counting-basis reconciliation).
