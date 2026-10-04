# ADR-048 — rename the two deferred surfaces

- Status: accepted
- Date: 2026-10-04 (round **R65**'s contract card, `R65-0`)
- Kind: **a naming contract, docs-only in this card.** It renames no product byte *here* — `R65-1`
  executes it, `R65-0b` audits this contract before `R65-1` is cut. It **does** authorise the edit of
  three conformance assertions (`conf_87`, `conf_46`, `conf_25`) and the mechanical path-follow edits in
  29 further case files; each is enumerated in §2 and each rests on the owner act quoted in §1. It adds no
  gate, moves no corpus, changes no threshold, mints no saving/latency/cost figure, and touches no price.
- Authority: **the owner's direction of 2026-10-04**, quoted verbatim in §1.1.
- Related: **ADR-047** (adopt the name `vadis` — whose §D *deferred* exactly these two surfaces and whose
  authorisation §4 records the boundary this ADR crosses); **ADR-041** (the authoritative owner of the
  metric contract — the surface's shape, its 21 series and its single-owner rule); **ADR-009** (the
  SQLite/WAL store — whose *filename* this ADR changes and whose *one-writer* rule it does not);
  **ADR-012** and `AGENTS.md` constraint **9** (**the measurement is not part of the search space** — the
  reason a conformance edit needs an owner act); constraint **1** (the byte boundary — untouched here),
  constraint **2** (content determinism — the metric *names* are not content), constraint **8** (docs
  before code); `design/RENAME-MAP.md` §D (the deferral this ADR closes) and §G (the gate whose
  expected-survivor list this ADR amends).
- Cases: `tests/conformance/tests/conf_87_metrics_single_owner.rs` (**its `FROZEN_SERIES` array and its
  ~100 name literals**), `tests/conformance/tests/conf_46_metrics_is_served.rs` (**4 name literals**),
  `tests/conformance/tests/conf_25_config_driven_serve.rs` (**one assertion**); plus the *fixture-path*
  edits in the 29 case files that read the store the product wrote (§2.3). No id is spent, reused or
  renumbered.
- **Line-number convention.** Every `path:line` below resolves at **this branch's HEAD** — the commit
  that carries this ADR, cut from `main` (`37e099a`). R65-1's own edits shift the numbers in the files it
  touches; the deltas are stated where they matter (§10) rather than left to be discovered. Citations
  into `design/RENAME-MAP.md` use **section anchors** (`§D`, `§G`), not line numbers, because this same
  change amends those sections and renumbers the file.

---

## 1. The authority for crossing the deferral boundary

### 1.1 The owner's authorisation, verbatim and dated

**The owner's direction of 2026-10-04**, verbatim:

> *"router_\* 指标序列 + state/router.db — 单开一轮"*
> — *(the `router_*` metric series + `state/router.db` — give them a round of their own)*

Three facts about that sentence are part of the record, and none may be inferred away:

1. **It is an owner act, not a loop outcome.** Both surfaces are pinned by conformance assertions
   (`conf_87`/`conf_46` for the metric names, `conf_25` for the store path). `AGENTS.md` constraint 9 /
   ADR-012 put *"the gate definitions, the fixed corpus, the conformance assertions and the L1 envelope"*
   outside the loop's mutable scope: **no round may take this decision; a round may only carry it.** That
   is what §2–§3 do.
2. **It is dated and scoped**: *these two* surfaces, in one round of their own (`单开一轮`), and nothing
   else. §9 lists everything it does **not** reach.
3. **It closes the deferral ADR-047 left open.** ADR-047 D5 + §4 deliberately did **not** sweep the
   metric series or the store filename, registered them in `RENAME-MAP.md` §D, and named each surface's
   follow-up as *"a round that renames X **and** states the migration"*. This is that round: §5 states
   the store migration, §7 names the metric rename's consumer set.

### 1.2 What ADR-047 said, re-read at this HEAD

| Artifact | What it says, closely | Status here |
|---|---|---|
| ADR-047 D5 (`ADR-047-adopt-the-vadis-name.md:107-114`) | the rename is total *except* the two allowlists in `RENAME-MAP.md`; §D DEFERRED = "the metric series names, `router.db`" | **Superseded for those two entries only** — §8 of this ADR closes them out of §D |
| ADR-047 authorisation §4 (`:74-77`) | *"Two surfaces are deliberately NOT swept (owner-scoped, registered in `RENAME-MAP.md` §D): the `router_*` metric series names (pinned by `conf_87`) and the fixed store filename `state/router.db` (pinned by `conf_25`). Each is a separate decision with its own authorisation"* | **The separate authorisation is §1.1 above** — the sentence's own precondition, now met |
| ADR-047 *Alternatives* (`:139`) | *"Rename the metric `router_*` series and the store filename in the same sweep — Declined **in this round**, not in principle"* | Declined then, **taken now**, on the owner's newer act |
| ADR-047 D8 (`:127-130`) | *"`conf_87` (metrics) and `conf_25` (store path) are **not** edited"* | **Reversed here, by name** — §2 names the edit and its authorisation |
| `RENAME-MAP.md` §D | the two deferral rows, each with the follow-up that this round discharges | **Closed** — the §D update lands in this same change (§8) |
| `RENAME-MAP.md` §G | expected survivors = §C + §D ("any `router_<metric-name>` … and `router.db`") | **Amended** — after R65 the two §D entries are no longer legal survivors, and this ADR joins the gate's file-exclusion list (§8.3, §8.4) |

### 1.3 What the authorisation does **not** license

By its own terms the owner's sentence reaches two surfaces and nothing else. It does **not** reach:

- **the §C protected set** — `route`/`routing`/`reroute`/`resolve_route`/`route_*`/`*_route`/
  `failover_from`/`plan_switch`, `axum::Router`, `health_router`, `OpenRouter`, `routers`,
  `router-auto-suite/1` (RENAME-MAP §C). Untouched, and §9.1 adds one token to that set.
- **the trace schema.** `TRACE_SCHEMA_VERSION` stays **2**; no field is added, removed or retyped. The
  store's *filename* changes; no `events` row, no `Query`, no index and no `body_hash` convention moves.
- **any byte on the passthrough path.** The two permitted mutations (constraint 1 / ADR-015) stay two;
  neither surface here is a request or response byte.
- **any price, quota, threshold, gate, corpus or L1 envelope** (constraint 9). The metric *names* change;
  no *figure* changes — every series renders the same value it does today (§4.3).
- **the store's location rule, its writer, or its `state/` directory** (ADR-009, spec §4.5, ADR-040 D5).
  Only the leaf filename moves.
- **the product name beyond these two surfaces.** Nothing else in the tree that contains `router` moves
  in R65 (§9 enumerates the survivors).

---

## 2. The authority chain, surface by surface

### 2.1 Surface 1 — the 21 `router_*` metric series names → `vadis_*`

**The pinning assertions.** Two cases carry the names as literals, and both must be edited or the suite
goes red:

| Case | What pins the names | Authorised by |
|---|---|---|
| `conf_87_metrics_single_owner.rs` | the `FROZEN_SERIES: [&str; 21]` array (`:51-73`), the `expected_provenance` match arms (`:77-92`), and ~100 further name literals through `:1324` — **106 occurrences, 21 distinct names** | the owner act §1.1: the series set *is* "the 21 `router_*` metric series names" |
| `conf_46_metrics_is_served.rs` | 4 literals of **one** name: `# HELP router_metrics_window_seconds` (`:247`), `# TYPE … gauge` (`:248`), `router_metrics_window_seconds 900` (`:250`, `:348`) | **the same owner act** — this case asserts a member of the same 21-name set |

`conf_46` is the **gap the card's enumeration did not name**: the card lists "the 21 `router_*` metric
series names" as one surface but names only `conf_87` as its site. `conf_46` asserts one of those names,
so editing it is *executing* the authorisation, not extending it. It is recorded here so R65-1 cannot
miss it and R65-0b can check it.

**The contract owner.** ADR-041 (`design/decisions/ADR-041-metrics-surface.md`) is the authoritative
owner of the metric contract, and its text **is** the table of the 21 names (`:256-276`) plus the
omission-arm table that repeats five of them (`:304-312`), plus §3.3 (`:238`), §4 (`:515`) and §5
(`:531`, `:533`). Renaming the series therefore obliges an edit of ADR-041's *names* — content
unchanged, per ADR-047 authorisation §3 ("historical ADR prose is renamed in place; only this ADR is
newly appended"). ADR-041 is not a historical ADR in the R59 sense (it is the live contract), so its
edit is stronger: the contract's own names change because the contract's subject changed.

### 2.2 Surface 2 — the store filename `state/router.db` → `state/vadis.db`

**The pinning assertion.**

| Case | What pins the filename | Authorised by |
|---|---|---|
| `conf_25_config_driven_serve.rs` | the resolved-path assertion at `:199-202`: `state_db.replace('\\', "/").ends_with("state/router.db")` and the message at `:201` | the owner act §1.1 (`state/router.db` named as the surface) |

`crates/vadis-cli/src/config_load.rs:209` is the single production site (`state_db: resolve(&config_dir,
"state/router.db")`); its own unit assertion at `:319` and its doc comment at `:28` follow it.

### 2.3 The fixture-path edits — pinned *and* load-bearing, but not a new decision

`SqliteStore::open` takes a path; the case files that read the store **the `serve` process wrote** build
that path themselves, e.g. `conf_15_prefix_continuity.rs:184` and `conf_30_stream_same_books.rs:107`:

```rust
let store = vadis_store::SqliteStore::open(&dir.join("state/router.db")).unwrap();
```

Once the product resolves `state/vadis.db`, that call no longer opens the product's store: `SqliteStore::open`
either creates an empty database at the old name or fails — either way the case's assertions are reading
the wrong file. **30 case files carry the token; 29 of them carry it as this fixture path** (§3.2 lists
them). Their edits are *mechanical path-following*: the case's meaning ("read what the product wrote") is
unchanged, so **no assertion's semantics move**. The 30th is `conf_25` (§2.2 — a real assertion, at
`:200-201`), and `conf_23_startup_refusals.rs` additionally carries one comment line (`:70`, about the
refusal it stages) above its own fixture reads at `:86, :101, :130`.

---

## 3. The complete site list (the completeness proof for R65-1)

Measured with `rg` at this HEAD (`round/65-deferred-surfaces` @ `37e099a`), excluding the **three** files
`RENAME-MAP.md` §G exempts (`design/RENAME-MAP.md`,
`design/decisions/ADR-047-adopt-the-vadis-name.md`, and this ADR itself — the contract files quote
pre-rename tokens *as the record*), and excluding the nested, git-excluded `autowork/**` tree. **The
re-derive commands below carry the same three exclusions**, so a verifier reproduces these numbers
exactly rather than counting this ADR's own quotations.

### 3.1 The 21 series names — 194 occurrences, 6 files

Re-derive with:

```bash
rg -o '\brouter_(metrics|trace_files|requests|failures|cost|cache|prefix|transform|plan|stateful|overhead_ms_p99)[a-z0-9_]*\b' \
  crates tests docs design book rules README.md AGENTS.md CONTRIBUTING.md SECURITY.md NOTICE \
  Cargo.toml Cargo.lock config.example.yaml providers.example.yaml \
  -g '!design/RENAME-MAP.md' -g '!design/decisions/ADR-047-adopt-the-vadis-name.md' \
  -g '!design/decisions/ADR-048-rename-the-deferred-surfaces.md' -g '!autowork/**' | wc -l
```

*(The last alternative is spelled `overhead_ms_p99`, not `overhead`: the bare prefix also matches
`router_overhead_ms` — the 10 occurrences §9.1 protects — and the same command with `overhead` reads
**204** = 194 + those 10. Use this spelling and the number is 194.)*

| File | occurrences | what is there |
|---|---|---|
| `tests/conformance/tests/conf_87_metrics_single_owner.rs` | **106** | `FROZEN_SERIES` (`:51-73`), `expected_provenance` arms (`:77-92`), and every `value_of`/`at`/`has_series` name through `:1324` |
| `design/decisions/ADR-041-metrics-surface.md` | **30** | the contract's own 21-row table (`:256-276`), §3.3 (`:238`), the omission-arm table (`:304-312`), §4 (`:515`), §5 (`:531`, `:533`) |
| `crates/vadis-cli/src/metrics.rs` | **28** | 27 emitted literals (`:50, 89, 98, 105, 112, 123, 129, 137, 153, 165, 180, 187, 198, 213, 232, 238, 246, 259, 267, 273, 289, 297, 303, 319, 330, 349, 405`) + the doc-comment name at `:25` |
| `docs/spec.md` | **23** | §4.16's 21-row table (`:1649-1669`), the window sentence (`:1637`), the omission sentence (`:1687`) |
| `tests/conformance/tests/conf_46_metrics_is_served.rs` | **4** | `:247`, `:248`, `:250`, `:348` |
| `design/DESIGN.md` | **3** (2 lines) | the `CONF-87` register row (`:1052`, two names) and §12.21's invariant table (`:3956`, one) |

**Distinct names: 21** — not 28. The R59-0b audit already corrected the map's "28" to 21
(`RENAME-MAP.md` §A row 12 / §D / §G, and ADR-047's R59-0b section item 1); this ADR re-derives it a
third time from `metrics.rs`'s literals and `conf_87`'s array, which are identical sets:

```
router_metrics_window_seconds            router_transform_savings_tokens
router_metrics_omitted_figures           router_plan_switches
router_trace_files_read                  router_plan_switch_cost_nano
router_requests                          router_plan_switch_reprefill_tokens
router_requests_succeeded                router_plan_switch_reprefill_cost_nano
router_requests_failed                   router_plan_switches_without_usage
router_failures_by_kind                  router_stateful_inbound_rate
router_requests_usage_missing            router_overhead_ms_p99
router_cost_nano
router_cache_input_cached_tokens         (…counted per name in §3.4)
router_cache_input_tokens
router_cache_hit_rate
router_prefix_continuity_p50
```

Two further tokens live in the **metric namespace** but are **not** series names, and each needs a
decision (§9.2 decides both):

| token | sites | what it is |
|---|---|---|
| `router_uptime_seconds` | `ADR-041:609` (1) | a **hypothetical future series** named inside the `R50-0-N1` note ("a future *add `router_uptime_seconds`* idea must be recorded as a *new* contract"). Not emitted, not asserted. |
| `router_<quantity>[_<unit>]` | `ADR-041:250` (1) | the **pattern** the §3.4 sentence states, in backticks. It is prose *about* the family, not a name. |

### 3.2 The store filename `state/router.db` — 79 occurrences, 44 files

Re-derive with:

```bash
rg -l 'router\.db' crates tests docs design book README.md AGENTS.md \
  -g '!design/RENAME-MAP.md' -g '!design/decisions/ADR-047-adopt-the-vadis-name.md' \
  -g '!design/decisions/ADR-048-rename-the-deferred-surfaces.md' -g '!autowork/**' | wc -l
```

**Production code (5 files, 19 occurrences):**

| File | occ | sites |
|---|---|---|
| `crates/vadis-cli/src/config_load.rs` | 3 | `:28` (doc comment), **`:209`** (the resolve — *the one production site*), `:319` (unit assertion) |
| `crates/vadis-cli/src/reload.rs` | 4 | `:24`, `:1630` (comments), `:1865`, `:1954` (test fixtures) |
| `crates/vadis-store/src/lib.rs` | 9 | `:1018, 1131, 1166, 1240, 1254, 1312, 1385, 1411, 1426` (all test fixtures in the store's own suite) |
| `crates/vadis-core/src/config.rs` | 2 | `:1022`, `:1035` (comments on the known-key note) |
| `crates/vadis-proxy/src/health.rs` | 1 | `:22` (doc comment on `trace_dir`/store) |

**Conformance cases (30 files, 42 occurrences).** One is an assertion (`conf_25:200,201`), one is a
comment (`conf_23:70`), the rest (41 occurrences in 29 files) are fixture-path reads (§2.3):

`conf_15:184` · `conf_20:34,117` · `conf_21:32` · `conf_23:70,86,101,130` · `conf_24:130` ·
`conf_25:200,201` · `conf_29:93,238` · `conf_30:107` · `conf_32:37` · `conf_33:36` ·
`conf_37:38` · `conf_42:183` · `conf_44:49` · `conf_54:206` · `conf_57:159` · `conf_58:191` ·
`conf_59:129` · `conf_64:156` · `conf_66:135,249` · `conf_71:126` · `conf_73:290` · `conf_75:87` ·
`conf_76:91,145,215` · `conf_77:91` · `conf_78:86` · `conf_80:196,221` · `conf_81:116,142,199` ·
`conf_82:189` · `conf_85:583` · `conf_90:209`.

**Documents (9 files, 18 occurrences):**

| File | occ | sites |
|---|---|---|
| `design/DESIGN.md` | 5 | `:180` (store path), `:706` (§12.5 `state` row), `:1582` (Q22), `:1706` (the `state_db` type sketch comment), `:3868` |
| `docs/spec.md` | 2 | `:510` (§4.5's fixed path), `:1373` (§4.12's relative-path paragraph) |
| `design/decisions/ADR-009-…sqlite-wal-store.md` | 2 | `:58`, `:159` |
| `design/decisions/ADR-025-…verbatim-template.md` | 2 | `:135`, `:292` |
| `design/decisions/ADR-029-…gate-quantity.md` | 1 | `:98` |
| `design/decisions/ADR-040-…state-store.md` | 2 | `:205`, `:791` |
| `book/operations.md` | 2 | `:86` (the artifact table), `:123` (the WAL-sidecar backup instruction) |
| `book/connecting-clients.md` | 1 | `:335` |
| `README.md` | 1 | `:152` (the "One local store" bullet) |

**Nothing else.** No tracked `.github/**` site (`rg 'router\.db' .github/` → 0), no `Cargo.toml`, no
`.gitignore` entry (the ignore is the directory `state/`, `.gitignore:10-11`), no `rules/**`, no
`AGENTS.md`, no `CONTRIBUTING.md`/`SECURITY.md`/`NOTICE`.

### 3.3 The `router_*` survivors that are **not** on either surface

The two surfaces above are not the whole `router_*` population. §9.1/§9.2 adjudicate the remainder, so
R65-1 can run **one** `router_*` sweep and still be correct:

| token | occ / files | disposition |
|---|---|---|
| `router_overhead_ms` | **10 / 4** — `ADR-029:46,70,74,93,117`; `ADR-030:6,76`; `ADR-036:221`; `ADR-045:85,353` | **PROTECTED — do not touch** (§9.1): it is the private analysis loop's `result.json` field name (ADR-045:85 quotes the record verbatim) and ADR-029/030's *gate quantity*, i.e. a measurement-register identifier, not a product one |
| `router_uptime_seconds` | 1 / 1 (`ADR-041:609`) | **rename** → `vadis_uptime_seconds` (§9.2) |
| `router_<quantity>[_<unit>]` | 1 / 1 (`ADR-041:250`) | **rename** → `vadis_<quantity>[_<unit>]` (§9.2) |
| `router_auto_suite` / `router-auto-suite/1` | 1 / 1 (`ADR-026:65`) | §C protected — unchanged |

Everything else that contains `router` after R65 is §C or the §E.1 generic-noun prose list (§9.3).

---

## 4. The metric rename, in contract form

### 4.1 The rule

**D1 — every distinct series name in `conf_87`'s `FROZEN_SERIES` is renamed `router_` → `vadis_`,
prefix only; nothing else in the name moves.** The rename is applied to **all 21, as one set**: the
array, the emitted literals and every literal in the two cases must stay *identical to each other*, so a
partial rename is not an option — a case and the code naming different series is the defect `CONF-87`
exists to catch. There is no "rename some" reading of this decision.

| before | after |
|---|---|
| `router_metrics_window_seconds` | `vadis_metrics_window_seconds` |
| `router_metrics_omitted_figures` | `vadis_metrics_omitted_figures` |
| `router_trace_files_read` | `vadis_trace_files_read` |
| `router_requests` | `vadis_requests` |
| `router_requests_succeeded` | `vadis_requests_succeeded` |
| `router_requests_failed` | `vadis_requests_failed` |
| `router_failures_by_kind` | `vadis_failures_by_kind` |
| `router_requests_usage_missing` | `vadis_requests_usage_missing` |
| `router_cost_nano` | `vadis_cost_nano` |
| `router_cache_input_cached_tokens` | `vadis_cache_input_cached_tokens` |
| `router_cache_input_tokens` | `vadis_cache_input_tokens` |
| `router_cache_hit_rate` | `vadis_cache_hit_rate` |
| `router_prefix_continuity_p50` | `vadis_prefix_continuity_p50` |
| `router_transform_savings_tokens` | `vadis_transform_savings_tokens` |
| `router_plan_switches` | `vadis_plan_switches` |
| `router_plan_switch_cost_nano` | `vadis_plan_switch_cost_nano` |
| `router_plan_switch_reprefill_tokens` | `vadis_plan_switch_reprefill_tokens` |
| `router_plan_switch_reprefill_cost_nano` | `vadis_plan_switch_reprefill_cost_nano` |
| `router_plan_switches_without_usage` | `vadis_plan_switches_without_usage` |
| `router_stateful_inbound_rate` | `vadis_stateful_inbound_rate` |
| `router_overhead_ms_p99` | `vadis_overhead_ms_p99` |

### 4.2 The in-band `# vadis:` comments do **not** move

The exposition's omission comments already read `# vadis: …` (metrics.rs `:60, 69, 72-76, 148, 204,
220, 336, 356`; ADR-041's arm table `:306-312`; spec §4.16 `:1687`). They are **already** `vadis` and are
the wording R59's sweep fixed. R65 changes **no** comment text — a rename that "helpfully" touched them
would be a re-edit of frozen wording.

### 4.3 No figure moves

Every series renders the same value before and after: the rename is a *string* change in the formatter
and in two cases. Nothing in `vadis stats`, the derivation, the window, the labels, the omission arms or
`CONF-87`'s equality table changes. `WINDOW_MS` stays `900_000`; the `window_seconds` series still reads
`900`.

---

## 5. The store rename, and the install-migration statement

### 5.1 The rule

**D2 — the fixed store filename is `state/vadis.db`; the directory, the writer, the one-anchor resolution
rule and every store semantic are unchanged.** `crates/vadis-cli/src/config_load.rs:209` becomes
`state_db: resolve(&config_dir, "state/vadis.db")`. Spec §4.5 (`:510`) and §4.12 (`:1373`), DESIGN
`:180/:706/:1582/:1706/:3868`, `book/operations.md:86,123`, `book/connecting-clients.md:335` and
`README.md:152` follow the same leaf.

### 5.2 The migration statement — **there is nothing to migrate**

Stated explicitly because the deferral's own follow-up demanded it (`RENAME-MAP.md` §D: *"a round that
renames the file **and** states the `<old>→<new>` migration"*). The statement is:

> **No install of `vadis` has ever been released, so no state store exists outside a working tree, and
> the migration is empty. A fresh start creates `state/vadis.db`; nothing reads, converts or deletes a
> pre-existing `state/router.db`.**

Grounds, each checkable, so the claim is *measured* rather than assumed:

1. **No release artifact exists.** `git tag` is **empty**; `Cargo.toml:17` reads `version = "0.1.0"`;
   there is **no** CHANGELOG and the only workflow is `.github/workflows/ci.yml` (build/test/clippy/fmt —
   no publish, no release job). Nothing has been distributed under either name.
2. **Nothing has been published on crates.io.** Live check, 2026-10-04:
   `GET https://crates.io/api/v1/crates/vadis` → `404` / `{"errors":[{"detail":"crate \`vadis\` does not
   exist"}]}`. (ADR-047's *Background* records the same for 2026-10-04.)
3. **The population is empty, not merely small.** Any store in existence today was created by an
   in-tree build on the machine that built it, under a `state/` directory that is gitignored
   (`.gitignore:10-11`) and never shipped. The repository went public 2026-10-03; its first commit is
   2026-09-19. There is no operator to migrate.
4. **The one residual action, for a developer holding an unreleased intermediate tree** (the only
   non-empty population this can touch): if a `state/router.db` exists beside your config, the renamed
   product simply does not read it — a fresh `state/vadis.db` is created at next startup
   (`SqliteStore::open` creates it). Delete the old file, or `mv state/router.db state/vadis.db` **and
   its `-wal`/`-shm` sidecars together** (`book/operations.md:123`'s existing warning about copying only
   the `.db` — the same mistake applies to the `mv`). This is *optional*, it is **not an upgrade path**,
   and it exists only because an intermediate unreleased tree existed for a day.
5. **What is *not* claimed.** This ADR does **not** claim the rename is risk-free for a hypothetical
   operator; it claims there is no such operator, and names the population (unreleased in-tree builds)
   that the claim's truth depends on. If a release had existed, this section would have had to specify an
   auto-detect-and-rename on startup, a refusal, or a documented manual step — and the correct choice
   would be an owner's, not a round's.

*If a reader disagrees with this section, the disagreement is with claim 1–3 (that nothing was released),
not with the disposition.* Claims 1–3 are re-runnable in one command each (`git tag`, `git log v0.1.0`,
the crates.io read above).

---

## 6. The effect on external consumers

### 6.1 The metric rename — **no external consumer exists**

`GET /metrics` was added by R50 (ADR-041, 2026-09-27) and is described by spec §4.16, **which landed in
v0.1** — the same unreleased version §5.2 establishes has never shipped. Therefore:

- **No Prometheus scrape config, dashboard, alert or recording rule outside this repository can name
  these series**, because there is no released process that has ever emitted them.
- **The consumer set is closed and in-repo**: `conf_87` (`FROZEN_SERIES` + ~100 name literals) and
  `conf_46` (4 literals). Both are enumerated in §3.1 and both are edited by R65-1 in one change.
- **No operator runbook breaks**: `book/observability-and-accounting.md` names no series (verified:
  `rg 'router_(metrics|requests|cost|cache|plan)' book/` → 0 metric-name hits in `book/`), and the
  shipped example configs carry no metric name.
- **The rename is therefore a pre-release rename, not a breaking change.** It is the *cheapest possible
  moment* to make it: after the first release it would be a breaking change to every scraper.

### 6.2 The store rename — the same, for the same reason

The store file is opened only by the process that owns it (`config_load.rs:209`) and read by an operator
only through the documented `<config dir>/state/…` path (`book/operations.md:86`). With no released
install (§5.2), no external backup script, cron job or monitoring probe can name it.

---

## 7. Decisions

**D1 — the 21 `router_*` series → `vadis_*`** (§4), **in one set**, code + `conf_87` + `conf_46` +
spec §4.16 + DESIGN + ADR-041 together, so the suite is never red in between.

**D2 — `state/router.db` → `state/vadis.db`** (§5), product site + `conf_25` + the 29 fixture-path cases
+ the 9 documents together, with the migration statement of §5.2 landing in `book/operations.md` beside
the existing WAL-sidecar note.

**D3 — the metric namespace's two non-series tokens are resolved, not left dangling** (§9.2):
`router_uptime_seconds` → `vadis_uptime_seconds`; the pattern sentence `router_<quantity>[_<unit>]` →
`vadis_<quantity>[_<unit>]`.

**D4 — the storage rename is a *name*, not a behaviour.** No key, no query, no index, no migration code,
no startup refusal, no `state:` config key (§1.3). The ADR-009 one-writer rule, ADR-040 D5's reload
refusal of `trace.dir`, and the "one anchor" resolution rule are untouched.

**D5 — `RENAME-MAP.md` §D is closed and §G amended in this same change** (§8). The map's Status line
records R65's supersession of §D.

---

## 8. The `RENAME-MAP.md` update (in this same change)

Four edits, landed with this ADR:

1. **Status line** — the map is no longer "frozen for R59" only: it records that **ADR-048 supersedes
   §D for the two deferred entries** and that **§G's expected-survivor list is amended**.
2. **§D** — both rows get a closed marker pointing at ADR-048, and the section keeps the *history* (why
   they were deferred, and how the deferral was discharged) rather than deleting it. The map is a
   contract, and a contract that erases why a rule existed cannot be audited.
3. **§G's expected-survivor list** — after R65 the only legal `router_*` survivors are the §C protected
   set plus `router_overhead_ms` (§9.1). The two §D entries (`router_<metric-name>`, `router.db`) are
   **removed** from the survivor list: their presence there was the deferral, and the deferral is over.
4. **§G's file-exclusion list** — extended from two files to **three**: **this ADR** joins
   `RENAME-MAP.md` and `ADR-047` as a contract file that names pre-rename tokens *as the record*. Without
   this edit, R65-1's own gate run would report this ADR's ~60 quotations as stragglers — the same
   self-reference R59 already had to solve for its two contract files.

The map's own text quotes `router` tokens as history and stays exempt from its own §G gate (that
exemption is unchanged).

---

## 9. What is **not** renamed, and why

### 9.1 The one new protected token: `router_overhead_ms` (PROTECTED)

`router_overhead_ms` is a `router_*` token that is **neither** of the two surfaces. Its 10 occurrences
(§3.3) name:

- **the private analysis loop's `result.json` field**, quoted verbatim at `ADR-045:85` (`the loop's
  result record — tracked; \`router_overhead_ms: {p50 3, p95 16, p99 23, max 24, n 220}\``). Renaming the
  ADR's *quotation* would make it misquote an artifact the repository does not own — the same defect
  class as renaming the `iron/router` evidence URL in ADR-047 (which §G already exempts for exactly this
  reason).
- **ADR-029/ADR-030's *gate quantity*** (`ADR-029:46` *"**`router_overhead_ms`** = `result.overhead_ms`
  − `result.upstream_ms`"*; `ADR-030:6,76`; cited by `ADR-036:221` and `ADR-045:353`). This is the
  measurement register: `AGENTS.md` constraint 9 / ADR-012 put the gate definitions outside the mutable
  scope, just as they put `conf_87` outside it.

Disposition: **add it to the §C protected set** (same justification as `router-auto-suite/1`: it is a
data-format identifier owned by an out-of-scope writer, and renaming it desyncs a document from its
source). It is **not** a deferred surface: nothing about it is waiting on a future round's
authorisation. Registered as `R65-0-F1` (§11) for the owner to overrule if they prefer a repo-side
rename, which would need the loop's records renamed in the same act.

Note the deliberate asymmetry, stated so it is not read as an oversight: the **metric series**
`router_overhead_ms_p99` is renamed (§4.1) while the **quantity** `router_overhead_ms` is not. They are
different objects — the first is this product's `/metrics` series name (ours, unreleased, no consumer);
the second is the measurement's own field label (the loop's, cited). The series is derived from the
quantity, and a rename of the derived name does not oblige a rename of the source name.

