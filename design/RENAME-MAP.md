# RENAME-MAP — the `router` → `vadis` token rules

- Round: **R59**, branch **`rename/vadis`**, cut from `main` at `ed801e6` (2026-10-04).
- Authority: **ADR-047** (`design/decisions/ADR-047-adopt-the-vadis-name.md`) — the *why* and the four owner
  authorisations live there; this file is the *what* every card executes against.
- Status: **contract, frozen for R59.** A change to this file is a decision (ADR-047), not an edit.
  **Amended by ADR-048 (round R65, 2026-10-04): `§D`'s two deferrals are CLOSED and `§G`'s
  expected-survivor list is narrowed.** The 21 `router_*` metric series names and the store filename
  `state/router.db` are renamed to `vadis_*` / `state/vadis.db` by that round's `R65-1`; after it they
  are **stragglers, not legal survivors**. The deferral's *history* is kept below (§D) because a
  contract that erases why a rule existed cannot be audited.
- One rule governs everything below: **after the rename, no token containing the string `router`
  (any case) may survive in the product tree except the tokens on the two allowlists in §C and §D.**
  The gate that proves it is in §G.

## §0 — How to use this map

0. **Precedence: allowlists beat mechanics.** Before applying any §B rule, match the *whole*
   token (longest match) against §C and §D. If it is on either allowlist it is never touched —
   even though a §B residue rule may also match a prefix of it. This is load-bearing for exactly
   two collisions an unguarded mechanical sweep would commit: **B12 `\brouter-` matches the
   protected `router-auto-suite`** (§C), and **B14 `\brouter\b` matches the deferred
   `router.db`** (§D). *(R65 note: §D is now CLOSED by ADR-048 — `router.db` is renamed, so this
   second collision no longer exists; the precedence rule itself is unchanged and still load-bearing
   for the §C collision.)* Neither may move.
1. Apply **§B (mechanical)** in the order given: longest literal token first, then the three residue
   rules. This order is load-bearing — applying a short rule before a long one can mint a token the
   allowlists no longer recognise (e.g. touching `router_meta` before the `router_` rule).
2. **Never** touch a token on the **§C protected allowlist** (domain vocabulary + third-party types),
   and **never** rename a token on the **§D deferred allowlist** in this round (they are pinned by
   conformance or by an out-of-scope data format).
3. Judgment applies **only to documentation prose** (§E). In code, a token containing `router` is
   either mechanical (§B) or one of the two allowlists — there is no third case.
4. Prove completeness with **§G**, then hand the surviving-token list to the round's verifier.

## §A — Surface inventory (measured with `rg`, this tree, `ed801e6`)

Counts are *occurrences* (`rg --count-matches`). `-F` = fixed string; the token sweep is
`rg -o -i '[A-Za-z0-9_.-]*router[A-Za-z0-9_.-]*'`. `autowork/**` is **excluded** — it is a nested,
git-excluded tree (`.git/info/exclude:13`), not part of the product (`AGENTS.md` "Repo layout").

