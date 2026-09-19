# router

A multi-protocol LLM gateway: **a byte-faithful data plane + a revertible plugin runtime + a measurable cost engine**.

For everyday agents (codex / hermes / claude code): point the client's base_url here and router decides
which (provider, model) a request takes, saves tokens **without breaking the upstream prefix cache**, and
records every decision and every cent into a replayable trace.

## Status

| | |
|---|---|
| Phase | **Skeleton ready**: the cargo workspace compiles, `serve` starts (`/health` 200 + the three protocol endpoints as `501` stubs), the five-tier cost/quota/breakeven pure functions and the `RawBody` byte primitive are implemented with unit tests; **forwarding and trace are not implemented yet** (Round 2). See `autowork/STATE.md` |
| Model choice | **Explicit** (provider, model) or an alias; automatic selection is a plugin slot, not enabled in v0.1 |
| Protocols | Inbound and outbound both support OpenAI chat completions / OpenAI responses / Anthropic messages |
| Cost | P0 cache fidelity → P1 input-side payload compression → P2 output-side discipline → P3 provider arbitrage |
| Iteration | the `autowork/` loop side keeps iterating the product (multi-profile + kanban, see `autowork/program.md`) |

## Quick Start

```bash
cp config.example.yaml config.yaml     # edit the roster: provider / model / price / quota
set -a && source .env && set +a        # provider keys are read from env only, never written into yaml
cargo run -p router-cli -- serve --config config.yaml
```

Client onboarding (**must** be done first, or the macOS system proxy intercepts requests going to localhost):

```bash
export NO_PROXY=127.0.0.1,localhost     # both codex (reqwest) and hermes (httpx) read this variable
```

On the codex side:

```toml
[model_providers.router]
base_url = "http://127.0.0.1:8790/v1"
wire_api = "responses"                  # or "chat"
env_key  = "ROUTER_TOKEN"
```

## CLI

```bash
router serve      --config config.yaml          # start the gateway
router stats      --window 24h                  # cost / cache hits / stateful share / measured gain per transform
router replay     --trace traces/x.jsonl --config config.yaml   # offline replay: cost computed on the same real code path
router trace tail                               # follow the decisions and the accounting stream
```

## API

Inbound endpoints (the three are equivalent and mirror the upstream semantics per protocol):

| Method | Path | Protocol |
|---|---|---|
| POST | `/v1/chat/completions` | OpenAI chat completions |
| POST | `/v1/responses` | OpenAI responses |
| POST | `/v1/messages` | Anthropic messages |
| GET | `/health` | liveness + the plugins/services loaded |
| GET | `/metrics` | Prometheus |

The `model` field accepts `provider/model`, an alias from the config, or `auto` (v0.1 returns 400 and says a plugin must take over).
Responses carry `router_meta`: the plugin chain that hit, the accounting of each transform step, and the session and cache state.

## Build & Test

```bash
cargo build --workspace
cargo test  --workspace          # includes the 3×3 protocol matrix in tests/conformance
cargo clippy --workspace -- -D warnings
```

## Layout

```
crates/router-core        domain model, decision pipeline, cost engine, cache ledger, plugin trait
crates/router-protocol    codec for the 3 protocols, translation matrix, usage normalization
crates/router-providers   provider adapters (wire_api capability, auth, retry, SSE)
crates/router-runtime     the Cordis-semantics runtime (effect / coeffect / fiber / declarative loader)
crates/router-plugins     built-in tier-A plugins
crates/router-proxy       data plane (axum), byte-faithful forwarding
crates/router-cli         serve / stats / replay / trace
crates/router-plugin-sdk  out-of-process tier-B plugin protocol
crates/router-store       SQLite/WAL store: event log + projections (ADR-009)
tests/conformance         protocol fidelity, prefix stability, the accounting convention
autowork/                 the iteration loop side (Python orchestration + router replay for policy simulation)
```

## Design documents

- [User book (start here)](book/SUMMARY.md) — user-facing guide: what router is, how to connect a client, the cost levers, how to read the reports.
- [Spec (WHAT)](docs/spec.md) — protocol contracts, config schema, observation and accounting conventions
- [Design (HOW)](design/DESIGN.md) — crate layout, plugin runtime, cost engine, cache policy
- [Decisions (WHY)](design/decisions/) — ADR-001…013
- [Autowork](autowork/program.md) — iteration charter, gates, direction pool

## Ops

- No server-side session state: an inbound `store:true` / a non-empty `previous_response_id` → sticky routing + a trace marker.
- Local state is one SQLite/WAL file (`state/router.db`, spec §4.5, ADR-009) holding the event log and its projections; the trace stays the only analysis channel (ADR-005) and no request or response body is stored.
- Cache is the first-order cost lever: every rewrite must be **content-deterministic** (same content → same upstream bytes).
- Metrics are described by the observation contract in `docs/spec.md`; offline replay is the only authoritative way to compute money.
