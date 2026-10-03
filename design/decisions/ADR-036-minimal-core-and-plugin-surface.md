# ADR-036 — the minimal core and the plugin surface (what a plugin may never own, and what the type system refuses to let it say)

- Status: accepted
- Date: 2026-09-25
- Related: AGENTS constraints **1** (the byte boundary), **2** (content determinism), **3** (the
  observation boundary), **4** (**no unverified savings**), **5** (**no fabricated prices**), **8**
  (docs before code) and **9** (**the measurement is not part of the search space**); **ADR-002** (the
  Cordis runtime this ADR lands as a contract); **ADR-005** (the decision record), **ADR-009** /
  **ADR-010** (the event log is the truth; intent before effect), **ADR-015** (the byte boundary's
  exactly two span mutations), **ADR-016** + DESIGN **§13** (the primitive register, the module map,
  the leak register, the decision-provider seam, the mode map), **ADR-019** + **§12.12** (the
  transform contract), **ADR-021**/**§12.13** (price tiers are data), **ADR-022** (the wire gate a
  non-native inbound class must pass), **ADR-006**/**ADR-018** (money as integer `Nano`, one
  `currency` per amount — the type trick D4 reuses), **ADR-013** items 1–4 (the shadow/canary rails
  that need P9), **ADR-029** (the latency baseline R41-3 may not move), **ADR-012** (the mutable
  scope); DESIGN **§4**, **§12.1**, **§12.2**, **§12.3**, **§13.1–§13.5**; `docs/spec.md` §2.1, §3,
  §4 (the `plugins:` block and its four keys), **§4.3**, **§4.4**, §6, **§9.3**; `config.example.yaml`
  (`plugins:`); `crates/router-runtime/src/lib.rs`, `crates/router-plugin-sdk/src/lib.rs`,
  `crates/router-cli/src/lib.rs`.
- Numbering note: **ADR-035** landed with R39. This is the next free number in `design/decisions/`.
- Owner decisions this ADR records (2026-09-25): the direction — **strengthen the plugin mechanism,
  define a minimal core, everything else through plugins** (the `deepseek-harness` / Cordis
  *everything-is-a-plugin* shape) — and two of its boundaries as 〈暂时〉 decisions that can be
  re-opened later: **no** `ai-gateway-bench` adoption, **no** protocol translation. The semantic /
  exact-match response cache exclusion in D6 is this ADR's own reasoning, not an owner's word.
- **What this ADR is, and what it is not.** It freezes the **contract**. It writes no code: P9 is
  `contract-only` in §13.1's register (`crates/router-runtime/src/lib.rs:1-4` and
  `crates/router-plugin-sdk/src/lib.rs:1-4` are stubs, verbatim: *"Not yet implemented"*), and
  `inject` / `isolate` / `intercept` are parsed (the `PluginCfg` fields, `router-core/src/config.rs`)
  and validated (the plugin validation loop, `config.rs:1782-1803`) and **nothing acts on them**;
  `disabled` is the exception — it is honoured at start-up (D6.3). Every statement below about the repository is
  cited to a §13.1 row, a leak-register row, or a `CONFn` id; every statement about intent names the
  round that would land it.

## Background

**One of nine designed primitives was never built, and the assembly that would use it is privileged.**
§13.1's register carries nine primitives. Eight are `wired`. The ninth — **P9 `plugin-runtime`** — has
no code home at all, while its contract has been described in §4 (the Cordis primitive → Rust
mapping, the honest HMR trade-off, the lifecycle state machine) and §12.2 (`Ctx`, `ServiceKey<T>`,
`Effect`, `FiberState`, `trait Plugin`, the four-step unload order and the deep-equality assertion
after load → activate → unload) for as long as the register has said `contract-only`. The declarative
list these paragraphs promise already parses — `config.example.yaml:1079`'s `plugins:` block carries
`id` / `kind` / `config` / `inject` / `isolate` / `intercept` / `disabled`, and `docs/spec.md` §4
states the config-change semantics (*a `config` change diffs itself; `disabled: true` unloads the
fiber and rolls back its effects; an `id`/`kind` change rebuilds that entry*).