| # | Surface | Token(s) | Occurrences | Notes |
|---|---|---|---|---|
| 1 | crate dirs, hyphen form | `router-core` 234 · `router-cli` 173 · `router-proxy` 131 · `router-store` 49 · `router-providers` 39 · `router-protocol` 16 · `router-runtime` 30 · `router-plugins` 48 · `router-plugin-sdk` 18 · `router-conformance` 7 | **745** | 10 workspace members (`Cargo.toml:3-14`) |
| 2 | crate idents, underscore form | `router_core` 413 · `router_cli` 143 · `router_conformance` 70 · `router_store` 63 · `router_proxy` 56 · `router_providers` 33 · `router_protocol` 4 · `router_runtime` 2 · `router_plugins` 2 · `router_plugin_sdk` 0 | **786** | `use` paths, `router_conformance::` etc. |
| 3 | types | `RouterConfig` **103** · `RouterError` **2** | 105 | `RouterConfig` spans 17 files |
| 4 | const | `ROUTER_OWNED_TOP_LEVEL_KEYS` **41** | 41 | 11 files |
| 5 | HTTP headers (5 distinct) | `X-Router-Request-Id` 35\|`x-router-request-id` 13 · `X-Router-Transform` 29\|`x-router-transform` 7 · `X-Router-Lossy` 6 · `X-Router-Session` 3 · `X-Router-Failover-From` 1\|`x-router-failover-from` 8 | **102** | code emits 3 (`x-router-request-id`, `x-router-failover-from`, `x-router-transform`); Session/Lossy are doc-only (spec §8, book, DESIGN §12.7 — no code site) |
| 6 | wire field | `router_meta` **104** | 104 | 22 files; the one owned top-level key (`body.rs:29`) |
| 7 | env vars (real) | `ROUTER_TOKEN` 29 · `ROUTER_API_KEY` 7 | 36 | spec §4.7; `config.example.yaml` |
| 8 | env-var-like tokens | `CONF45_ROUTER_TOKEN` 4 · `RAMP_ROUTER_API_KEY` 1 · `ROUTER_SETUP_ADDR` 2 | 7 | test fixtures + one doc-only rejected alternative (ADR-025) |
| 9 | config dir | `.config/router` 21 (of which `~/.config/router` 17) | 21 | spec §4.7/§4.12, book, `config_path.rs` |
| 10 | binary / subcommands | `router setup` 105 · `router replay` 40 · `router serve` 17 · `router trace` 14 · `router version` 1 · `router stats` ~50 | ~230 cmd sites | `[[bin]] name = "router"` (`crates/router-cli/Cargo.toml:11`) |
| 11 | store filename | `router.db` **79** | 79 | **§D deferred** — `state/router.db`, `config_load.rs:209`, pinned by `conf_25` |
| 12 | metric series names | 21 distinct `router_*` names (`router_requests`, `router_cache_hit_rate`, `router_metrics_window_seconds`, …) | ~230 | **§D deferred** — `crates/router-cli/src/metrics.rs`, pinned by `conf_87` (the frozen set is `conf_87`'s `FROZEN_SERIES`, 21 names, identical to `metrics.rs`'s emitted literals; count re-verified by R59-0b) |
| 13 | compound prose/idents | `router-owned` 57 · `router-visible` 7 · `Router-specific` 4 · `Router-owned` 1 · `router-owned-key` 1 | 70 | product-name compounds |
| 14 | test scaffolding idents/strings | `start_router` 15 · `router_binary` 7 · `router_store_now_us` 4 · `router_hint` 4 · `router_field_removal_…` 1 · `router-conf-port-locks` 2 · `router-hermes-tool` 2 · tempdir prefixes `router-store-` `router-cfg-` `router-setup-` `router-reload-watch` `router-reload-` `router-cli-lib-` `router-r60-2-serve-` | ~50 | ours; test-only (see §C note on `health_router`) |
| 15 | struct field | `ResolvedConfig.router` (`config_load.rs:31`), read as `rc.router.*` | ~40 sites | the parsed config document field |
| 16 | repo slug | `flyer103/router` 4 (README badge ×2, `Cargo.toml:20`, SECURITY.md) | 4 | GitHub rename is a human ops action (§F) |
| 17 | standalone word `router` | `\brouter\b` (lowercase) **1 746** · `\bRouter\b` (capital) **101** (6 of which are `axum::Router`) | — | includes the 745 hyphen-crate hits + ~1 001 prose |

### §A.1 standalone-`router` sites by directory (the "~1.7k prose sites", sums to 1 746)

| Directory / file | `\brouter\b` |
|---|---|
| `design/` (incl. ADRs) | **907** |
| `tests/` (incl. `tests/conformance/`) | 187 |
| `crates/router-cli` | 165 |
| `crates/router-core` | 43 |
| `crates/router-proxy` | 25 |
| `crates/router-store` | 20 |
| `crates/router-plugins` | 19 |
| `crates/router-providers` | 6 |
| `crates/router-protocol` | 4 |
| `crates/router-runtime` | 3 |
| `crates/router-plugin-sdk` | 1 |
| `book/` | 149 |
| `docs/` | 94 |
| `rules/` | 14 |
| `Cargo.lock` 29 · `README.md` 31 · `AGENTS.md` 16 · `Cargo.toml` 14 · `CONTRIBUTING.md` 7 · `config.example.yaml` 5 · `providers.example.yaml` 3 · `SECURITY.md` 3 · `NOTICE` 1 | 109 |
| **total** | **1 746** |

## §B — MECHANICAL substitutions (ordered; apply longest token first)

Substitute on a **word boundary at both ends of the whole token**. Order within the table is
longest-first; the three residue rules (B12–B14) run last.

