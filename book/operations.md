# Operations

Status: outline only. Behaviour under failure is normative in `docs/spec.md` §8; this
chapter is the day-2 view for whoever runs the gateway.

router is a single local process for a single operator. Operating it is mostly about
knowing what it deliberately does not persist, and what it does when an upstream misbehaves.

## Outline

- **Run it**: `serve` with a config file; liveness via the health endpoint; all logs and
  accounting go to the trace files you configured.
- **Trace lifecycle**: files append and roll over; v0.1 has no automatic retention or
  cleanup, so archiving is a manual operations job and the path is configurable so state can
  live outside the repository.
- **No server-side session state**: the gateway does not store conversations. Requests
  arriving with server-side state are routed stickily and tagged in the trace so the
  assumption is continuously auditable rather than assumed.
- **Failover**: a configured ordered chain of routes is tried when an upstream errors,
  rate-limits or exhausts its quota; switching routes loses the prefix cache, and the
  re-prefill cost plus the origin route are recorded. An exhausted chain is a clean gateway
  error, not a hang.
- **Degradation rules**: a failed transform falls back to the original payload and the
  request is still served; a prefix discontinuity warns by default and can be made
  rejecting; unknown fields pass through.
- **Config changes**: editable config is re-read as a diff — reload do not restart; nothing
  in the serving path depends on wall-clock time or turn order, so restarts do not change
  what a request looks like upstream.
- **Upgrades and rollback**: one logical change per commit on a round branch, merged only
  when the round's gates pass; a round that fails leaves documentation and no broken code.
- **Troubleshooting entry points**: client sees 503 with nothing in the logs → check the
  proxy prerequisite first; endpoint returns "not implemented" → that path is staged for a
  later round; costs moved → compare prefix continuity between turns.

## Authoritative sources

- [`docs/spec.md` §8](../docs/spec.md) — unified error body, error-type-to-HTTP table,
  response headers and degradation behaviour.
- [`docs/spec.md` §4.2](../docs/spec.md) — the failover chain as configured.
- [`design/DESIGN.md` §8](../design/DESIGN.md) — state and persistence boundaries.
- [`design/DESIGN.md` §11](../design/DESIGN.md) — risks and mitigations.
- [`AGENTS.md`](../AGENTS.md) — build/test commands, version-control rules, environment
  gotchas.
- [`autowork/program.md`](../autowork/program.md) — the round gates that gate a merge.
