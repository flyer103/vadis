# FAQ

Status: outline only. Answers here point at the contract that decides them; where the
question is about a number, the answer is a link, never a number.

## Outline

- **"Nothing reaches the router and my client says 503."** Check the local-proxy
  prerequisite first: `NO_PROXY=127.0.0.1,localhost`. On macOS, clients that read the system
  proxy do not honour its exclusion list for `127.0.0.1`, so the request goes to the proxy
  instead of the gateway.
- **"Why does `model: auto` fail?"** Automatic model selection is intentionally not enabled
  in v0.1; `auto` returns an explicit error and the selection slot is reserved for a plugin.
  Use an explicit `provider/model` or a configured alias.
- **"Endpoint X returns 'not implemented'."** Those paths are staged: the gateway is
  implemented in rounds, and unimplemented endpoints answer with a specific error code
  rather than pretending or panicking.
- **"Where are the prices?"** In the config, one entry per model, each carrying the official
  pricing page URL and capture date. Documentation intentionally never copies price numbers,
  so there is exactly one place to update and one place to audit.
- **"Can I get the original payload back after a rule dropped lines?"** Not in v0.1: rules
  can be marked as teeing, and the trace records the tee identifier, but no storage or
  retrieval channel exists yet, so do not rely on recovering dropped content.
- **"Is my traffic - or my keys - sent anywhere else?"** No. It is a local process; the only
  outbound traffic goes to the providers you configured, authenticated with your own keys.
  Keys themselves live in your environment: the config file references variable names only,
  never the secret values.
- **"Can I run one router for a team, with accounts?"** No: v0.1 is a single-operator local
  gateway; multi-user, multi-tenant and multi-node are explicit non-goals.
- **"How do I know a saving is real, and what do I look at when cost goes up?"** Look at the
  verdict first: only a measured (`verified`) difference may be reported as a saving, local
  estimates are labelled `inferred`. When cost moves, check prefix continuity between turns
  in the same session before anything else, then the per-transform accounting.

## Authoritative sources

- [`docs/spec.md` §5](../docs/spec.md) — the local-proxy prerequisite, with the measurement.
- [`docs/spec.md` §1](../docs/spec.md) — the non-goal list (including team use and server-side
  state).
- [`docs/spec.md` §3](../docs/spec.md) — selection semantics and why `auto` is not enabled.
- [`docs/spec.md` §4.0](../docs/spec.md) and [§7](../docs/spec.md) — price provenance and
  the savings convention.
- [`docs/spec.md` §4.4](../docs/spec.md) — rules, tee, and the missing retrieval channel.
- [`docs/spec.md` §8](../docs/spec.md) — the error-type registry and its status codes.
- [`AGENTS.md`](../AGENTS.md) — the constraints that produce these answers.
