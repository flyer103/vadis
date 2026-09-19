# Plugins

Status: outline only. The plugin runtime is specified in `design/DESIGN.md` §4 and
`docs/spec.md` §4.3/§4.4; this chapter tells a user what plugins can do and what shipped.

Plugins are how the gateway is extended: routing policy, cache guards, payload rules.
They come in two tiers, and the difference is about blast radius, not fashion.

## Outline

- **Tier A — in-process builtins**: compiled into the product, declared in config, and
  given the runtime's lifecycle hooks (load, reload, unload with full rollback of the
  effects they installed).
- **Tier B — out-of-process plugins**: a separate process reached over a socket, so a
  half-finished or experimental plugin cannot destabilise the gateway. A plugin can run in
  an isolated realm to shadow-compare two versions of itself.
- **Dependency declaration**: a plugin declares the service slots it needs; until they are
  satisfied it waits at load rather than failing, and other plugins are unaffected.
- **Rules as data**: a family of transforms takes its behaviour from a TOML rule file with
  inline tests, so filtering/truncation rules can be reviewed and changed without shipping
  code. Overriding is a three-level precedence with first match wins, no merging.
- **Configuration changes are diffed, not restarted**: a config change is handed to the
  plugin for a keyed diff; disabling one detaches just that plugin and rolls back its
  effects.
- **Interception and sampling**: a plugin can be enabled as an observer for a sample of
  traffic, which is how an experiment proves itself before it is allowed to change outbound
  bytes.
- **What v0.1 ships**: the builtins listed in the example config, with automatic model
  selection left as an empty slot — explicit selection is the v0.1 behaviour.

## Authoritative sources

- [`design/DESIGN.md` §4](../design/DESIGN.md) — the plugin runtime and its Cordis-style
  semantics (`effect`, `coeffect`, `fiber`).
- [`design/DESIGN.md` §12.2](../design/DESIGN.md) — the runtime primitives, as sketched.
- [`docs/spec.md` §4.3](../docs/spec.md) — the `inject` (dependency) declaration.
- [`docs/spec.md` §4.4](../docs/spec.md) — rule files, including the explicit statement
  that original-payload retrieval is **not** implemented in v0.1.
- [`rules/tool_output.toml`](../rules/tool_output.toml) — a real rule file with its inline
  tests.
- [`config.example.yaml`](../config.example.yaml) — how plugins are declared, including a
  disabled tier-B entry.
- [`design/decisions/ADR-002-cordis-plugin-runtime.md`](../design/decisions/ADR-002-cordis-plugin-runtime.md)
  and [`ADR-008`](../design/decisions/ADR-008-rule-override-and-trust.md) — the runtime
  choice and the rule-override policy.