**Meanwhile the assembly that the list is supposed to drive lives in the CLI, and three plugins are
resident without ever being declared.** §13.1's **P6** row names its wiring as
`router-cli/src/lib.rs` (`plugins[].config.rules_file`) — the launcher walks `plugins[]` itself
(`router-cli/src/lib.rs:269`) to load a rule file — and **P2**'s row names
`router-cli/src/lib.rs:314,344-375`. `config.example.yaml:1079` says the quiet part out loud:
*"Three more built-in plugins are always resident with no config entries of their own:
`builtin/cost_ledger`, `builtin/quota_guard`, `builtin/sticky`."* A plugin list that three plugins do
not appear in is not yet an assembly point.

**Four leak-register rows say which of the plugin-hosted surfaces can honestly be declared today,
and the answer is not the same for each one.** These are the register's own words, and they are the
reason D3's order is forced rather than chosen:

| row | what it says | consequence for a plugin surface |
|---|---|---|
| **L5** (`router-runtime/src/lib.rs:1-4`, `router-plugin-sdk/src/lib.rs:1-4`) | *"primitives whose absence is load-bearing for accepted modes"* — ADR-013's items 1–4 (shadow/canary) compose `isolate`/`intercept` | **`isolate` and `intercept` have no meaning until P9 exists**; M3 `shadow` and M4 `canary` are `contract-only` *because* of this (§13.5) |
| **L6** (`GuardOutcome`, `router-core/src/plan.rs:3-5`) | named as vocabulary and sketched (`DESIGN.md:368-369`) *"but no such type exists"*; the code answers with a plan-specific `PlanMove` (`plan.rs:52-69`), and the caller is a hand-written method with its own outcome struct (`forward.rs:156,1121-1200`) — *"a second rule would invent a second move type, so 'the guard chain' is a paragraph rather than an interface — which is exactly what a decision provider needs"* | a **`Guard` plugin surface cannot be an interface** until this is settled; a plugin protocol over a paragraph would freeze the second move type as the contract |
| **L4** (the reserved `auto` / `Selector` slot) | *"there is no `trait Selector`"*, `decision.selection_source` is a `String` (`trace.rs:88`) *"whose third value is absent from spec §3's own list"*, and the honest options are *"to define the value or delete it — a human decision"* | a **`Selector` plugin surface cannot be declared** before that human decision; declaring it would enshrine a documented-but-unreachable value |
| **L2a / L2b** (P3 re-derived in two transports) | alias/explicit resolution *"including the 404 codes and both message strings"* exists twice (`forward.rs:1270-1306` vs `stream_forward.rs:981-1017`), as does the `supports` capability check and its 400 body (`forward.rs:491-507` vs `stream_forward.rs:332-345`) — *"the two transports can resolve one request differently"* | a plugin-hosted **resolution** would sit on top of a re-derivation; the surface must wait for one implementation, not two |

**What the repo already has that this decision reuses rather than invents.** Plugin provenance is
already in the trace contract: `decision.plugin_chain[]` and `decision.decision_ms` are spec §6
fields, and §13.4's decision-provider seam (DP-1) already specifies landing an advisor's provenance
in *fields that already exist*, with *"no dedicated trace field … and no CONF id allocated here"*.
The four service keys (`CACHE_LEDGER`, `SESSION_TABLE`, `QUOTA_STORE`, `TRACE_SINK`) and the four
plugin-facing traits — `Transform` (§12.3's sketch and §12.12's landing), `Selector`, `Guard`,
`Observer` (§12.3) — are sketched but not implemented — §13.2's module map says so for `router-runtime/` and
`router-plugin-sdk/`, and §13.3's own *not-a-leak* list already declares the shared helpers between
the two forwarding paths intentional, which is why L2a/L2b are the only resolution sites in play.

