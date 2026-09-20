# Roadmap

Status: outline only. This chapter is rewritten every round — the plan is a snapshot, the
authoritative current state is the file linked below, not this page.

router is built in rounds. Each round has one change scope, its own gates, and a written
record; a round that fails its gates leaves documentation and no broken code.

## Outline

- **Where the project stands right now**: the data plane is real — requests are forwarded to
  upstreams with byte-faithful native passthrough, the streaming path relays the SSE stream
  event byte-for-byte, usage is normalized, upstream failures are classified and can fail over,
  and every terminal outcome lands in the trace. Cross-protocol translation is still to come; the
  reporting surface that reads those traces back out has started landing — `router stats` and
  `/health`'s plan section are served, while `router replay` and `router trace tail` are planned
  and not served ([`docs/spec.md` §9](../docs/spec.md)). See the state document rather than this
  page.
- **The next step**: cross-protocol translation and the trace/observation wiring, which is what
  turns the cache, cost and latency gates from "cannot be judged" into judgeable. Until then, a
  passing round means only the parts that were measurable passed.
- **Decision models: evaluated, not adopted.** A small "structured decision" model — one call returns
  every answer as a typed value with a probability — and a local open-weight alternative were measured
  against each other and against our own hand-written error classifier, on our own task. Verdict:
  **nothing is adopted.** A per-request decision has to fit inside the gateway's own latency budget,
  and the option that answers from the cloud would also send the upstream's error text (which can quote
  what you sent) off the machine. The strongest result is used **offline**: proposing improvements to
  the routing tables that a human merges, and supplying the reference probabilities the semantic-
  corroboration gate is currently missing. The conclusion, and the first step if it is ever adopted,
  are in [`design/decisions/ADR-017`](../design/decisions/ADR-017-decision-model-evaluation-and-shadow-plan.md).
- **The iteration loop**: the direction pool lists the candidate rounds (real-client
  smoke tests, trace capture and replay corpora, input-side compression, output-side
  discipline, quota-aware routing, cache robustness, cost reporting, price verification,
  selection evaluation foundations). One direction per round.
- **How a round is judged**: protocol fidelity, cache fidelity, verified cost and latency
  are blocking; semantic corroboration warns. A round is recorded with its evidence, and
  negative results are kept as documentation.
- **What is explicitly out of scope for v0.1** (and would need a decision to enter):
  server-side session state, semantic response caching and context summarisation,
  multi-user/multi-node deployments, automatic model selection, and retrieval of
  tee'd original payloads.
- **How to influence direction**: bring a hypothesis with a trace, a metric and an expected
  delta; each round is aligned with the maintainer before it is executed.
- **Where to look next**: the state document for current status and the program charter for
  gates, roles and the direction pool.

## Authoritative sources

- [`autowork/STATE.md`](../autowork/STATE.md) — current state, measured facts, decisions and
  the round log.
- [`autowork/program.md`](../autowork/program.md) — mission, gates, roles and the direction
  pool.
- [`autowork/progress/`](../autowork/progress) — one self-contained record per round.
- [`docs/spec.md` §1](../docs/spec.md) — the non-goal list that bounds the roadmap.
- [`design/DESIGN.md` §11](../design/DESIGN.md) — risks and mitigations tracked per round.