### 9.2 The two namespace tokens decided by D3

| token | site | why it moves |
|---|---|---|
| `router_uptime_seconds` | `ADR-041:609` | It is a *future metric name* — a member of the namespace being renamed — quoted inside the `R50-0-N1` note. A future contract for it would read `vadis_uptime_seconds`, so leaving it would put a stale name in the note. No code, no case, no assertion; the rename is one token in one note and changes no meaning. |
| `router_<quantity>[_<unit>]` | `ADR-041:250` | It is the *pattern sentence* that defines the 21 names' form. After D1 the form is `vadis_<quantity>[_<unit>]`; leaving the pattern at `router_` would make the sentence describe a shape no series has. |

### 9.3 The survivors that are deliberately left

R65 changes nothing in the §C protected set (`route`/`routing`/… , `axum::Router`, `health_router`,
`OpenRouter`, `routers`, `router-auto-suite/1`, plus the new `router_overhead_ms`) nor the §E.1
generic-noun prose leave-list (the ~30 prose sites where `router` is the class noun, not the product).
The R59 rename settled those; R65 has no authorisation to reopen them, and §G's gate continues to
accept them.

### 9.4 The sweep is `router_*`, not `router` — recorded so it is visible

The §E.1 leave-list and the protected set together leave a small number of *standalone* `router` words
in prose that a naive `router_*`/`router` sweep could catch (e.g. `ADR-045`'s and `DESIGN.md`'s
measurement-rig prose: *"the router out of the path"*, *"the router's work"*, *"an axum router"*).
Those are §C/§E.1 leaves. **R65-1's sweep is `router_*` → `vadis_*` and the literal `state/router.db` →
`state/vadis.db`; it is not a `router` sweep.** The gate in §10 is written to allow exactly the §C/§E.1
survivors.

