# Plugins

Status: **the contract is decided and the `plugins:` list is now the assembly point** — the launcher mounts
what your list declares, and `inject` is honoured (a plugin whose declared service nothing provides waits in
a named loading state instead of failing). Two keys are still accepted and still do nothing: `isolate` and
`intercept`. The contract is
[`ADR-036`](../design/decisions/ADR-036-minimal-core-and-plugin-surface.md), the where-things-live map
is [`design/DESIGN.md` §13.6](../design/DESIGN.md), and the configuration schema is
[`docs/spec.md` §4](../docs/spec.md). This chapter tells you what a plugin can do for you, what the
gateway does with your `plugins:` list **today**, and what is only specified.

## What plugins are for

Plugins are how the gateway is extended without changing its guarantees: routing policy, cache guards,
payload rules, and measurement. They come in two tiers, and the difference is about blast radius, not
fashion.

- **Tier A — builtins**: compiled into the product, declared in your config, and given the runtime's
  lifecycle hooks (load; reload when their config changes; unload with a full rollback of whatever
  they installed). *(The hooks are specified, not built: today only the start-up load exists — see
  "What is specified but not built yet".)*
- **Tier B — out-of-process**: a separate process reached over a local socket, so a half-finished or
  experimental plugin cannot destabilise the gateway. *(Specified, not built — see below.)*

## What ships today

- **Rules as data.** `builtin/transform_rules` takes its behaviour from a TOML rule file with inline
  tests, so filtering and truncation rules can be reviewed and changed without shipping code —
  [`rules/tool_output.toml`](../rules/tool_output.toml) is a real one. The rule file a plugin entry
  names is read at start-up (`plugins[].config.rules_file`), and a rule that cannot load, compile or
  pass its own inline tests does not apply: the payload travels verbatim and the record says why.
- **Your request decides, not the config file.** A content transform happens only when the request
  itself asks for it (the `X-Router-Transform` header, `docs/spec.md` §2.1). There is no config key
  that enables transforming a request that did not ask, and that absence is what the passthrough
  promise rests on.
- **Three plugins are always resident** with no config entry of their own: the cost ledger, the quota
  guard and the sticky (session) table. They are compiled in and always on.
- **Explicit model selection.** `auto` is an empty slot: explicit selection is the behaviour, and a
  request asking for `auto` is refused with a clear error rather than guessed.

## What is specified but not built yet

Named here so that no reader has to discover it from the config file itself:

- **The plugin runtime, in part.** The `plugins:` list **is** the assembly point now: the launcher mounts
  what your list declares, and `inject` is honoured — a declaration nothing provides leaves that plugin
  waiting in a named state rather than failing. What is still missing: reloading a plugin when its config
  changes, unloading one that is already running and rolling its effects back, realms for side-by-side
  comparison, and sampling/interception. `isolate` and `intercept` are still accepted and still do nothing
  (`ADR-036`, `docs/spec.md` §4.3).
  **`disabled` works, and always did at start-up**: a disabled entry is not loaded (its rule set is off)
  and `/health` shows it as disabled.
- **Tier B.** A plugin in its own process, over a local socket, so that its crash cannot take the
  gateway down.
- **A retrieval channel for `tee`.** A rule may append a fingerprint line naming what it dropped, but
  storing and fetching the original text is not implemented (`docs/spec.md` §4.4).

## What no plugin will ever be able to do

This is the part worth reading, because it is the reason to trust a number this gateway reports:

- **touch your bytes** — the only two byte-level edits the router makes to a request are the two it
  declares: dropping its own top-level fields, and substituting the resolved provider-native model id.
  A plugin may propose an edit to a *tool output payload* and nothing else; your messages, system
  instruction, tool schemas and whitespace are not a plugin's business.
- **write the decision record** — that record is the only channel from the serving path to the
  analysis side, so what a gate can see cannot be shaped by the thing being measured.
- **write state directly** — state is an append-only log with projections rebuilt from it, and a plugin
  that could write it could reorder intent and effect.
- **mislabel a saving** — every figure is either **measured** (a difference read from the upstream's
  own usage report) or **estimated** (a local calculation). Only measured figures count anywhere, and
  no plugin gets to attach that label to a number it did not measure.
- **load other plugins** — the loader is not itself loadable.

Why: each of those is a promise this gateway makes, and a promise that the constrained component owns
is not a promise. The full list, with the constraint behind each one, is `DESIGN` §13.6.

## Tiers in one line each

| | tier A (builtin) | tier B (out-of-process) |
|---|---|---|
| where it runs | in the gateway process | its own process, over a local socket |
| how it is declared | `plugins:` entry with `kind: builtin/…` | `plugins:` entry with `kind: process` and `url:` |
| what a crash costs | the gateway | that plugin |
| available today | the builtins listed above | no |

## Where rule parameters sit

A rule's numeric knobs are the tunable part of the system: the parameter's name and its range are
declared in the config schema (`docs/spec.md` §4.4), and changing a value inside that range is a
configuration change, not a code change.

## What the analysis side may adjust

Only the rule parameters named above, plus the routing and cost policy you configure. The rule shapes,
the cache-regression and byte-fidelity checks, and the conformance suite are not tunable: a change to
any of them is a reviewed change to the product, not an experiment result.

## Authoritative sources

- [`ADR-036`](../design/decisions/ADR-036-minimal-core-and-plugin-surface.md) — the minimal core, the
  plugin surface, and what a plugin may never own.
- [`design/DESIGN.md` §4](../design/DESIGN.md) — the runtime and its Cordis-style semantics
  (`effect`, `coeffect`, `fiber`); §12.2 — the runtime primitives; **§13.6** — the boundary map.
- [`docs/spec.md` §4](../docs/spec.md) — the configuration schema, including the `plugins:` list and
  §4.3/§4.4 (dependency declarations, rule files, and what is not implemented).
- [`rules/tool_output.toml`](../rules/tool_output.toml) — a real rule file with its inline tests.
- [`config.example.yaml`](../config.example.yaml) — how plugins are declared, including a disabled
  tier-B entry.
- [`design/decisions/ADR-002-cordis-plugin-runtime.md`](../design/decisions/ADR-002-cordis-plugin-runtime.md)
  and [`ADR-008`](../design/decisions/ADR-008-rule-override-and-trust.md) — the runtime choice and the
  rule-override policy.
