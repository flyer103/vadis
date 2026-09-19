# ADR-002 — the plugin runtime adopts Cordis semantics (revertible effects + reactive coeffects)

- Status: accepted
- Date: 2026-09-19
- Basis: Yifan Shi, Wei Zhang, Tianyi Cui, *A Programming Paradigm for Spatiotemporal Composability*,
  arXiv:2608.25512 (§3 mechanisms, §5.1 the core library, §5.2 the declarative loader and HMR)

## Background

The plugin system must carry "cost/effect experiments" for the long term: plugins are loaded, unloaded and
coexist frequently (A/B, shadow), and they depend on one another. Naive hook-style plugins have two known
defects: **unclean unload** (side effects linger, so an experiment's real impact cannot be judged) and
**dependencies resting on manual ordering / global convention** (adding a plugin means editing someone
else's code). The paper formalizes these two problems as temporal composability (revertible side effects)
and spatial composability (passively declared dependency resolution).

## Decision

Adopt the paper's **context paradigm** as the runtime kernel and take four groups of primitives from it:

| Paper primitive | Usage in this project |
|---|---|
| `ctx.effect(cb) → dispose` (LIFO rollback) | every transform/service registration must carry its own inverse; unloading a plugin = rolling back all of its effects |
| `ctx.set/get(key)` + `notify → refresh` | typed service slots; **when a provider goes offline its dependents are deactivated first**, then the binding is withdrawn |
| `fiber.inject` (a coeffect declaration) | a plugin declares its dependencies; while unsatisfied it stops at load-waiting instead of erroring out of order |
| `ctx.isolate(key, realm)` / `ctx.intercept(key, md)` | multiple realms under one key (two versions of one policy coexisting as a shadow); change the usage only, not the binding (sampling/timeout) |
| entries + keyed diff | the declarative `plugins:` list; per-field minimal operations (config self-diff / disabled unload / rebuild on id change) |

## Rust landing trade-offs (important)

The paper's HMR depends on dynamic module loading. This project:

- **tier-A (link-time) plugins have no module-level HMR** — a code change = rebuild + restart. Only
  **config-level coordination** is done (config / weights / rule TOML take effect immediately).
- **tier-B (out-of-process UDS/WASM) plugins do have module-level reload** — experiment-class and
  model-decision-class plugins are forced to go tier-B.
- To keep the restart cost low: the cache ledger, the sticky table and the trace buffer are persisted by
  the state service and handed over after a restart (the "a restart discards in-process state" problem the
  paper points out in §1.2.3 is resolved in this project by the state service).

## Consequences

- A plugin must implement the `Effect` inverse (on the Rust side a combination of an explicit `undo`
  closure + RAII); review must check "does an unload really return to the pre-load state" and assert it
  with a test (load → unload → state equivalence).
- Inter-plugin dependencies are written in the manifest's `inject`, not hard-coded as an order in code; the
  runtime resolves the load order.
- Realm isolation turns A/B from "two experiments" into "one coexisting comparison", which is the key to
  this project's experiment efficiency.
