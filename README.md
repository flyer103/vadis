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
| Phase | **v0.1 data plane landed**: `serve` is config-driven, and the three protocol endpoints forward **natively** to a real provider — byte-faithful (the client's own bytes, minus router-owned top-level keys, with the resolved provider-native `model` id), never parsed and reserialized. The streaming path relays the SSE stream event byte-for-byte. Every terminal outcome — success, upstream 4xx/5xx, connect refusal, timeout, stream — leaves one trace record with measured usage, integer-NanoUsd cost, the quota snapshot and prefix continuity; the local store holds the event log and its projections. `router stats` reads those traces back out (cost, cache, the plan family's switches and their verified cost) and `/health` reports what the process loaded, including a configured plan family's account and probe deadline. **Not implemented in v0.1**: the six cross-protocol translation cells (each answers `501 not_implemented`), `GET /metrics`, the `replay` / `trace tail` subcommands, the `router_meta` response block, tier-B (out-of-process) plugins, and automatic model selection. Current state, round by round: `autowork/STATE.md` |
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
cp config.example.yaml config.yaml     # edit the roster: providers.example.yaml, named by this file
set -a && source .env && set +a        # provider keys are read from env only, never written into yaml
cargo run --locked -p router-cli -- serve --config config.yaml
```

Steps 4–6 below are the client side. Every command and output shape quoted here
was run against this branch's binary with a real provider round-trip.

### 1. Turn on inbound auth (optional, recommended)

The example config ships `auth_token_env` commented out so the first copy starts before you have
chosen a token. To require one, generate it and name the variable in the config — the name only,
never the value:

```bash
export ROUTER_TOKEN=$(openssl rand -hex 32)
```

```yaml
server:
  auth_token_env: ROUTER_TOKEN
```

If the key is set but the variable is unset or empty, `router serve` refuses to start (exit code
4) rather than serving unauthenticated:

```
router: config file config.yaml: server.auth_token_env names ROUTER_TOKEN, which is unset: refusing to start (a token-less start would serve unauthenticated)
```

### 2. macOS prerequisite: let localhost bypass the system proxy

```bash
export NO_PROXY=127.0.0.1,localhost     # codex (reqwest) deterministically; hermes (httpx) as insurance
```

With a system proxy configured, codex sends requests to a locally bound router into the
proxy instead; router receives no connection and the client reports `503 Service
Unavailable` (reproduced 4/4 with only the system proxy set, R11-3). hermes did not
reproduce this on the same machine (3/3 runs reached the router), so for hermes the
export is insurance, not the repair of an observed failure. Export the variable in the
shell that starts the client — see `docs/spec.md` §5 for the recorded
incident.

### 3. Probe liveness

```bash
curl -s http://127.0.0.1:8790/health
```

`/health` never requires a token. With auth on, its `auth` member reads
`"auth":{"required":true,"env":"ROUTER_TOKEN"}`; the token value itself never appears in any
response.

### 4. First request with curl

The example roster's native protocols differ per provider: the `deepseek` entry (which the
`coding-fast` alias points at) speaks `responses`; `zai` and `kimi` speak `chat`. v0.1
serves a request **natively only** when the inbound protocol equals the provider's `wire_api`;
anything else would need cross-protocol translation and answers `501 not_implemented`. So the
curl example against the stock roster uses the responses endpoint:

```bash
curl -s http://127.0.0.1:8790/v1/responses \
  -H "Authorization: Bearer $ROUTER_TOKEN" \
  -H 'content-type: application/json' \
  -d '{"model":"coding-fast","input":"Reply with the single word: pong","max_output_tokens":64}'
```

Either accepted header form works; `x-api-key: $ROUTER_TOKEN` is equivalent. A successful call
returns the upstream response verbatim: `"model":"deepseek-v4-pro"` (the resolved native id,
not the alias you sent), the answer text in `output`, and `usage.total_tokens` greater than
zero.

Without a token the same request is refused locally — nothing reaches the upstream:

```
HTTP/1.1 401 Unauthorized
x-router-request-id: req-1