### B1–B2 crates

| Find | Replace | Sites |
|---|---|---|
| `router-plugin-sdk` | `vadis-plugin-sdk` | 18 |
| `router-conformance` | `vadis-conformance` | 7 |
| `router-providers` | `vadis-providers` | 39 |
| `router-protocol` | `vadis-protocol` | 16 |
| `router-runtime` | `vadis-runtime` | 30 |
| `router-plugins` | `vadis-plugins` | 48 |
| `router-core` | `vadis-core` | 234 |
| `router-proxy` | `vadis-proxy` | 131 |
| `router-store` | `vadis-store` | 49 |
| `router-cli` | `vadis-cli` | 173 |
| `router_plugin_sdk` `router_conformance` `router_providers` `router_protocol` `router_runtime` `router_plugins` `router_core` `router_proxy` `router_store` `router_cli` | `vadis_*` (same suffix) | 786 |
| *(Cargo)* `git mv crates/router-<x> crates/vadis-<x>`; each `Cargo.toml` `[package] name`, `[dependencies]`/`[dev-dependencies]` path **keys**, root `Cargo.toml` `members` + `[workspace.dependencies]` keys | | 10 dirs |

### B3–B8 identifiers, consts, wire field, env

| Find | Replace | Sites |
|---|---|---|
| `RouterConfig` | `VadisConfig` | 103 |
| `RouterError` | `VadisError` | 2 |
| `ROUTER_OWNED_TOP_LEVEL_KEYS` | `VADIS_OWNED_TOP_LEVEL_KEYS` | 41 |
| `router_meta` | `vadis_meta` | 104 |
| `ROUTER_TOKEN` | `VADIS_TOKEN` | 29 |
| `ROUTER_API_KEY` | `VADIS_API_KEY` | 7 |
| `CONF45_ROUTER_TOKEN` | `CONF45_VADIS_TOKEN` | 4 |
| `RAMP_ROUTER_API_KEY` | `RAMP_VADIS_API_KEY` | 1 |
| `ROUTER_SETUP_ADDR` | `VADIS_SETUP_ADDR` | 2 (doc-only, ADR-025's rejected alternative) |

### B9–B11 headers, config dir, binary

| Find | Replace | Sites |
|---|---|---|
| `X-Router-Request-Id` / `x-router-request-id` | `X-Vadis-Request-Id` / `x-vadis-request-id` | 48 |
| `X-Router-Failover-From` / `x-router-failover-from` | `X-Vadis-Failover-From` / `x-vadis-failover-from` | 9 |
| `X-Router-Transform` / `x-router-transform` | `X-Vadis-Transform` / `x-vadis-transform` | 36 |
| `X-Router-Session` | `X-Vadis-Session` | 3 |
| `X-Router-Lossy` | `X-Vadis-Lossy` | 6 |
| `.config/router` (and every `${XDG_CONFIG_HOME:-$HOME/.config}/router` spelling) | `.config/vadis` | 21 |
| `[[bin]] name = "router"` | `[[bin]] name = "vadis"` | 1 |
| `model_providers.router` / `model_provider = "router"` (client examples) | `.vadis` / `"vadis"` | 2 |

### B12–B14 residue rules (run last)

| # | Rule | Applies to | Sites |
|---|---|---|---|
| **B12** | `\brouter-` → `vadis-` | hyphen compounds not already covered: `router-owned`→`vadis-owned`, `router-visible`, `router-conf-port-locks`, `router-hermes-tool`, tempdir prefixes `router-store-` `router-cfg-` `router-setup-` `router-reload-watch` `router-reload-` `router-cli-lib-` `router-r60-2-serve-`, `Router-owned`→`Vadis-owned`, `Router-specific`→`Vadis-specific` | ~75 |
| **B13** | `\bRouter\b` (not preceded by `::`) → `Vadis` | PascalCase product name in prose/idents: `Router computes…`→`Vadis computes…`; `Router-specific`. **Excludes** `axum::Router` (protected, §C) | ~95 |
| **B14** | `\brouter\b` (lowercase standalone) → `vadis` | the executable (`./target/release/router`, `router serve|setup|stats|replay|trace`), the struct field (`rc.router`→`rc.vadis`, `config_load.rs:31`), and prose product-name uses | ~1 001 |

**B15 test scaffolding (ours, rename):** `start_router`→`start_vadis`, `router_binary`→`vadis_binary`,
`router_store_now_us`→`vadis_store_now_us`, `router_hint`→`vadis_hint` (an in-repo test decoy — it is
*not* a real owned field; `body.rs:1049`), `router_field_removal_does_not_change_block_hashes`→
`vadis_field_removal_does_not_change_block_hashes`, and the three `conf_10_*` test-fn names
(`conf_10_router_meta_echo_never_reaches_upstream`, …). These are test-local and safe.

## §C — PROTECTED allowlist (NEVER touch)

Applying any rule above to one of these is a **defect**, not a style choice — they are domain
vocabulary or third-party types. The verifier greps for them (§G).

| Token | Why | Real sites (sample) |
|---|---|---|
| `axum::Router` | third-party (axum) type | `crates/router-cli/src/lib.rs:593,609,610,799,824,825` (6) |
| `route` `routes` `routed` `reroute` `routing` | HTTP-routing / mode, the domain noun | spec §2, DESIGN §12, `resolve_route`, … |
| `resolve_route` `route_accounting` `route_*` `*_route` | domain identifiers | `router-core` routing module |
| `failover_from` `plan_switch` | the failover/plan domain vocabulary | `router-core`, ADR-014/024 |
| `health_router` | a local `axum::Router` value (`lib.rs:593`) — contains `router` but names the HTTP router, not the product | 2 code sites (`lib.rs:593,949`) + 1 prose mention (`ADR-041:104`) |
| `OpenRouter` | third-party company/product (a prior-art citation) | `design/decisions/ADR-043-ingress-three-verdicts.md:206` |
| `routers` | generic plural noun ("the two routers actually sent") | `design/decisions/ADR-027-per-arm-plan-and-the-two-comparison-rules.md:173` |
| `router-auto-suite/1` | a **data-format schema id** consumed by the private analysis loop (out of scope); renaming it desyncs a format from its writer | `design/decisions/ADR-026-corpus-tiers-and-automated-scoring.md:65` |
| `router_overhead_ms` | a **data-format field** of the private analysis loop's result record (ADR-045:85 quotes it verbatim) **and** ADR-029/030's *gate quantity* (the measurement register, `AGENTS.md` constraint 9) — not a product identifier and not one of the 21 `/metrics` series; added to this table by **ADR-048 §9.1** | `ADR-029:46,70,74,93,117`; `ADR-030:6,76`; `ADR-036:221`; `ADR-045:85,353` (10 occurrences / 4 files) |

> Note the boundary this table draws: `route`/`routing` do **not** contain the string `router`, so the
> gate in §G does not see them; `axum::Router`, `health_router`, `OpenRouter` and `routers` do, and are
> the four justified survivors of the word-level sweep. `router-auto-suite` contains `router-`, and
> `router_overhead_ms` (added by ADR-048) contains `router_`; each survives only because renaming it is
> a data-format / measurement-register change, not a rename.

## §D — DEFERRED allowlist (NOT renamed in R59) — **CLOSED by ADR-048 (R65)**

These carry the product name but were **out of scope for R59** — each is either pinned by a
conformance assertion (`AGENTS.md` constraint 9: a conformance change is a human decision) or names a
format shared with the out-of-scope loop. They survived the R59 rename **on purpose**; a later round
with its own authorisation owns them — and **that round is R65**, whose contract is
`design/decisions/ADR-048-rename-the-deferred-surfaces.md`. The two rows are kept (with their original
"why deferred" text) because the deferral is part of the record; each now carries its **closure**.

| Token | Where | Why deferred (R59) | Closed by / how |
|---|---|---|---|
| the 21 `router_*` **metric series names** (`router_requests`, `router_requests_succeeded`, `router_cache_hit_rate`, `router_metrics_window_seconds`, `router_overhead_ms_p99`, …) | `crates/vadis-cli/src/metrics.rs`; `tests/conformance/tests/conf_87_metrics_single_owner.rs`; `tests/conformance/tests/conf_46_metrics_is_served.rs`; spec §4.16; `design/DESIGN.md`; ADR-041 | pinned by conformance `conf_87` (and `conf_46`, which asserts one name); not in the owner's R59 authorisation enumeration (ADR-047 §4) | **RENAMED → `vadis_*` (all 21, one set) by ADR-048 D1**, under the owner's act of 2026-10-04 (ADR-048 §1.1). `conf_87` + `conf_46` edited with the code in one change. 194 occurrences / 6 files (ADR-048 §3.1) |
| the store filename `state/router.db` | `crates/vadis-cli/src/config_load.rs:209`; `conf_25_config_driven_serve.rs:200-201`; 29 further case fixtures; `crates/vadis-store/src/lib.rs`; `reload.rs`; `config.rs`; `health.rs`; spec §4.5/§4.12; `design/DESIGN.md`; ADR-009/025/029/040; `book/operations.md`; `book/connecting-clients.md`; `README.md` | pinned by conformance `conf_25`; renaming it forces an **install-migration** decision for existing operators | **RENAMED → `state/vadis.db` by ADR-048 D2**, with the migration statement in ADR-048 §5.2: **there is nothing to migrate** (no `vadis` install has ever been released). 79 occurrences / 44 files (ADR-048 §3.2) |

> **One §D token stayed protected, not renamed, and it is recorded here so the §G gate is exact:**
> `router_overhead_ms` (10 occurrences / 4 files: ADR-029, ADR-030, ADR-036, ADR-045) is a `router_*`
> token that is **not** a series name and **not** a product identifier — it names the private analysis
> loop's `result.json` field (ADR-045:85 quotes the record verbatim) and ADR-029/030's *gate quantity*.
> ADR-048 §9.1 adds it to the **§C protected** set (same justification as `router-auto-suite/1`). It is
> not a deferral: nothing about it awaits a future round's authorisation.

## §E — PROSE rule (documentation only) + 3 worked examples

**Rule.** In documentation text (`README.md`, `book/**`, `docs/spec.md`, `design/**`), a standalone
`router`/`Router` is renamed to `vadis`/`Vadis` when it **names the product or its binary**, and left
unchanged when it is the **generic class noun** (a router in general). This is the only judgment in
the map; it changes no code token.

Worked examples (quoted verbatim; `→` is the result):

1. **RENAME — the binary.** `README.md:39` —
   ``export <the variables it names>        # the wizard prints each name; `router setup --check` verifies them``
   → `…; `vadis setup --check` verifies them`. Here `router` is the executable (§B11/B14).

2. **RENAME — the product name, sentence-initial.** `book/introduction.md:6` —
   ``router is a local-first, multi-protocol LLM gateway.``
   → ``vadis is a local-first, multi-protocol LLM gateway.`` Here it names the product (§B13/B14).

3. **LEAVE — the generic noun.** `design/decisions/ADR-014-plan-first-routing.md:36` —
   ``… and a router that **rejects** on it refuses work on the authority of a number nobody can verify``
   → **unchanged**: "a router" is the generic class noun, not this product (the same sentence would read
   true of any gateway).
   *Note: the card asked for three examples from `README.md`/`book/`; the generic-noun case does not
   occur in either — every standalone `router` in `README.md` and `book/` names the product or its
   binary. The generic case appears only in a handful of ADR/code-comment sentences (`ADR-014:36`,
   `ADR-018:286` "through a router at all"). This example is drawn from there so the judgment is shown
   against a real occurrence.*

**§E.1 — the exhaustive generic-noun leave-list (adjudicated by R59-0b; every other standalone
`router`/`Router` in prose names the product or binary and moves).** A mechanical B14 sweep would
rename these too; an implementer must leave them. Verified against the whole tree at `3f16df8`:

| Site | Text (the `router` in question) |
|---|---|
| `AGENTS.md:53-54` | "A router bound to a local port receives *no connection*" — the macOS-proxy gotcha; generic class noun |
| `book/protocols.md:117` | "adding a field to your client's request does not require a router change" — generic (any gateway tolerates field addition) |
| `design/decisions/ADR-014:36` | "a router that **rejects** on it refuses work…" — the worked example above |
| `design/decisions/ADR-018:286` | "may not be legal to use through a router at all" — generic |
| `design/decisions/ADR-027:173` | "the two routers actually sent" — protected plural (§C `routers`) |
| `design/decisions/ADR-032:27` | "the only one that is a router transform with a landed mechanism" — generic class noun |
| `design/decisions/ADR-045:184` | "a router `p99` that is smaller than the harness floor's spread" — generic (any router under test) |
| `design/DESIGN.md:2501` | "sending an alias upstream would turn a router bug into a provider 400" — generic |

Code-comment sites with the same shape (`crates/router-store/src/trace_sink.rs:54` "a router that
cannot write its own analysis truth", `crates/router-providers/src/stream.rs:66` "not a router
framing", `crates/router-cli/src/setup/mod.rs:2759` "no hash crate is a router-cli dependency",
conf_* comments) are **code, not prose** — §0.3 says code tokens have no third case, and these
standalone words are product references; they move with B14. If R59-1 disagrees on any single row,
that row is escalated to the owner, not silently swept or silently kept.

## §F — Client-contract edits the rename forces (for card **R59-1w**)

`AGENTS.md` constraint 8 (docs before/in the same change). Every path below is a site the rename
touches; the writer changes the **token**, not the sentence around it (except where a sentence quotes a
path/name outright).

**`docs/spec.md`**
- §4.7 `server.auth_token_env` — the shipped example value `ROUTER_TOKEN` → `VADIS_TOKEN`
  (`spec.md:144`, and the §9.1 JSON sample `"env": "ROUTER_TOKEN"` at `:2356`); the prose name
  "of the router process" (`:680`) is the product → `vadis`.
- **Response-header table** — §8 (`spec.md:2199-2200`: `X-Router-Request-Id` / `X-Router-Session` /
  `X-Router-Lossy`), and the §2.1 opt-in header `X-Router-Transform` (`:89, :95, :478`), the refusal
  tables (`:728, :1427, :1445, :1635`), the trace correlation cell (`:2011`), and the error table's
  `invalid_request` row (`:2185`). All five `X-Router-*` spellings.
- **Config-dir path** — `~/.config/router/` → `~/.config/vadis/` at `spec.md:1239, :1376` and the §9.1
  `root_path`/`roster_path` samples (`:2311-2312`).
- §4.5's fixed store path (`spec.md:510`) and §4.12's relative-path paragraph (`:1373`) name
  `state/router.db` — **§D deferred: leave the filename, change nothing** (do not "fix" it here).
  *(R65 note: this instruction is SUPERSEDED by ADR-048 D2 — the filename becomes `state/vadis.db` and
  both spec sentences move in `R65-1`. It is kept because it is what the R59 writer executed.)*

**`book/**`** — the chapters carrying an env var, a header, the config dir, the wire field, the binary
or the product name: `connecting-clients.md` (env var + the codex/hermes blocks + `NO_PROXY`),
`getting-started.md`, `operations.md` (config dir + store path — leave the filename), `protocols.md`
(headers + `router_meta`→`vadis_meta`), `cost-and-caching.md`, `observability-and-accounting.md`,
`plugins.md`, `faq.md`, `introduction.md`, `roadmap.md`. The `router setup|serve|stats` command
examples become `vadis …` throughout.

**`README.md`** — every `router` (product name, `router setup`-family commands, the
`model_providers.router` TOML block, the badge/repo URLs). The `router_meta` mention at `README.md:121`
→ `vadis_meta`.

**Non-`book/` docs and config:** `AGENTS.md`, `CONTRIBUTING.md`, `SECURITY.md`, `NOTICE`,
`config.example.yaml` (`ROUTER_TOKEN` at its `auth_token_env` line + comments),
`providers.example.yaml` (comments), `rules/tool_output.toml` (comments/prose only).

**Repo URL (human ops, not a commit):** `Cargo.toml:20` `repository`, the two `README.md` badge URLs
and `SECURITY.md:8` carry `flyer103/router`. The writer sets `repository` to
`https://github.com/flyer103/vadis` **pre-emptively** (it is a plain string, harmless until the repo is
renamed) but the **GitHub repository rename itself is an owner action** outside this tree — flag it,
do not attempt it.

**Client-contract break R59-1w must document (added by R59-0b, not in the original enumeration):**
after the rename a client that still sends the old key `router_meta` is no longer echo-removed — the
router owns only `vadis_meta`, so the client's own `router_meta` bytes are forwarded upstream
verbatim (mutation (a) no longer covers them). This is a second user-visible break alongside the
config-directory move (ADR-047 D4); the book/`README.md` sections covering `vadis_meta` must say so.

## §G — Completeness gate (the proof)

After the rename, this command must return **only** lines whose `router`-containing token is on §C or
§D; every other line is a straggler:

```bash
rg -in 'router' \
  crates/ tests/ docs/ design/ book/ rules/ \
  README.md AGENTS.md CONTRIBUTING.md SECURITY.md NOTICE \
  Cargo.toml Cargo.lock config.example.yaml providers.example.yaml \
  -g '!design/RENAME-MAP.md' -g '!design/decisions/ADR-047-adopt-the-vadis-name.md' \
  -g '!design/decisions/ADR-048-rename-the-deferred-surfaces.md'
```

The **three** excluded files are the contract itself: `design/RENAME-MAP.md`,
`design/decisions/ADR-047-adopt-the-vadis-name.md`, and (added by R65)
`design/decisions/ADR-048-rename-the-deferred-surfaces.md`. They quote pre-rename tokens (including the
`https://github.com/iron/router` evidence URL and the `router`→`vadis` table) *as history*, and sweeping
them would destroy the record. They are exempt from the sweep and from the gate. All other `design/**`
files — including every historical ADR — ARE swept (owner authorisation §3).

Expected survivor tokens (the only legal ones) — **as of ADR-048 (R65)**, which closed §D:

- **§C protected:** `axum::Router`, `health_router`, `OpenRouter`, `routers`, `router-auto-suite/1`,
  **`router_overhead_ms`** (added to §C by ADR-048 §9.1 — the loop's `result.json` field / ADR-029/030's
  gate quantity)
- **§E.1 generic-noun prose** (the ~30 standalone-`router` sites the §E.1 leave-list enumerates — the
  word-level residual, not a token on either allowlist)

**Nothing else.** Since ADR-048, a bare `router`, a `router_*`/`router-*`/`Router*`/`ROUTER_*` token **and
the two former §D entries** — any `router_<metric-name>` (the 21) and `router.db` — are **all stragglers**:
the deferral is over and their presence in this list was the deferral. (Before R65, the list also carried
`router_<metric-name>` and `router.db`; a reader running §G on an R59-era tree should expect them, and a
reader running it after R65 should treat them as defects.)

The code-only subset (`crates/ tests/`) was the sweep R59-1 ran; after R65 its justified survivors are
exactly `axum::Router` (6) and `health_router` (2 code sites) — and after ADR-048 **not** `router.db` and
**not** the `metrics.rs` `router_*` series (both renamed by R65-1).

**Line-wrapped tokens (added by R59-0b):** a line-based `rg`/sed sweep can miss a token broken
across lines. One exists today: `crates/router-core/src/body.rs:954-955` splits
`ROUTER_OWNED`/`_TOP_LEVEL_KEYS` across a comment line break. Before running §G, re-join wrapped
lines or run the gate over `rg -U` (multiline) output; and after any sweep, `git diff` that site to
confirm both halves moved.

## §H — Deliberate divergences from the R59-0 card body (recorded, not silently chosen)

| The card body says | This map says | Why |
|---|---|---|
| "ADR-026" | **ADR-047** | the register holds ADR-001…046 (`ADR-046`'s own numbering note); the sibling card R59-0b already reads `ADR-047`. Using 026 would collide with the existing corpus-tiers ADR |
| "headers 2 (+casing)" (`X-Router-Request-Id`, `X-Router-Failover-From`) | **5 headers** (add `X-Router-Transform`, `X-Router-Session`, `X-Router-Lossy`) | `rg` finds all five; a rename that leaves three `X-Router-*` headers is half-done |
| "env 2" | **2 real** (`ROUTER_TOKEN`, `ROUTER_API_KEY`) **+ 3 like-tokens** (`CONF45_ROUTER_TOKEN`, `RAMP_ROUTER_API_KEY`, `ROUTER_SETUP_ADDR`) | the extras are a test fixture, a quoted sample, and a doc-only rejected alternative — all listed so no worker re-derives them |
| — (not named) | **§D deferrals**: the `router_*` metric series (pinned by `conf_87`) and `state/router.db` (pinned by `conf_25`) | not in the owner's R59 authorisation enumeration; each touches a conformance assertion (constraint 9). Registered as its own follow-up |
| — (not named) | `ResolvedConfig.router` field, `start_router`, `health_router`, `router-conf-port-locks`, tempdir prefixes, repo slug | found by the exhaustive sweep; each classified so the gate is provable |
| "101 `router setup` sites" | **105** measured | count drifted; the map's numbers are `rg`-measured at `ed801e6` |
