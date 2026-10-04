# ADR-047 — adopt the name `vadis`

- Status: accepted
- Date: 2026-10-04 (round **R59**'s contract card, `R59-0`; the owner's authorisations are recorded
  verbatim in §*The four owner authorisations* below and were fixed **before** any card of the round
  was cut)
- Kind: **a naming contract, docs-only in this card.** It renames no product byte *here* — `R59-1`
  (code) and `R59-1w` (docs) execute it, `R59-0b` audits the map before `R59-1` is cut. It adds no
  gate, moves no corpus, and changes no conformance assertion *by itself*: the one conformance edit it
  authorises (`router_meta` → `vadis_meta` in 9 cases) is an explicit owner decision below. The
  measurement (gate definitions, corpus, L1 envelope) is not part of this decision's search space
  (`AGENTS.md` constraint 9 / ADR-012), and no saving, latency or cost figure of any kind is minted.
- Related: **ADR-001** (the Rust product, one repo — whose crate namespace this renames);
  **ADR-005** (**trace as interface**) and **ADR-041** (the metrics surface) — the observation
  surfaces this ADR deliberately *does not* rename (the §D deferrals in `design/RENAME-MAP.md`);
  **ADR-007** (span-faithful forwarding) and **ADR-015** (the byte boundary's two mutations) — the
  `router_meta`→`vadis_meta` wire-field change is a rename of the *key the router already owns and
  removes*, not a third mutation; **ADR-009** (the SQLite/WAL store — whose fixed filename is a §D
  deferral); `AGENTS.md` constraints **1** (byte boundary — `vadis_meta` is the same owned key, no
  new byte is touched on the passthrough path), **8** (docs before code) and **9** / ADR-012 (a
  conformance change is a human decision — the reason two surfaces are deferred rather than swept).
- **Numbering note.** The card body asked for `ADR-026`; the register already holds
  `ADR-026-corpus-tiers-and-automated-scoring.md` and runs `ADR-001 … ADR-046` (`ADR-046`'s own
  numbering note), so the next free number is **047**. The sibling card `R59-0b` already reads
  `ADR-047`. `026` would have collided; `047` is this decision's number. The card body's `026` is
  recorded here as a known typo, not silently corrected.

## Background — the measured problem

**The crate namespace is occupied, and that — not aesthetics — is the whole driver.** Every one of the
nine `crates/` packages plus the tenth workspace member (`router-conformance`) and the bare crate name
`router` are **taken on crates.io**. Read from the crates.io API on 2026-10-04, grounded not inherited:

```
GET https://crates.io/api/v1/crates/router   ->  "crate":"router", "max_stable_version":"0.6.0",
    "description":"A router for the Iron framework.", "repository":"https://github.com/iron/router",
    "downloads":1071245, "recent_downloads":47749, "created_at":"2014-12-03"
GET https://crates.io/api/v1/crates/vadis    ->  404 (no such crate)
```

So `router` is a live, 1.07-million-download crate owned by the Iron framework (last published 0.6.0,
2017). This product could never publish `router` — nor `router-core`, `router-cli`, … — under those
names: the namespace is not ours. The name is therefore not a branding choice; it is a **publishing
blocker**. `vadis` is free on crates.io (the API above returns no such crate), and the owner records it
free on GitHub and on `.com` / `.io` / `.dev` / `.ai` / `.rs`.

**What is *not* the driver, said so the ADR stays honest.** No claim is made here that `router` is a bad
name, that `vadis` is a better one, or that a rename buys anything measured. It buys exactly one thing:
a name the product may actually publish under. Every other consequence in this ADR is a *cost* of that,
not a gain.

**Why the change is hard-swept and not aliased.** The product name is not behind one indirection. It is
in the crate roots (1 531 occurrences, hyphen + underscore), the two config types, one const, five HTTP
headers, the wire field `router_meta`, two environment variables, the XDG config directory, the binary
and ~230 subcommand sites, and ~1 746 standalone prose occurrences — `design/RENAME-MAP.md` §A counts
each. A partial rename would leave a product that publishes under `vadis` but speaks `X-Router-*`, writes
`~/.config/router/`, and echo-removes `router_meta` — a *worse* state than either pole. The owner chose
the pole. `RENAME-MAP.md` is the sweep's exact definition so no worker re-derives it.

## The four owner authorisations (recorded, not assumed)

1. **The rename itself is a FULL rename** — the driver above, and the surface set: binary `vadis`;
   `VADIS_TOKEN` / `VADIS_API_KEY`; `X-Vadis-*` headers; `~/.config/vadis/`; wire field `vadis_meta`;
   the ten crate names; `VadisConfig` / `VadisError`; `VADIS_OWNED_TOP_LEVEL_KEYS`.
2. **A HARD `router_meta` → `vadis_meta` change, INCLUDING the 9 conformance files that use it** —
   `conf_01`, `conf_02`, `conf_03`, `conf_10`, `conf_13`, `conf_15`, `conf_27`, `conf_57`, `conf_62`
   (`rg -l router_meta tests/conformance/` = exactly these 9; no more, no fewer). The owner has signed
   this entry into the frozen corpus, which is why it is a human decision under constraint 9 and not a
   `RENAME-MAP.md` liberty.
3. **Historical ADR prose is renamed in place, content unchanged; only this ADR is newly appended.**
   The name-token in earlier ADRs' prose moves; no fact, figure, threshold, `§`/`ADR`/`CONF` id, code
   sample or contract sentence moves. The register stays append-only in *decisions* while its *naming*
   is swept.
4. **Two surfaces are deliberately NOT swept** (owner-scoped, registered in `RENAME-MAP.md` §D): the
   `router_*` **metric series names** (pinned by `conf_87`) and the fixed store filename
   `state/router.db` (pinned by `conf_25`). Each is a separate decision with its own authorisation; this
   ADR records the boundary, it does not cross it.

## Decision

**D1 — The product is named `vadis`; the crate namespace is `vadis-*`.** The ten workspace members
become `vadis-core`, `vadis-protocol`, `vadis-providers`, `vadis-runtime`, `vadis-plugins`,
`vadis-proxy`, `vadis-plugin-sdk`, `vadis-cli`, `vadis-store`, `vadis-conformance`; the Rust crate
identifiers become the underscore forms (`vadis_core`, …). The exact substitution table is
`design/RENAME-MAP.md` §B1–B2. `vadis` is the free name established in *Background*.

**D2 — The binary is `vadis`; every subcommand is `vadis <verb>`.** `[[bin]] name = "vadis"`
(`crates/router-cli/Cargo.toml`); `vadis serve` / `vadis setup` / `vadis stats` / `vadis replay` /
`vadis trace` in code, tests, and every documented invocation.

**D3 — The wire's product-owned tokens become `vadis`-named: five headers and one field.**
`X-Router-Request-Id`, `X-Router-Failover-From`, `X-Router-Transform`, `X-Router-Session`,
`X-Router-Lossy` → `X-Vadis-*` (all casing variants); the owned top-level request key
`router_meta` → `vadis_meta`. **This is a key rename, not a third byte mutation**: `vadis_meta` is the
*same* router-owned key ADR-015's mutation (a) already removes, so the byte boundary (constraint 1) and
the passthrough conformance assertions are unchanged in kind — the client's bytes minus router-owned
keys still hash-equal the upstream-visible prefix.

**D4 — The config surface becomes `vadis`: the XDG directory and the two environment variables.**
`${XDG_CONFIG_HOME:-$HOME/.config}/router/` → `…/vadis/`; `ROUTER_TOKEN` → `VADIS_TOKEN` and
`ROUTER_API_KEY` → `VADIS_API_KEY` (the shipped `config.example.yaml` and `docs/spec.md` §4.7 values).
*Consequence stated plainly:* an existing install's config and state live under `~/.config/router/` and
after the rename are no longer read — the operator must move the directory (or re-run `vadis setup`).
That migration is a *documentation* obligation (`R59-1w`), not an in-tree one; the store filename inside
it is a §D deferral, so `state/router.db` keeps its name under the moved directory.

**D5 — The rename is total over the five surfaces, and it is defined by exclusion.** Every token that
contains the string `router` (any case) is renamed **except** the two allowlists in
`design/RENAME-MAP.md`: §C PROTECTED (domain vocabulary — `route`/`routing`/`reroute`/`resolve_route`/
`route_accounting`/`route_*`/`*_route`/`failover_from`/`plan_switch`; third-party types —
`axum::Router`, `OpenRouter`; and the non-product `health_router`, `routers`,
`router-auto-suite/1` data-format id) and §D DEFERRED (the metric series names, `router.db`). This is
why the map states the rule as a **gate** (`RENAME-MAP.md` §G): "no `router`-containing token survives
except the allowlists" is checkable; "rename the product name" is not.

**D6 — The documentation is renamed in the same change.** Constraint 8: `docs/spec.md` (§4.7 env var,
the response-header table, the config-dir path — enumerated in `RENAME-MAP.md` §F), `book/**`,
`README.md`, `AGENTS.md`, `CONTRIBUTING.md`, `SECURITY.md`, `NOTICE`, and the two example configs move
with the code. The book changes no price and copies no type sketch (constraints 5, 8); it swaps the
token and re-points its links.

**D7 — The repository's own URLs are a *pre-emptive string* here and a human action elsewhere.** The
writer sets `Cargo.toml`'s `repository` to `https://github.com/flyer103/vadis` (a plain string, inert
until published). The **GitHub repository rename**, the badge URLs and the security-advisory URL are an
**owner action outside this tree** — flagged in `RENAME-MAP.md` §F, not attempted by any card.