## Decision

### D1. "Minimal" is a falsifiable sentence, not a taste call

> **The core is minimal when the released binary, started with an empty `plugins:` list, still serves
> the passthrough path — and every capability we ship today is mountable from that list.**

The first half is testable without a new CONFn id: it is the existing conformance suite plus one run
with `plugins: []`, and it is **R41-3's acceptance**. The second half is a claim about the current
tree, checked against §13.1's `wired` rows and against `config.example.yaml:1079`'s three
undeclared residents. Until both halves hold, "everything else is a plugin" is a direction, not a
property — and this ADR may not present it as one (D6).

### D2. The six primitives a plugin may never own, and the constraint each one would break

The core is exactly the set of primitives a plugin cannot own *because* owning one lets a plugin
break an AGENTS hard constraint. This is not a list of important things; it is a list of things whose
absence from a plugin is forced:

| in core | why it cannot be a plugin | where the constraint lives |
|---|---|---|
| **P1 `byte-fidelity`** — the client's bytes and ADR-015's exactly **two** span mutations | the only API that can touch client bytes must be owned by the component that answers for the invariant. A plugin holding it could return any body, and **no test could distinguish that from a legitimate translation** — which is precisely the capability this ADR's owner decision declines to ship | AGENTS 1; ADR-007; ADR-015; §13.1 P1 (`body.rs:29,56,90,197`; `forward.rs:223,528`; `stream_forward.rs:365`) |
| **P2 `inbound-admission`** | it runs **above** the pipeline (one guard, headers only, and a refusal leaves exactly one pre-pipeline record). A component mounted *inside* the pipeline cannot be the thing that admits to it | spec §4.7, §9.1; §12.11; §13.1 P2 (`auth.rs:25,34,49,121`; wiring `router-cli/src/lib.rs:314,344-375`) |
| **P5 `decision-record`** | it is the **only** product → analysis-loop channel (constraint 3). If a plugin could write the record, the observation boundary becomes pluggable, and every gate below it becomes negotiable | ADR-005; spec §6, §7; §13.1 P5 (`trace.rs:22,27,88,311`; writer `accounting.rs:358`; sink `trace_sink.rs:40,73`) |
| **P7 `state-truth`** | intent-before-effect ordering and "projections are never the truth" are **write-path** properties, not features: a plugin that could write state directly could reorder them silently | ADR-009; ADR-010; spec §4.5; §13.1 P7 (`store.rs:26,180,361,412,442`) |
| **P8 `accounting`** — the integer `Nano` amounts **and the `verified`/`inferred` label** | constraint 4 is a **labelling** invariant: gates read `verified` only. Prices and quota data may be plugin-hosted (they are data, ADR-021/§12.13); the **label** may not be, or a plugin could mint a `verified` saving nobody measured | ADR-006; ADR-018; spec §7, §4.0, §4.8; §13.1 P8 (`cost.rs:11,44,55`; `peak.rs`; `quota.rs`; `trace.rs:297`) |
| **P9 `plugin-runtime`** | the thing that loads plugins cannot itself be loaded — and if it could, unload order and failure isolation would be the loaded thing's promises about itself | ADR-002; §4; §12.2; §13.1 P9 (**none**; `contract-only`) |

### D3. The plugin-hosted surfaces, in the order the leak register allows (not the order of ambition)

