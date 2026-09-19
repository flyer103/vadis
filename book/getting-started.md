# Getting started

Status: outline only. Commands below are the ones the repository already documents; this
chapter does not restate config keys or price tables, it points at their definitions.

Getting from a clone to a first request is four steps: build, copy the example config,
put provider keys in the environment, start the gateway.

## Outline

- **Build** the workspace and run the test suite that guards the protocol contract.
- **Configure**: copy `config.example.yaml` to `config.yaml` and edit the roster —
  providers, models, prices, quotas, aliases. The example file is the single source of
  truth for every price number, and each entry carries a `source` URL plus capture date.
- **Secrets stay in the environment**: the config references environment variable names
  only; API keys are never written into YAML.
- **Start** `router serve --config config.yaml` and confirm liveness on `GET /health`.
- **Point a client** at the gateway base URL — but do the local-proxy prerequisite from
  [Connecting clients](connecting-clients.md) first, or nothing will reach router.
- **First request**: send one request in any of the three inbound protocols and read the
  `router_meta` block in the response (plugin chain, per-transform accounting, session and
  cache state).
- **Know where state lands**: traces are appended under the `trace.dir` configured for
  the run; nothing else is persisted in v0.1.

## Authoritative sources

- [`README.md`](../README.md) — quick start, CLI surface, endpoint table.
- [`config.example.yaml`](../config.example.yaml) — the roster, the price schema and its
  per-model `source` provenance.
- [`docs/spec.md` §4](../docs/spec.md) — the config schema as a contract, §4.1 for the
  trace output parameters.
- [`docs/spec.md` §5](../docs/spec.md) — the client-side prerequisite (also covered in
  the next chapter).
- [`AGENTS.md`](../AGENTS.md) — the build/test commands and the environment gotchas.
