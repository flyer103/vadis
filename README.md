# vadis

[![CI](https://github.com/flyer103/vadis/actions/workflows/ci.yml/badge.svg)](https://github.com/flyer103/vadis/actions/workflows/ci.yml) [![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

Local-first LLM gateway for codex / hermes / Claude Code: bytes go upstream unchanged, the prompt cache keeps hitting, and every request leaves a decision + cost record.

Three native wires — `POST /v1/chat/completions`, `POST /v1/responses`, `POST /v1/messages` — are served from one local endpoint. Routing resolves before the upstream call; the upstream then sees the client's own bytes, with only two permitted mutations: vadis-owned top-level fields are dropped, and the top-level `model` value is replaced with the resolved provider-native id. Every rewrite is a pure function of (content, stable config), so the prefix on turn N stays a prefix of turn N+1 and an edit cannot invalidate the upstream prompt cache. The contracts are [`docs/spec.md`](docs/spec.md) (WHAT) and [`design/DESIGN.md`](design/DESIGN.md) (HOW); the guide is [the user book](book/SUMMARY.md).

**What vadis does for you:**

- **One local endpoint, three wires** — point a chat, responses, or Anthropic client at `http://127.0.0.1:8790/v1`; every request resolves to one configured (provider, model).
- **No cache damage** — content-deterministic rewrites keep the upstream prefix cache hitting across a session.
- **A record per request** — the decision and the upstream-measured usage and cost land in one append-only trace record per completed request.

## Features

| Capability | what it is |
|---|---|
| **Three wires, one endpoint** | chat completions, responses and Anthropic messages, each served natively when the route's provider speaks that wire |
| **Byte-faithful passthrough** | the client's own bytes minus vadis-owned top-level fields, carrying the resolved provider-native `model` id; a streaming response relays the upstream's SSE events as they arrive |
| **A decision and a cost record per request** | the decision, the plan family's switches, the usage the upstream reported, integer-Nano amounts and the quota snapshot; `vadis stats` and `GET /health` read them back |
| **Plan-first routing, classified failover** | a subscription account as primary, its metered twin as spill, a session-boundary probe back; upstream failures are classified and the request fails over along the configured chain |
| **Prefix-cache continuity** | session identity is the client's own key (`prompt_cache_key`, then the configured headers), so one turn cannot invalidate the cache for the turns after it |
| **Hot reload, guided setup** | a config change takes effect without a restart; `vadis setup` writes the config trio from templates embedded in the binary and asks about only your own keys ([plugins](book/plugins.md)) |

## Requirements

- **Rust** — `rust-toolchain.toml` pins the verified toolchain and `rustup` installs it; the minimum supported version is `rust-version` in `Cargo.toml` (`[workspace.package]`), currently **1.88**.
- **A C compiler** — `rusqlite` uses its `bundled` feature, so the pinned SQLite is compiled into `vadis-store`; a working `cc` is needed.
- **Provider keys in the environment only** — the config file names the environment variable, never the value.

## Quick Start

The guided path — `vadis setup` writes the config trio and asks about only your own keys:

```bash
cargo build --release
./target/release/vadis setup          # writes the config trio, then asks about your deployment
export <the variables it names>        # the wizard prints each name; `vadis setup --check` verifies them
./target/release/vadis serve
```

`vadis setup` lands the **trio**: the root config at `${XDG_CONFIG_HOME:-$HOME/.config}/vadis/config.yaml` (mode `0600`; any directory it creates `0700`), the roster that config names with `providers_file:`, and the rule file the config's `plugins` entry points at (`rules/tool_output.toml`). Each is created when absent, left untouched when present, replaced only by `--force` with the previous bytes kept at `<file>.bak` ([ADR-046](design/decisions/ADR-046-setup-lands-the-rule-file.md); the contract is [`docs/spec.md` §4.11](docs/spec.md)). The templates are embedded in the binary, so this works from anywhere. `vadis setup --check` loads the file with the same loader `serve` runs and reports every environment variable it names — exit `0` when all are present, `4` when one is missing.

**The repository-local alternative** — a copy of the shipped example pair instead of the wizard:

```bash
cp config.example.yaml config.yaml     # edit the roster: providers.example.yaml, named by this file
set -a && source .env && set +a        # provider keys are read from env only, never written into yaml
cargo run --locked -p vadis-cli -- serve --config config.yaml
```

The config's `plugins` entry names `./rules/tool_output.toml` relative to the config's own directory, so a pair copied **out** of the checkout must take the rule file (`rules/tool_output.toml`) with it, or the transform engine starts with a dangling reference.

The client side, in short — the full per-client walkthrough, with the non-obvious flags explained line by line, is [Connecting clients](book/connecting-clients.md):

```bash
export VADIS_TOKEN=$(openssl rand -hex 32)     # 1. inbound auth: name the variable in the config, never the value
export NO_PROXY=127.0.0.1,localhost             # 2. macOS: let localhost bypass the system proxy
curl -s http://127.0.0.1:8790/health            # 3. liveness; never requires a token
```

With `auth_token_env: VADIS_TOKEN` written into the config's `server:` section, every protocol endpoint requires the token; if the variable is unset or empty, `vadis serve` refuses to start (exit `4`) rather than serving unauthenticated. With a system proxy configured, codex sends requests for a locally bound router into the proxy, router receives no connection, and the client reports `503 Service Unavailable` — hence `NO_PROXY`.

First request with curl — accepted as `-H "Authorization: Bearer <your-token>"` or `-H "x-api-key: <your-token>"`, either header form works. The example roster's `deepseek` entry (which the `coding-fast` alias points at) speaks `responses`, so against the stock roster use the responses endpoint:

```bash
curl -s http://127.0.0.1:8790/v1/responses \
  -H "Authorization: Bearer <your-token>" \
  -H 'content-type: application/json' \
  -d '{"model":"coding-fast","input":"Reply with the single word: pong","max_output_tokens":64}'
```

A success returns the upstream response verbatim — `"model":"deepseek-flash"` (the resolved native id, not the alias you sent) — and `usage.total_tokens` greater than zero. Without a token the same request is refused locally (`401 unauthorized`; nothing reaches the upstream) and still leaves exactly one trace record, pre-pipeline and cost `0`. v0.1 serves a request **natively only** when the inbound protocol equals the provider's `wire_api`; a route the client names in a cell that needs cross-protocol translation answers `501 not_implemented`.

**codex** — in `~/.codex/config.toml`:

```toml
model_provider = "vadis"
model = "coding-fast"                   # a route: provider/model, or an alias from your config
model_catalog_json = "~/.codex/models.json"

[model_providers.vadis]
name = "vadis"
base_url = "http://127.0.0.1:8790/v1"
wire_api = "responses"                  # must equal the route's native protocol
env_key  = "VADIS_TOKEN"                # codex reads this variable and sends it as the auth header
```

Then `export NO_PROXY=127.0.0.1,localhost` and `codex exec --skip-git-repo-check -C <dir> "Reply with the single word: pong" < /dev/null`. The `models.json` catalog entry for the slug, the flag-by-flag explanations, and the expected smoke output are in [Connecting clients](book/connecting-clients.md).

**hermes** reaches the same native `/v1/responses` wire through its codex transport and needs an isolated `HERMES_HOME`. Write `<HERMES_HOME>/config.yaml` with `model: {default: coding-fast, provider: vadis, base_url: http://127.0.0.1:8790/v1, api_mode: codex_responses}` and a matching `custom_providers` entry, then put the token in `<HERMES_HOME>/.env` as both `VADIS_TOKEN` and `VADIS_API_KEY`. Both are load-bearing: without `api_mode: codex_responses`, hermes resolves a loopback `base_url` to the chat wire; without `VADIS_API_KEY`, it refuses with `No usable credentials found`. The full block and the smoke commands are in [Connecting clients](book/connecting-clients.md).

Both client blocks are outside every gate — no conformance case reads a client config.

## CLI

| Command | what it does |
|---|---|
| `vadis serve --config config.yaml` | starts the HTTP proxy — config-driven, one process |
| `vadis stats --config config.yaml --window 24h` | prints the window's figures from the trace and event log (spec §9.2) |
| `vadis setup [--non-interactive] [--config <path>]` | guided first configuration: the trio, the questions, the edits in place |
| `vadis setup --check` | loads the file with the same loader `serve` runs, then checks every environment variable it names — no prompt, no write |

`serve`, `stats` and `setup` are the three subcommands the binary has. Without `--config`, all three find the file by one rule ([`docs/spec.md` §4.12](docs/spec.md)): the XDG location (`${XDG_CONFIG_HOME:-$HOME/.config}/vadis/config.yaml`), else `./config.yaml`; when neither exists, `serve` / `stats` refuse and name `vadis setup`, which creates the XDG file. The trio `setup` writes is described in [Quick Start](#quick-start).

`--window` is **required**: a report must state the window it covers. The grammar is `300ms` / `90s` / `15m` / `1h30m` — there is no `d` unit, a day is `24h`. Figures carry their `verified` / `inferred` label, and `--json` prints the same figures for a script.

## API

Inbound endpoints (the three are equivalent and mirror the upstream semantics per protocol):

| Method | Path | Protocol |
|---|---|---|
| POST | `/v1/chat/completions` | OpenAI chat completions |
| POST | `/v1/responses` | OpenAI responses |
| POST | `/v1/messages` | Anthropic messages |
| GET | `/health` | liveness, plus what this process actually loaded (and, when a `plan_policy` is configured, that family's account and probe deadline); never requires a token |
| GET | `/metrics` | the last 900 seconds of this process's own `trace.dir`, in the Prometheus text exposition format, behind the same token guard as the three protocol endpoints (spec §4.16) |

The `model` field accepts `provider/model`, an alias from the config, or `auto` (v0.1 returns `400` and says a plugin must take over). A `vadis_meta` response block is planned design intent — v0.1 does not add it to the response body, and what it would report lives in the trace record instead. Every non-2xx answers the unified error body, which clients parse rather than string-match.

## Build & Test

```bash
cargo build --workspace
cargo test  --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

`cargo test` includes the protocol matrix in `tests/conformance` (the 3×3 wire matrix plus the byte / cache / accounting invariants). All four must be green before a pull request; CI runs exactly these four commands on every push and pull request.

## Documentation

- [User book](book/SUMMARY.md) — the user-facing guide: introduction, getting started, connecting clients, protocols, cost and caching, plugins, observability and accounting, operations, FAQ, roadmap.
- [Spec (WHAT)](docs/spec.md) — protocol contracts, config schema, observation and accounting conventions.
- [Design (HOW)](design/DESIGN.md) — crate layout, plugin runtime, cost engine, cache policy.
- [Decisions (WHY)](design/decisions/) — the ADRs, append-only.

## Contributing

The four gates must be green before a pull request, one logical change per commit, all repository text in English, and the measurement is not part of the change space — a pull request that turns a red gate green by editing the gate is rejected. The full conventions are in [`CONTRIBUTING.md`](CONTRIBUTING.md).

## Security

Report a vulnerability **privately** through GitHub's private vulnerability reporting — never a public issue. See [`SECURITY.md`](SECURITY.md).

## Ops

- **No server-side session state.** Session identity is the client's own key (`prompt_cache_key`, then the configured headers) — `store` / `previous_response_id` are never inspected, so `state.stateful_inbound` in the trace is `false` on every request.
- **One local store.** Local state is one SQLite/WAL file (`state/vadis.db`, spec §4.5, [ADR-009](design/decisions/ADR-009-persistence-boundary-sqlite-wal-store.md)) holding the event log and its projections; the trace stays the only analysis channel, and **no request or response body is stored**.
- **Back up the pair and the store together.** The config is a pair — the file plus the roster it names; back both up with the store, and see [Operations §Backup](book/operations.md#backup).

## License

Apache-2.0. See [`LICENSE`](LICENSE); the copyright line is in [`NOTICE`](NOTICE).