**D8 — The frozen corpus moves by exactly the authorisation in §2, and nothing else is touched.** The 9
conformance files above are edited for the `router_meta` token only; their assertions are otherwise
byte-identical. `conf_87` (metrics) and `conf_25` (store path) are **not** edited (D5 §D). The gate
verdict for the round records the evaluator commit and the corpus digest (ADR-012) at the close.

## Alternatives considered, and why each was declined

| Alternative | Why it is not the decision |
|---|---|
| **Keep the names; never publish** | The measured defect **is** this alternative: the crate namespace is occupied (1.07M-download `router`), so the product cannot be published under its own names at all. Doing nothing leaves a product that can be built and run but not shipped — the blocker the round exists to clear |
| **Publish under `vadis-*` crate names but keep the wire/config names `router`** | Declined: it produces the worst pole — a `vadis` crate whose CLI is `router`, whose headers are `X-Router-*`, and whose config is `~/.config/router/`. A user reading one surface would be misled by the other, and the splitting rule (which surface keeps which name) would itself have to be decided and defended. One name across all surfaces is the only self-consistent choice |
| **Alias the old tokens (accept `X-Router-*` *and* `X-Vadis-*`, read both env vars)** | Declined: it doubles the surface a worker must keep correct forever, violates the "one fact, one surface" discipline the spec repeats, and for the *wire field* it would mean owning two removable keys — a second thing ADR-015's byte boundary must reason about. The owner chose the hard cut (authorisation §2) |
| **Rename the metric `router_*` series and the store filename in the same sweep** | Declined **in this round**, not in principle: both are pinned by conformance assertions (`conf_87`, `conf_25`) and neither is in the owner's authorisation enumeration (§4). Constraint 9 makes a conformance change a human decision; renaming the store file additionally forces an install-migration decision. Both are registered as §D deferrals with named follow-ups, not swept on a worker's guess |
| **Treat the change as a branding exercise and rename product-facing words freely** | Declined: the driver is the crate namespace (D1), and free prose rewriting is exactly the class of un-owned change this ADR forbids — the map's §E limits prose judgment to a single rule (product name vs generic noun) with three worked examples, so the sweep stays a *rename*, not an edit |

