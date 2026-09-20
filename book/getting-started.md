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
  only; API keys are never written into YAML. That covers the inbound token too: the key
  `server.auth_token_env` names a variable, and its value is never in the file
  ([`docs/spec.md` §4.7](../docs/spec.md)).
- **Start** `router serve --config config.yaml` and confirm liveness on `GET /health` —
  which answers without a token even when inbound auth is on.
- **Point a client** at the gateway base URL — but do the local-proxy prerequisite from
  [Connecting clients](connecting-clients.md) first, or nothing will reach router. If you
  turned inbound auth on, that chapter also has the token step.
- **First request**: send one request in any of the three inbound protocols. The `router_meta`
  response block (plugin chain, per-transform accounting, session and cache state) is planned
  design intent, not served in v0.1 — today those same facts are read from the trace record
  instead.
- **Know where state lands**: traces are appended under the `trace.dir` configured for
  the run, and the gateway's own state (session stickiness, cache ledger, quota counters) is
  a local store beside the config file, so the two can be backed up together
  ([`docs/spec.md` §4.5](../docs/spec.md)). Request and response bodies are written to
  neither.

## Authoritative sources

- [`README.md`](../README.md) — quick start, CLI surface, endpoint table.
- [`config.example.yaml`](../config.example.yaml) — the roster, the price schema and its
  per-model `source` provenance.
- [`docs/spec.md` §4](../docs/spec.md) — the config schema as a contract, §4.1 for the
  trace output parameters, §4.7 for the inbound auth key.
- [`docs/spec.md` §5](../docs/spec.md) — the client-side prerequisite (also covered in
  the next chapter).
- [`AGENTS.md`](../AGENTS.md) — the build/test commands and the environment gotchas.