---

## 10. The instruction to R65-1 (the executable contract)

Ordered, so the tree is never red and the gate is provable.

1. **One commit, one logical change**, on a branch cut from this ADR's branch (or from `main` once this
   lands). Code + cases + docs together (constraint 8).
2. **The metric rename (D1).** Apply `\brouter_(metrics|trace_files|requests|failures|cost|cache|prefix|transform|plan|stateful|overhead_ms_p99)` →
   `vadis_…` **in `crates/vadis-cli/src/metrics.rs` and the two case files only**, then fix the four
   documents by hand (`docs/spec.md` §4.16's 23 lines, `design/DESIGN.md:1052,3956`,
   `ADR-041`'s 30 occurrences). Do **not** touch `router_overhead_ms` in ADR-029/030/036/045 (§9.1) — a
   sweep written with the bare prefix `overhead` would catch it; write the pattern as §3.1's command does.
   Include `ADR-041:250`'s pattern and `:609`'s hypothetical (D3).
3. **The store rename (D2).** `config_load.rs:209`'s literal, then `:28`/`:319`, `reload.rs`'s 4,
   `vadis-store/src/lib.rs`'s 9, `config.rs:1022,1035`, `health.rs:22`; then `conf_25:200,201` and the
   29 fixture-path cases (§3.2); then the 9 documents (18 occurrences). Land the §5.2 migration sentence
   in `book/operations.md` beside the WAL-sidecar warning.
4. **The four gates**, verbatim: `cargo build --workspace` · `cargo test --workspace` ·
   `cargo clippy --workspace -- -D warnings` · `cargo fmt --all -- --check`. `conf_87` and `conf_46` are
   each their own target in `tests/conformance/` and must be run explicitly.
5. **The completeness gate** (RENAME-MAP §G, amended by §8.3):

   ```bash
   rg -in 'router' \
     crates/ tests/ docs/ design/ book/ rules/ \
     README.md AGENTS.md CONTRIBUTING.md SECURITY.md NOTICE \
     Cargo.toml Cargo.lock config.example.yaml providers.example.yaml \
     -g '!design/RENAME-MAP.md' -g '!design/decisions/ADR-047-adopt-the-vadis-name.md' \
     -g '!design/decisions/ADR-048-rename-the-deferred-surfaces.md' -g '!autowork/**'
   ```

   Expected survivors, **the whole list**: the §C protected set (`axum::Router`, `health_router`,
   `OpenRouter`, `routers`, `router-auto-suite/1`, **`router_overhead_ms`**) and the §E.1 generic-noun
   prose sites. **Any `router_<metric-name>` or `router.db` is now a straggler** — they are no longer
   legal survivors (§8.3). Also re-join line-wrapped tokens (`rg -U`) before trusting the sweep, per
   RENAME-MAP §G.
6. **Do not** add a migration routine, a startup refusal, an alias, a config key, a second metric name
   for compatibility, or a comment explaining the rename. Both surfaces are renamed **hard**, exactly as
   R59 renamed the product name (ADR-047's declined-alternatives row for aliasing applies unchanged).

---

## 11. Consequences, alternatives, reversibility, and the register

| Decision | Alternatives | Gain / sacrifice | Reversible? |
|---|---|---|---|
| **Rename all 21 series, one set** (D1) | rename a subset; keep both names as aliases; bump a metrics-version path (`/metrics/v1`) | Gain: one name across all five surfaces (the property ADR-047 D5 buys) and zero pre-release cost. Sacrifice: none — no consumer exists (§6.1). | **Yes, now** (cheap; pre-release) — **No, after the first release** (every scraper breaks). This is why the round is cheap exactly once. |
| **Rename the store file; state the migration as "nothing to migrate"** (D2/§5.2) | keep `router.db` "just in case"; ship an auto-rename on startup; make the filename a config key | Gain: the `state/` directory reads `vadis` too, and no speculative migration code ships for a population of zero. Sacrifice: a developer's unreleased tree needs a one-line `mv` (§5.2 item 4). | Yes in both directions; the population it can affect is one unreleased tree, so the reversal is symmetric |
| **Edit `conf_87` / `conf_46` / `conf_25` + 29 fixture paths** (§2) | leave the cases; add a compatibility shim; edit only `conf_87`/`conf_25` (the card's enumeration) | Gain: the frozen assertions and the emitted names agree, which is `CONF-87`'s whole subject. Sacrifice: an owner-authorised conformance edit (constraint 9), named per case. | No — a conformance edit is a recorded act; the *content* it protects is unchanged (the single-owner rule and the resolved-path rule are identical) |
| **`router_overhead_ms` stays** (§9.1) | rename it in ADR-029/030/036/045 too | Gain: the repository does not misquote the loop's `result.json`, and the gate register (constraint 9) is not edited by a naming round. Sacrifice: one `router_*` token survives in `design/` prose. | Yes — an owner may overrule (`R65-0-F1`) |

**Register this card opens** (non-blocking):

| id | item | owner | due |
|---|---|---|---|
| `R65-0-F1` | `router_overhead_ms` is added to the §C protected set (§9.1): it names the private analysis loop's `result.json` field and ADR-029/030's gate quantity. A repo-side rename would need the loop's records renamed in the same act. | owner (if they prefer the rename) | open |
| `R65-0-F2` | `conf_46` pins one metric name (4 literals) and was not in the card's site enumeration; R65-1 must edit it or the suite is red (§2.1). Recorded so the R65-0b audit checks the gap was closed, not discovered late. | — (note; closed by §2.1's inclusion) | closed in this ADR |
| `R65-0-N1` | cost: this card is docs-only, offline, `$0.00` — no provider dialled, no credential read; the only network read is the crates.io existence check in §5.2. | — (note) | n/a |
| `R65-0-N2` | after this round the repository's `router`-containing population is: §C protected + §E.1 prose, and nothing else. A future `rg -in router` over the product tree should return only those; a new occurrence is a new decision, not a straggler. | the next rename round, if any | n/a |

**What this ADR does not do.** It does not write the metric rename, the store rename, the case edits or
the doc edits; it does not touch `crates/**`, `tests/**`, `docs/spec.md`, `design/DESIGN.md`, `book/**`
or `README.md`; it changes no price, no key, no gate, no corpus, no threshold and no other case's
assertion. The implementation of everything frozen here is **R65-1**; its audit is **R65-0b**; the
serial gate it must leave green is the four `cargo` commands plus `conf_87`/`conf_46` as their own
targets.