{"error":{"type":"unauthorized","message":"inbound auth: no token presented (send it as 'Authorization: Bearer <token>' or 'x-api-key: <token>')","request_id":"req-1","details":{"header":null}}}
```

`details.header` names the header that was read and rejected: `"authorization"` for a wrong or
malformed `Authorization` value, `null` when no token was presented at all. A refused request
still leaves exactly one trace record (pre-pipeline, cost 0).

To exercise the chat endpoint instead, send the same shape to a **chat-native** provider from
your roster (`POST /v1/chat/completions` with a `messages` body, e.g. `zai/glm-5.3` from the
example file) — or add a `wire_api: chat` provider to the roster first.

### 5. codex

codex needs a provider entry, the token in the environment, and the localhost-proxy bypass.
In `~/.codex/config.toml`:

```toml
model_provider = "router"
model = "coding-fast"                   # a route: provider/model or an alias from your config
model_catalog_json = "~/.codex/models.json"

[model_providers.router]
name = "router"
base_url = "http://127.0.0.1:8790/v1"
wire_api = "responses"                  # must equal the route's native protocol (see step 4)
env_key  = "ROUTER_TOKEN"               # codex reads this variable and sends it as the auth header
```

Give codex a catalog entry for the slug (below; verified against codex-cli 0.137.0). A one-shot
`codex exec` also runs without one, but then codex has no context-window or capability metadata
for your route — the entry is the tested shape:

```json
{
  "models": [
    {
      "slug": "coding-fast",
      "display_name": "coding-fast (via router)",
      "description": "router alias -> deepseek/deepseek-v4-pro",
      "context_window": 1048576,
      "max_context_window": 1048576,
      "supported_reasoning_levels": [],
      "visibility": "list",
      "shell_type": "shell_command",
      "supported_in_api": true,
      "priority": 1,
      "base_instructions": "You are a helpful assistant.",
      "supports_reasoning_summaries": false,
      "support_verbosity": false,
      "truncation_policy": { "mode": "tokens", "limit": 10000 },
      "supports_parallel_tool_calls": true,
      "experimental_supported_tools": []
    }
  ]
}
```

Smoke it:

```bash
export NO_PROXY=127.0.0.1,localhost
codex exec --skip-git-repo-check -C /tmp "Reply with the single word: pong" < /dev/null
```

Expected: the banner names `provider: router` and `model: coding-fast`, the reply is `pong`,
and the router's trace records the session (codex sends a stable `prompt_cache_key`).

Why the non-obvious parts are there:

- `--skip-git-repo-check` — codex refuses to run outside a trusted git directory without it:
  `Not inside a trusted directory and --skip-git-repo-check was not specified.`
- `< /dev/null` — `codex exec` reads stdin for additional input (`Reading additional input from
  stdin...`); pointing it at `/dev/null` gives that read an immediate EOF when you drive it
  from a script or CI.
- `NO_PROXY` — step 2; reqwest does not honour the system proxy's exclusion list for
  `127.0.0.1`.
- `wire_api = "responses"` — step 4; a chat wire against the `deepseek` entry would answer
  `501 not_implemented`.
- `env_key = "ROUTER_TOKEN"` — the token a client sends router is router's own inbound token,
  not a provider credential; provider keys live only in the router process's environment.

### 6. hermes

hermes reaches the same native `/v1/responses` wire through its codex transport, and — unlike
codex — it needs an isolated `HERMES_HOME` so the setup never touches your default profile.

Write `<HERMES_HOME>/config.yaml` (the smoke used `/tmp/r11-smoke/hermes-home`):

```yaml
model: {default: coding-fast, provider: router, base_url: http://127.0.0.1:8790/v1, api_mode: codex_responses}
custom_providers:
  - {name: router, provider: router, base_url: http://127.0.0.1:8790/v1, model: coding-fast,
     api_mode: codex_responses, api_key_env: ROUTER_TOKEN, key_env: ROUTER_TOKEN}
```

Put the router token in `<HERMES_HOME>/.env` **twice** — `ROUTER_TOKEN=<token>` and
`ROUTER_API_KEY=<token>`, same value both times (why below). Then:

```bash
export NO_PROXY=127.0.0.1,localhost
HERMES_HOME=/tmp/r11-smoke/hermes-home hermes -z "Reply with the single word: pong"
# rc=0   stdout: pong

HERMES_HOME=/tmp/r11-smoke/hermes-home hermes --continue -z \
  "Now use the terminal tool to run the shell command: echo r11-hermes-tool . Then reply with exactly its stdout."
# rc=0   stdout: r11-hermes-tool
```

Expected in the trace: the session key is hermes's own `prompt_cache_key` (`pck_…`-prefixed,
content-derived); one `hermes -z` turn is two requests (the main conversation plus a small
auxiliary call in its own `pck_` session — 240 input tokens in the run, not a misroute); the
tool-call continuation hit the upstream prefix cache at 99.0% with `prefix.continuity` 1.0.
Chat instead of responses is a different, unwitnessed cell here — see
[the book](book/connecting-clients.md) before pointing hermes at `/v1/chat/completions`.

Why the non-obvious parts are there:

- `api_mode: codex_responses` (twice) — for a loopback `base_url` hermes's URL detection
  returns nothing and the entry would resolve to `chat_completions`, i.e. the wrong wire
  against a responses-native route.
- `ROUTER_API_KEY` in `.env` — hermes resolves a custom provider's credentials as
  `<PROVIDER>_API_KEY` and, without it, refuses with
  `No usable credentials found for provider 'router'. Set RAMP_ROUTER_API_KEY, ROUTER_API_KEY.`;
  `api_key_env` alone does not reach it. `key_env: ROUTER_TOKEN` plus that variable is the
  combination that reached 200.
- `HERMES_HOME` — the isolation boundary: the smoke ran hermes under a scratch home
  (`/tmp/r11-smoke/hermes-home`) precisely so config, `.env` and sessions live there and your
  default profile is left alone.

## CLI

```bash
router serve      --config config.yaml          # start the gateway
router stats      --config config.yaml --window 24h   # read the traces back out
router setup      [--non-interactive] [--config <path>] # guided config: copy the example, ask, edit in place
```

`serve`, `stats` and `setup` are the three subcommands the binary has. Without `--config`, all
three find the file by one rule ([`docs/spec.md` §4.12](docs/spec.md)): the XDG location
(`${XDG_CONFIG_HOME:-$HOME/.config}/router/config.yaml`), else `./config.yaml` — and when
neither exists, `serve`/`stats` refuse and name `router setup`, which writes the XDG file
(mode `0600`; any directory it creates `0700`). `setup` writes the pair from the two templates embedded in the
binary — `config.example.yaml` and the roster it names, `providers.example.yaml` — byte for byte, and asks only
about the keys that are yours, editing them by anchored single-line edits; a config that still carries the
roster inline is **moved** into the roster file by the same run, so the wizard's output is one shape
([ADR-038](design/decisions/ADR-038-setup-writes-the-pair.md)). `setup --check` validates the pair
without writing; see [the book](book/getting-started.md). The roster may live in a file of its own that the
config **names** with `providers_file:` — exactly one of the two keys is written, both written or neither is a
load refusal, the roster is named rather than searched, and `setup providers` edits whichever of the two files
holds the entry — or it may stay **inline** as `providers:`, which is equally legal for a file you write by
hand ([`docs/spec.md` §4.14](docs/spec.md), [ADR-037](design/decisions/ADR-037-roster-file-and-config-identity.md)).
`stats` reads the traces in the window and
prints the cost and cache report, the plan family's switches and their verified cost, and the requests
whose outcome is unknown; every figure carries its `verified` / `inferred` label, `--window` is required
(a saving that does not state its window cannot be checked), and `--json` prints the same report for a
script. Two further reporting surfaces are **planned design intent, not served commands**:
`router replay --trace traces/x.jsonl --config config.yaml` (offline replay: cost computed on the same
real code path) and `router trace tail` (follow the decisions and the accounting stream). Today the trace
record is where a decision and its accounting are read from.

## API

Inbound endpoints (the three are equivalent and mirror the upstream semantics per protocol):

| Method | Path | Protocol |
|---|---|---|
| POST | `/v1/chat/completions` | OpenAI chat completions |
| POST | `/v1/responses` | OpenAI responses |
| POST | `/v1/messages` | Anthropic messages |
| GET | `/health` | liveness, plus what this process actually loaded (and, when a `plan_policy` is configured, that family's account and probe deadline) |

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
crates/router-cli         `serve`, `stats` and `setup` — the three subcommands in v0.1
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
- The config is a **pair**: the config file plus the roster it names (spec §4.14) — back up the pair and the store together, and see [Operations](book/operations.md#backup) for what a restore does when one half is missing. A root that carries the roster inline is one file again; exactly one of the two shapes is written.
- Cache is the first-order cost lever: every rewrite must be **content-deterministic** (same content → same upstream bytes).
- Metrics are described by the observation contract in `docs/spec.md`; the trace is the authoritative record of what each request cost, and `router stats` reads it back (`router replay`, the offline same-code-path recomputation, is planned — `docs/spec.md` §9.3).

## License

Apache-2.0. See [`LICENSE`](LICENSE); the copyright line is in [`NOTICE`](NOTICE).