| order | surface | may propose | what blocks it today |
|---|---|---|---|
| 1 | **`Observer`** (§12.3's trait sketch) | nothing — it receives `&DecisionRecord` and `&RouterError` and returns **no value that reaches the wire** | nothing. This is why R41-4's moat observer (inter-chunk jitter + chunk fidelity) is the first plugin: it proves the surface with a measurement that matters and leaves the byte path untouched |
| 2 | **`Transform`** (§12.3's sketch, §12.12, ADR-019) | a **path-addressed edit plan**, applied as value spans over the client's bytes — *"the parsed view is never what reaches the wire"* | nothing structural: P6's tier-1 engine, mode channel and ledger are `wired` (§13.1 P6, R9-2a/2b); `CONF-16` (R41-0) is its cache-regression case. The remaining absence is the paired `verified` measurement (L5), so every ledger figure stays `inferred` |
| 3 | **`Guard`** (§12.3's sketch) | `Pass` / `Reject{code}` / `Downgrade(route)` — a pure predicate over (route, projections, stable config, one clock read) | **L6**: the answer vocabulary is prose; a protocol over it would freeze `PlanMove` as the interface |
| 4 | **`Selector`** (§12.3's sketch) | a `Decision` over the roster; never sees or produces bytes | **L4** (a human decision: define the `plugin` value or delete the slot) **and** **L2a/L2b** (one resolution implementation, not two) |
| — | provider transport (`router-providers`) | receives a prepared `RawBody`, may add headers, **cannot rewrite the body** | — |
| — | protocol codecs / mappers (`router-protocol`) | per-cell declaration, `lossless \| lossy(reason)`; a missing declaration is a `400`, never a silent re-frame (ADR-022's wire gate) | the owner's 〈暂时不做协议翻译〉, so the translation column stays empty and no document may imply a mapper exists |
| — | price tables / tier config | nothing — **data, not code** (constraint 5: source URL + date in the config comment; ADR-021/§12.13) | — |
| — | the four service-key **implementations** (`cache_ledger`, `session_table`, `quota_store`, `trace_sink`) | a binding behind a key whose **semantics stay** the core's (the ledger's rules, the table's stickiness, the quota arithmetic, the sink's append-only contract) | — |

### D4. The boundary is enforced by types, not by convention

The repo already refuses a class of mistake by making it *unrepresentable* rather than forbidden:
ADR-018's money is integer `Nano` with one `currency` per amount, so cross-currency addition does not
compile. This ADR extends the same trick to the byte boundary and the label:

- a plugin that wants to change content has **no** API other than returning a report over a
  path-addressed plan; `RawBody`'s mutators are private to the core (P1), so the type that could
  return arbitrary bytes is not reachable from a plugin's signature;
- a plugin that wants to price has no path to `Nano` arithmetic except through the accounting
  primitive's own table (P8's data half), so a plugin cannot invent a rate — and cannot attach the
  `verified` label to one it did not measure (P8's label half);
- a plugin that wants a saving counted must set the verified verdict, and `verified` has exactly one
  definition (a measured usage difference, spec §7), so a plugin can only claim what it measured;
- a plugin that wants to see traffic is an `Observer`, whose return type cannot reach the wire.

Corollary, stated so it is not discovered later: **a plugin surface is a type-level promise, so a
surface declared before its types exist is the documented-but-unreachable defect** that spec §9.3
names — which is why D3's order is the leak register's order.

### D5. The surface itself: four keys, one lifecycle, and the four steps that must be asserted

The declared surface is `docs/spec.md` §4's `plugins:` block, and this ADR fixes its meaning rather
than adding to it:

- **`inject: [<service>…]`** (spec §4.3) — a **coeffect** declaration: while unsatisfied the fiber
  stays at `Loading{waiting_on}` (`FiberState`, §12.2), it does not error, and other plugins are
  unaffected. Service names are product-defined typed slots (`cache_ledger`, `session_table`,
  `quota_store`, `trace_sink`), never arbitrary strings.
- **`isolate`** — the same key with multiple realms (§12.2 `Ctx::isolate(key, RealmId)`), so two
  bindings coexist. Its only designed consumer today is ADR-013's shadow/canary rails, which are
  `contract-only` for exactly this reason (L5, §13.5 M3/M4).
- **`intercept`** — does **not** change the binding, only how it is used (sample rate, timeout,
  shadow switch). It rebinds nothing; a test that finds the intercept table changed is a defect.
- **`disabled`** — unload that fiber and fully roll back its effects; `id`/`kind` change → rebuild
  that entry; a `config` change → the plugin diffs it itself (§4's config-change semantics).
- **Lifecycle**: `Loading → Active → Unloading → Removed`, `Failed(err)` carrying the error and
  **not affecting other fibers** (ADR-002's failure isolation).
- **Unload order** (§12.2's four-step paragraph), asserted by a test, and the round that lands it is R41-2:
  ① recursively move dependents into `Unloading` and wait → ② run this fiber's `Effect::undo` in
  reverse LIFO → ③ withdraw the service bindings → ④ `Removed`. After load → activate → unload the
  service table **and** the intercept table must be **deep-equal** to their pre-load state.

### D6. What must carry an honest label until it is built

Because this ADR is docs-only and the runtime is 0% implemented, four labels are load-bearing and
none of them moves early:

1. **§13.1's P9 row stays `contract-only`** — this round changes the row's *contract home* (it gains
   this ADR) and never its *state*.
2. **DESIGN §4 and §12.2 are labelled as a frozen contract, not as landed** — §4's title keeps
   *"aligned with Cordis semantics"*; the round adds a status line naming this ADR as the contract
   and R41-2 as the implementation. (The round's own plan of record had proposed relabelling §4 as
   *landed*; that is refused here for the §9.3 reason above.)
3. **The three contract-shaping keys — `inject`, `isolate`, `intercept` — are declared, parsed and
   validated, and nothing acts on them.** **`disabled` is not in that set**: it is honoured today, at
   start-up (a disabled entry's rule set is not loaded — `router-cli/src/lib.rs:270` — and `/health`
   reports the entry as disabled; spec §4.4), and what stays unimplemented is only its *designed
   runtime* semantics (unload a live fiber and roll back its effects). `docs/spec.md` §4.3 and
   `book/plugins.md` say this in the same words, and `book/plugins.md` (45 lines before this round,
   self-described *"outline only"*) is the user-facing half of this ADR so that the update precedes
   R41-2's code (AGENTS 8).
4. **No saving figure of any kind may be reported from this surface**, and no comparative speed
   claim may exist anywhere: the third-party harness that would give a same-basis number was declined
   (owner, 2026-09-25), so the only honest answer to *"is this faster than X"* is **"there is no
   comparable measurement"** — not a re-quoted vendor figure.

### D7. The honest boundary against `deepseek-harness`

dsh is TypeScript on Cordis and states that *"every part of the product is a plugin … there is no
privileged core to patch"*, with dynamic module loading, HMR, and profiles/bundles/patch layers. In
Rust, tier-A plugins are **linked at compile time**, so:

- there is **no module-level HMR**: a code change is a rebuild and a restart. What stays genuinely
  dynamic is **config-level coordination** (a keyed diff: weights, rule TOML, `disabled`) and
  **tier-B out-of-process** plugins over UDS (`router-plugin-sdk`, also `contract-only` today);
- the contract keeps the half that matters operationally (all of it **specified, none of it built** yet): **declarative composition at boot**
  (`plugins:` as the single assembly point, `inject` satisfied-or-waiting, realms for A/B,
  intercept for sample/timeout/shadow), **per-plugin rollback** (every registration carries its
  inverse, LIFO), **failure isolation**, and **dependency-ordered unload**;
- the restart cost is already mitigated by design, which is why the missing HMR is survivable here:
  the cache ledger, the sticky table and the quota counters are **projections of the local event
  log** (ADR-009/ADR-010) and are rebuilt from it, so a restart loses no session context;
- vocabulary map, so the comparison is not read as parity: dsh bundles / profile layers ≈ our
  `plugins:` list plus per-entry `config`; dsh's patch file ≈ our keyed config diff; dsh's plugin
  manager ≈ our loader (P9). There is **no** equivalent of npm-installed out-of-tree plugins here
  until tier-B (R41-5) lands.

### D8. Round order, and the gate that still binds

The order is forced by docs-before-code and by D3's blockers, and it is recorded in the round table
this ADR's round carries:

**R41-1** this contract (docs only) → **R41-2** implement P9 (§12.2's primitives, the loader, realms,
intercept; acceptance: the unload order asserted, the deep-equality assertion, a `Failed` fiber
leaving others `Active`, an unsatisfied `inject` staying `Loading{waiting_on}` with no error, and no
realm leakage into `ROOT_REALM`) → **R41-3** the assembly leaves the CLI (first migration:
`transform-chain`; acceptance **is** D1's falsifiable sentence) → **R41-4** the first two plugins that
prove the surface (`Observer` moat measurement; the reproducibility sink for `R33-4-F1`) → **R41-5**
tier-B → **R41-6** `book/`'s authoring chapter and the positioning statement.

**The latency gate still binds, and it is not waived by architecture.** ADR-029's baseline binds the
`router_overhead_ms` p99 band; §13.1's register and R32's ladder exist because a plugin runtime can
break the hot path quietly — a `ServiceKey` lookup per request, an `Arc` clone per hop, a lock on the
service table are each invisible in unit tests and visible in the p99. R41-3 therefore re-runs the
ladder as an acceptance condition, and **a plugin runtime that adds per-request cost is a regression
regardless of its architecture**.

## Alternatives considered

- **A. Leave the assembly in the CLI and grow it.** Rejected: the declarative list, its keys,
  the config-diff semantics and the no-rebuild promise are already **published** in
  `config.example.yaml:1079` and `docs/spec.md` §4, and §13.1 says the parse of the three
  contract-shaping keys is consumed by nobody.
  Continuing to grow a privileged assembly beside a published declarative one is exactly the
  documented-but-unreachable class (§9.3) that this repository treats as a defect — and L4 is the
  precedent for how it is handled: register it, or fix it.
- **B. No core notion — "the byte boundary is a plugin too" (dsh parity in the strong sense).**
  Rejected: if P1 were plugin-hosted, a plugin could return any body and no test could distinguish
  it from a legitimate translation; if P5 were, the observation boundary would be pluggable; if P8's
  label were, a plugin could mint an unmeasured `verified`. Each of those is a hard constraint, and
  a constraint that the constrained component owns is not a constraint.
- **C. Dynamic module loading / HMR.** Rejected as unavailable rather than undesirable: tier-A links
  at compile time (ADR-002, §4), and the restart cost is already mitigated by ADR-009/010's
  projections. Claiming HMR parity would require a different language and would give up the
  type-level guarantees D4 rests on.
- **D. Declare all four surfaces at once** (so the contract is "complete"). Rejected by D3: `Guard`
  would freeze `PlanMove` (L6) and `Selector` would enshrine `selection_source`'s absent value (L4).
  A surface declared over an open leak is a promise the repo cannot keep, and the leak register
  exists to make that visible before it is promised.
- **E. Migrate P3/P4 to plugins first** (they are the most "policy-like", so they look most
  plugin-ready). Rejected: they are precisely the two surfaces the register shows as re-derived
  (L2a/L2b) and unresolved (L4/L6). Order of ambition is not order of admissibility.

## Consequences

**What this ADR buys.** (i) A one-sentence, testable definition of "minimal core" (D1) that R41-3
must satisfy rather than assert. (ii) A boundary derived from the constraints instead of from taste:
every in-core primitive in D2 is there because a plugin owning it would break an AGENTS constraint,
and each row cites the constraint. (iii) An *ordered* plugin surface (D3) whose order is the leak
register's, which means the next rounds do not re-litigate "why can't we do `Selector` first" — L4
answers it. (iv) Plugin provenance needs **no new trace field**: `decision.plugin_chain[]` and
`decision_ms` already exist in spec §6, and §13.4's DP-1 already says provenance lands in existing
fields with no dedicated field and no new CONF id.

**What it costs.** (i) The contract is frozen while its implementation is 0% — so the honest labels
of D6 are load-bearing until R41-2 lands, and a reader who skips them will over-read §4 and §12.2.
(ii) No HMR, permanently, for tier-A. (iii) Tier-B (R41-5) becomes a second product surface —
framing, sandboxing, versioning, and a plugin that can crash — which is why it is sequenced last.
(iv) §13's own drift rule ("one implementation per invariant") means the register is updated **in the
same round** that moves a site; R41-3 may not leave a second resolution implementation behind, which
narrows R41-3's freedom: P3's migration is not in R41-3.

**What is now tracked rather than implicit.** L4, L5, L6 and L2a/L2b stop being idle register rows:
each is named in D3 as the blocker of a specific surface, so a later round that wants that surface
must cite the row and get it settled first.

## What this ADR does not decide

- **L4's settlement** — define the `plugin` value of `selection_source` (spec §6 already lists it) or
  delete the reserved `auto` slot. A **human decision**; it is the precondition of the `Selector`
  surface (D3), and no round of this chain may settle it as a side effect.
- **L6's vocabulary** — a `GuardOutcome` type, or the deletion of the prose. Becomes real when a
  second guard rule lands.
- **Whether the moat observable becomes a blocking gate** (inter-chunk jitter, chunk fidelity) — that
  is a measurement definition, outside the loop's mutable scope (AGENTS 9, ADR-012).
- **The metric register** (the R42 plan) and **the provider split** (the R43 plan) — separate
  decisions, separate rounds.
- **Tier-B's security model** — sandboxing, permissions and versioning for an out-of-process plugin.
- **Semantic / exact-match response caching.** Excluded by this ADR's reasoning, not by an owner's
  word: a hit removes the upstream call, so no `usage` object exists and the saving can only ever be
  `inferred` — it would be the first shipped feature whose headline benefit cannot enter a gate
  (AGENTS 4), and it breaks the 1 client request = 1 upstream call correspondence the accounting rests
  on. If it is ever built it must be a plugin, **off by default, disclosed in the trace, and excluded
  from every gate**; the decision is *"is parity on a feature chart worth diluting the one axis where
  we are alone"*, not *"is it hard"*.
- **the loop state record rows 14–16** (R40's registrations) and the standing owner rows carried there.

## References

- `design/DESIGN.md` §4, §12.1, §12.2, §12.3, §12.12, §12.13, **§13.1–§13.6**, §13.4
- `docs/spec.md` §2.1, §3, §4, §4.3, §4.4, §4.10, §6, §7, §9.3
- `config.example.yaml` (`plugins:`), `rules/tool_output.toml`
- `crates/router-runtime/src/lib.rs`, `crates/router-plugin-sdk/src/lib.rs`,
  `crates/router-cli/src/lib.rs`, `crates/router-core/src/config.rs`
- ADR-002 (the runtime), ADR-005/009/010 (the record, the truth, the write path), ADR-006/018
  (money), ADR-013 (the rails that need P9), ADR-015/019 (the byte and transform contracts),
  ADR-016 (the register and the seam), ADR-021 (price tiers as data), ADR-022/023 (the wire gate and
  the candidate walk), ADR-029 (the latency baseline), ADR-012 (the mutable scope)
- `CONF-15`, `CONF-16` (§12.8's cache rows, the second landed by R41-0), `CONF-60`…`CONF-63`
  (the transform chain's cases)

## Publication note (2026-10-03, R62-2)

The `R<n>` labels and finding ids in this document name iterations of the project's own
private analysis loop — a loop that is not part of this repository, so no label here is
resolvable by a reader of it; they are kept as the provenance of the decision. This
publication pass removed only the dead-pointer class: every reference into that loop's
working tree (its file paths and round-record names, its state record, charter, execution
model and replay contract, its scripts and module names, and the kanban card ids), each
replaced by the neutral phrase the sentence needs. Nothing else moved — no figure,
threshold, `§`/`ADR`/`CONF` id, code sample or contract sentence.