## Consequences

- **The product may publish.** After R59, `cargo publish` has free crate names on crates.io — the
  round's one claim.
- **One name, five surfaces.** Crate, binary, wire, config and prose read `vadis`; a user who learns one
  surface can guess the others correctly. This is the property the hard cut buys and the alias
  alternative would lose.
- **An existing install does not survive the upgrade untouched.** `~/.config/router/` is no longer read
  (D4); the book must say so. This is the round's only user-visible break, and it is documented rather
  than silent.
- **Two surfaces stay `router`-named on purpose** (D5/§D): the metrics series and the store file. A
  reader of `/metrics` still sees `router_requests`; a reader of the store path still sees
  `state/router.db`. That is a known, registered inconsistency, not an oversight — it is the price of
  leaving two conformance-pinned surfaces to their own decisions.
- **`Cargo.lock` and the crate graph are regenerated, not hand-edited.** The build's own artifacts move
  with the `Cargo.toml` keys; the gate is the four `cargo` commands, all green (R59-1).
- **Nothing else moves**: the byte boundary's two mutations, the trace schema, the observation
  boundary, every price, every gate threshold, the dependency allowlist (no new dependency), the rule
  file, and — except for the 9 authorisation files' one token — every conformance assertion.

## What this ADR changes in the existing record

