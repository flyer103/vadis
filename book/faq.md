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
- **"Endpoint X returns 'not implemented'."** Unimplemented paths are declared, staged gaps:
  the gateway answers them with a specific error code rather than pretending or panicking.
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
- **"Why was my request routed somewhere other than the model I asked for?"** A configured
  fallback chain is used when a route is refused by the guard — including a provider that is
  inside a cooldown after its plan or account was reported exhausted. The trace records the
  origin route and what the switch cost the prefix cache, so the decision is auditable rather
  than unexplained.
- **"How do I know a saving is real, and what do I look at when cost goes up?"** Look at the
  verdict first: only a measured (`verified`) difference may be reported as a saving, local
  estimates are labelled `inferred`. When cost moves, check prefix continuity between turns
  in the same session before anything else, then the per-transform accounting.
- **"Can I configure both the China-mainland and the international endpoint of the same vendor?"**
  Yes — they are two provider entries, because the endpoint and the key belong to the deployment
  rather than to the model. Each entry declares which deployment it is (`region`) and, separately,
  what currency its prices are written in (`currency`); the prices of the mainland entry come from
  that deployment's own official page. Nothing converts between them: see
  [`docs/spec.md` §4.8](../docs/spec.md) and [Currency](cost-and-caching.md#currency-what-a-price-is-denominated-in).
- **"My plan and my pay-per-token account serve the same model but their model ids differ. Can the
  plan-first policy still prefer the plan?"** Yes, with a **family tag**: both model entries declare
  the same `family` string and the policy names that string, so the two routes are one family while
  each keeps the id its own provider expects. Without the tag, the policy would have to assume the
  ids are equal — the default, and the behaviour of every config that predates the tag.
- **"Why does my report show two cost lines and no total?"** Because the window holds requests
  priced in two currencies. Router never adds one currency to another and never converts one into
  another, so it reports each currency separately and states no combined total — a single number
  there would be the one figure it cannot compute honestly
  ([`docs/spec.md` §9.2](../docs/spec.md)).

## Authoritative sources

- [`docs/spec.md` §5](../docs/spec.md) — the local-proxy prerequisite, with the measurement.
- [`docs/spec.md` §1](../docs/spec.md) — the non-goal list (including team use and server-side
  state).
- [`docs/spec.md` §3](../docs/spec.md) — selection semantics and why `auto` is not enabled.
- [`docs/spec.md` §4.0](../docs/spec.md) and [§7](../docs/spec.md) — price provenance and
  the savings convention.
- [`docs/spec.md` §4.8](../docs/spec.md) — regions, currencies and the family tag (why two entries
  of one vendor, why a price is never converted, and why a family may pair two different model ids).
- [`design/decisions/ADR-018-currency-region-and-family-mapping.md`](../design/decisions/ADR-018-currency-region-and-family-mapping.md)
  — why money is never mixed and what each of the three keys is for.
- [`docs/spec.md` §4.4](../docs/spec.md) — rules, tee, and the missing retrieval channel.
- [`docs/spec.md` §8](../docs/spec.md) — the error-type registry and its status codes.
- [`design/decisions/ADR-011-upstream-error-taxonomy-and-recovery-actions.md`](../design/decisions/ADR-011-upstream-error-taxonomy-and-recovery-actions.md)
  — why a dead provider is skipped rather than retried.
- [`AGENTS.md`](../AGENTS.md) — the constraints that produce these answers.
