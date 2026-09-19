# router

[![CI](https://github.com/flyer103/router/actions/workflows/ci.yml/badge.svg)](https://github.com/flyer103/router/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

A multi-protocol LLM gateway: **a byte-faithful data plane + a revertible plugin runtime + a measurable cost engine**.

For everyday agents (codex / hermes / claude code): point the client's base_url here and router decides
which (provider, model) a request takes, saves tokens **without breaking the upstream prefix cache**, and
records every decision and every cent into a replayable trace.

## Status

| | |
|---|---|
| Phase | **v0.1 data plane landed**: `serve` is config-driven, and the three protocol endpoints forward **natively** to a real provider — byte-faithful (the client's own bytes, minus router-owned top-level keys, with the resolved provider-native `model` id), never parsed and reserialized. The streaming path relays the SSE stream event byte-for-byte. Every terminal outcome — success, upstream 4xx/5xx, connect refusal, timeout, stream — leaves one trace record with measured usage, integer-NanoUsd cost, the quota snapshot and prefix continuity; the local store holds the event log and its projections. **Not implemented in v0.1**: the six cross-protocol translation cells (each answers `501 not_implemented`), `GET /metrics`, the `stats` / `replay` / `trace` reporting subcommands, the `router_meta` response block, tier-B (out-of-process) plugins, and automatic model selection. Current state, round by round: `autowork/STATE.md` |
| Model choice | **Explicit** (provider, model) or an alias; automatic selection is a plugin slot, not enabled in v0.1 |
| Protocols | Inbound and outbound native passthrough for OpenAI chat completions / OpenAI responses / Anthropic messages |
| Cost | P0 cache fidelity → P1 input-side payload compression → P2 output-side discipline → P3 provider arbitrage |
| Iteration | the `autowork/` loop side keeps iterating the product (multi-profile + kanban, see `autowork/program.md`) |

## Requirements

- **Rust**: `rust-toolchain.toml` pins the toolchain the project is verified on, and `rustup` installs it for you. The minimum supported version is lower and is declared as `rust-version` in `Cargo.toml` (`[workspace.package]`) — currently **1.88**.
- **A C compiler**: `rusqlite` is used with its `bundled` feature, so the pinned SQLite C library is compiled into `router-store` at build time — no system SQLite is needed, but a working `cc` is.
- **Provider keys in the environment only**: the config file names the environment variable, never the value.

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
```

`serve` is the only subcommand the binary has today. The reporting surface below is **planned design
intent, not a served command**: `router stats --window 24h` (cost / cache hits / stateful share /
measured gain per transform), `router replay --trace traces/x.jsonl --config config.yaml` (offline
replay: cost computed on the same real code path) and `router trace tail` (follow the decisions and
the accounting stream). Today the trace record is where a decision and its accounting are read from.

## API

Inbound endpoints (the three are equivalent and mirror the upstream semantics per protocol):

| Method | Path | Protocol |
|---|---|---|
| POST | `/v1/chat/completions` | OpenAI chat completions |
| POST | `/v1/responses` | OpenAI responses |
| POST | `/v1/messages` | Anthropic messages |
| GET | `/health` | liveness + the plugins/services loaded |

`GET /metrics` (Prometheus) is planned, not served: it is not registered in v0.1.

The `model` field accepts `provider/model`, an alias from the config, or `auto` (v0.1 returns 400 and says a plugin must take over).
A `router_meta` response block — the plugin chain that hit, the accounting of each transform step, and the session and cache state — is planned design intent: v0.1 does not add it to the response body, and what it would report lives in the trace record instead.

## Build & Test

```bash
cargo build --workspace
cargo test  --workspace          # includes the 3×3 protocol matrix in tests/conformance
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

## Layout

```
crates/router-core        domain model, decision pipeline, cost engine, prefix/cache attribution, quota, trace contract
crates/router-protocol    the 3-protocol codec: usage normalization + the SSE block parser
crates/router-providers   provider adapters (wire_api capability, auth, error classification, SSE relay)
crates/router-runtime     the Cordis-semantics runtime (effect / coeffect / fiber / declarative loader) — scaffold in v0.1
crates/router-plugins     built-in tier-A plugins — scaffold in v0.1, not invoked from the serving path
crates/router-proxy       data plane (axum), byte-faithful forwarding
crates/router-cli         `serve` — the only subcommand in v0.1
crates/router-plugin-sdk  out-of-process tier-B plugin protocol — scaffold in v0.1
crates/router-store       SQLite/WAL store: event log + projections, and the JSONL trace sink (ADR-009)
tests/conformance         protocol fidelity, prefix stability, the accounting convention
autowork/                 the iteration loop side (Python orchestration; replay corpora and policy simulation)
```

## Design documents

- [User book (start here)](book/SUMMARY.md) — user-facing guide: what router is, how to connect a client, the cost levers, how to read the reports.
- [Spec (WHAT)](docs/spec.md) — protocol contracts, config schema, observation and accounting conventions
- [Design (HOW)](design/DESIGN.md) — crate layout, plugin runtime, cost engine, cache policy
- [Decisions (WHY)](design/decisions/) — ADR-001…015
- [Autowork](autowork/program.md) — iteration charter, gates, direction pool

## Ops

- No server-side session state: session identity is the client's own key (`prompt_cache_key`, then the configured headers, spec §4), never `store` / `previous_response_id`. v0.1 does not inspect those two fields — they are forwarded byte-for-byte like every other client field and take no part in routing, so `state.stateful_inbound` in the trace is `false` on every request. Detecting inbound state and marking the trace record is planned, not implemented (known gap G-F).
- Local state is one SQLite/WAL file (`state/router.db`, spec §4.5, ADR-009) holding the event log and its projections; the trace stays the only analysis channel (ADR-005) and no request or response body is stored.
- Cache is the first-order cost lever: every rewrite must be **content-deterministic** (same content → same upstream bytes).
- Metrics are described by the observation contract in `docs/spec.md`; offline replay is the only authoritative way to compute money.

## License

Apache-2.0. See [`LICENSE`](LICENSE); the copyright line is in [`NOTICE`](NOTICE).