- **`design/RENAME-MAP.md` is created** — the executable contract (map, allowlists, gate, writer list).
- **`docs/spec.md`** — §4.7's env-var value and prose, the response-header table (§8 and the §2.1/§4.x
  sites), and the config-dir path move to `vadis` (enumerated in `RENAME-MAP.md` §F). The §4.5/§4.12
  store-path sentences name `state/router.db` — **unchanged** (§D).
- **`book/**`, `README.md`, `AGENTS.md`, `CONTRIBUTING.md`, `SECURITY.md`, `NOTICE`** — the name token
  and every `router <verb>` / `X-Router-*` / `ROUTER_TOKEN` / `~/.config/router/` / `router_meta`
  occurrence move to `vadis`; prose judgment per `RENAME-MAP.md` §E. Done in `R59-1w`.
- **`crates/**` and `tests/**`** — the code sweep (`R59-1`): crate dirs, `Cargo.toml` keys, every
  `use vadis_*`, `VadisConfig`/`VadisError`, `VADIS_OWNED_TOP_LEVEL_KEYS`, the CLI, env, headers,
  `vadis_meta`, and the 9 authorisation conformance files.
- **Historical ADRs (`ADR-001 … ADR-046`)** — the name token in their prose moves in place (authorisation
  §3); their decisions, figures and ids do not.
- **No ADR index file exists to update** (checked: no ADR list in `README.md`, `CONTRIBUTING.md`,
  `design/DESIGN.md` or `docs/spec.md`); the register is the directory listing, so appending this file
  is the whole change to it.

## Evidence this ADR rests on (re-runnable at the round's base `ed801e6`)

- The namespace driver: the crates.io API responses quoted in *Background* (read 2026-10-04; `router` =
  iron/router, 1 071 245 downloads, `vadis` = 404).
- The surface counts: `design/RENAME-MAP.md` §A — every figure `rg --count-matches` at `ed801e6`, with
  the standalone-word total (1 746) summing exactly over the per-directory table in §A.1.
- The owned wire key: `crates/router-core/src/body.rs:29` — `pub const ROUTER_OWNED_TOP_LEVEL_KEYS:
  &[&str] = &["router_meta"];` (one key; D3's "the same key, renamed").
- The frozen-corpus hit list: `rg -l router_meta tests/conformance/` = `conf_01, conf_02, conf_03,
  conf_10, conf_13, conf_15, conf_27, conf_57, conf_62` (9 files; authorisation §2).
- The two deferral sites: `crates/router-cli/src/metrics.rs` (the `router_*` series) with
  `tests/conformance/tests/conf_87_metrics_single_owner.rs`; `crates/router-cli/src/config_load.rs:209`
  (`resolve(&config_dir, "state/router.db")`) with `conf_25_config_driven_serve.rs:200-201`.
- The protected vocabulary: `axum::Router` (`crates/router-cli/src/lib.rs:593, 609, 610, 799, 824,
  825`), `health_router` (`lib.rs:593`), `OpenRouter`
  (`design/decisions/ADR-043-ingress-three-verdicts.md:206`), `routers`
  (`design/decisions/ADR-027-per-arm-plan-and-the-two-comparison-rules.md:173`),
  `router-auto-suite/1` (`design/decisions/ADR-026-corpus-tiers-and-automated-scoring.md:65`).
- The workspace members: `Cargo.toml:3-14`; the binary: `crates/router-cli/Cargo.toml:11`.

## What this ADR does not decide

- **The metric series names and the store filename** (D5/§D). Each is its own decision, gated on its own
  conformance edit and — for the store file — an install-migration story. Registered, not decided.
- **The GitHub repository rename and its URLs** (D7). An owner action outside this tree.
- **A public announcement, a version bump, or a release.** This is a name, not a release.
- **The internal naming of the domain** (`route`, `routing`, `failover_from`, `plan_switch`). The domain
  keeps its vocabulary; only the *product* name changes.
