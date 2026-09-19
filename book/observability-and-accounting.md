# Observability and accounting

Status: outline only. The field-level observation contract is `docs/spec.md` §6 and the
accounting convention is §7; this chapter tells a user what to look at and what the
numbers mean.

Every request produces exactly one decision record, appended to a trace file. The trace is
the only channel between the serving path and everything that analyses it — there is no
hidden second source of truth. The gateway also keeps its own operational state (session
stickiness, cache ledger, quota counters) in a local store, but that store is never read by
the analysis side: the split is defined in [`docs/spec.md` §4.5](../docs/spec.md) and ADR-010.

## Outline

- **One record per request**: identity (request id, client, session, turn), protocol in and
  out, the decision (provider, model, selection source, plugin chain, decision latency).
- **Prefix evidence**: the block-level breakdown of the upstream-visible prefix plus a
  continuity measure against the previous request in the same session. This is the
  fidelity metric: when it drops, some transform is breaking the cache.
- **Per-transform accounting**: for each plugin step, tokens added, tokens saved, the cache
  impact, and the verdict — measured (`verified`) or estimated (`inferred`). Only measured
  values may be reported as savings or used in gates.
- **Result and failure detail**: status, upstream status, failover origin, router overhead
  and upstream latency, plus a failure list that is always present — an absence of failures
  is an empty array, never an omitted field.
- **Where traces go**: one JSONL file per configured rollover interval under the configured
  trace directory; a trace write failure never blocks a request, but it is recorded rather
  than hidden.
- **Reading the report**: `router stats --window ...` for cost, cache hit rate, stateful
  share and per-transform measured savings; `router trace tail` to follow the live stream.
- **Replay is the authority for money**: `router replay` recomputes cost over a fixed trace
  through the same code path, which is why any "we saved X" statement must be reproducible
  from a trace.
- **Reporting rules**: any savings claim states its convention (`verified` or `inferred`),
  its sample size and its time window; mixing conventions is treated as an error.

## Authoritative sources

- [`docs/spec.md` §6](../docs/spec.md) — the observation contract: field groups, the
  definition of prefix blocks and their hashes, and the metric definitions.
- [`docs/spec.md` §7](../docs/spec.md) — accounting: `verified` versus `inferred`.
- [`docs/spec.md` §4.1](../docs/spec.md) — trace output parameters (directory, rollover).
- [`docs/spec.md` §4.5](../docs/spec.md) — the local state store: event log (state truth)
  versus trace (analysis truth), and their join key.
- [`design/DESIGN.md` §9](../design/DESIGN.md) — replay with the same code path.
- [`design/decisions/ADR-005-trace-as-interface.md`](../design/decisions/ADR-005-trace-as-interface.md)
  — why the trace is the sole interface to the iteration loop.
- [`autowork/STATE.md`](../autowork/STATE.md) — the measured facts and the current
  state-of-the-world as of the last round.
