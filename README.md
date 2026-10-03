# router

[![CI](https://github.com/flyer103/router/actions/workflows/ci.yml/badge.svg)](https://github.com/flyer103/router/actions/workflows/ci.yml) [![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

A **local-first, multi-protocol LLM gateway**: point codex / hermes / claude code at `http://127.0.0.1:8790/v1` and router resolves each request to one configured (provider, model), saves tokens **without breaking the upstream prefix cache**, and records what it decided and what the upstream measured into a trace record for every request it completes.

The upstream sees the client's own bytes. Routing is resolved **first**, the upstream is called with the resolved provider-native `model` id, and the only two byte mutations permitted are deleting router-owned top-level fields and replacing that one top-level `model` **value span** — never message content, order, whitespace or tool schemas.

Every **terminal outcome** — success, upstream 4xx/5xx, connect refusal, timeout — leaves exactly one trace record. What its usage and cost figures say is what the upstream's own carrier measured: a record with **no** carrier is marked `usage_missing: true` and is priced nowhere — never as a zero.

Every transform is a pure function of (content, stable config). The prefix on turn N is a prefix of turn N+1, so an edit cannot invalidate the upstream's prompt cache for the turns after it.

## Features

| **Capability** | what it is |
|---|---|
| **Three wires, one local endpoint each** | `POST /v1/chat/completions`, `POST /v1/responses` and `POST /v1/messages` — semantically equivalent, each served natively when the route's provider speaks the same wire |
| **Byte-faithful passthrough, streaming included** | the client's own bytes minus router-owned top-level fields, carrying the resolved provider-native `model` id; on the streaming path the upstream's SSE events are relayed as they arrive |
| **A decision and a cost ledger in every trace record** | one record per terminal outcome — the decision, the plan family's switches, the usage the upstream reported, integer-Nano amounts and the quota snapshot; `router stats` and `GET /health` read them back out |
| **Plan-first routing, with a classified failover chain** | a subscription account is the primary, its metered twin is the spill, the way back is a session-boundary probe; upstream failures are classified, and the request can fail over along the configured chain |
| **Prefix-cache continuity** | session identity is the client's own key (`prompt_cache_key`, then the configured headers), and every rewrite is content-deterministic, so one turn cannot invalidate the cache for the turns after it |
| **A declarative plugin runtime** | the runtime mounts what the `plugins:` list declares — exactly one kind today, `builtin/transform_rules`, the rule engine over `rules/tool_output.toml`; `inject` and `disabled` are honoured, `isolate` / `intercept` are inert, tier-B is a stub, and the three always-resident builtins stay resident |
| **Hot reload** | a configuration change takes effect without a restart |
| **Guided setup** | `router setup` writes the config pair — and the rule file that config names — from the templates embedded in the binary, byte for byte, and edits only the keys that are yours |

## Requirements

- **Rust** — `rust-toolchain.toml` pins the toolchain the project is verified on, and `rustup` installs it for you. The minimum supported version is lower and is declared as `rust-version` in `Cargo.toml` (`[workspace.package]`) — currently **1.88**.
- **A C compiler** — `rusqlite` is used with its `bundled` feature, so the pinned SQLite C library is compiled into `router-store` at build time — no system SQLite is needed, but a working `cc` is.
- **Provider keys in the environment only** — the config file names the environment variable, never the value.

## Quick Start

```bash
cp config.example.yaml config.yaml     # edit the roster: providers.example.yaml, named by this file
set -a && source .env && set +a        # provider keys are read from env only, never written into yaml
cargo run --locked -p router-cli -- serve --config config.yaml
```

The config is a **pair**: the root file plus the roster it names. The roster may live in a file of its own that the config **names** with `providers_file:` — exactly one of the two keys is written, both written or neither is a load refusal, and the roster is named rather than searched — or it may stay **inline** as `providers:`, which is equally legal for a file you write by hand (spec §4.14, [ADR-037](design/decisions/ADR-037-roster-file-and-config-identity.md)). The shipped example uses the named form: `providers_file: providers.example.yaml`.

Steps 4–6 are the client side. Those steps were verified end to end by the R11 onboarding smoke ([`autowork/harness/r11-onboarding-smoke-runbook.md`](autowork/harness/r11-onboarding-smoke-runbook.md)); the figures it quotes belong to that run.

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
proxy instead; router receives no connection and the client reports `503 Service Unavailable`
(reproduced 4/4 with only the system proxy set, R11-3). hermes did not reproduce this on the
same machine (3/3 runs reached the router), so for hermes the export is insurance, not the
repair of an observed failure. Export the variable in the shell that starts the client — see
`docs/spec.md` §5 for the recorded incident.

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
returns the upstream response verbatim: `"model":"deepseek-flash"` (the resolved native id,
not the alias you sent), the answer text in `output`, and `usage.total_tokens` greater than
zero.

Without a token the same request is refused locally — nothing reaches the upstream:

```
HTTP/1.1 401 Unauthorized
x-router-request-id: req-1

{"error":{"type":"unauthorized","message":"inbound auth: no token presented (send it as 'Authorization: Bearer <token>' or 'x-api-key: <token>')"}}
```

`details.header` names the header that was read and rejected: `"authorization"` for a wrong or
malformed `Authorization` value, `null` when no token was presented at all. A refused request
still leaves exactly one trace record (pre-pipeline, cost 0).

To exercise the chat endpoint instead, send the same shape to a **chat-native** provider from
your roster (`POST /v1/chat/completions` with a `messages` body, e.g. `zai/glm-5.3` from the
example file, `wire_api: chat`) — or add a `wire_api: chat` provider to the roster first.

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
      "description": "router alias -> deepseek/deepseek-flash",
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

- `--skip-git-repo-check` — codex refuses to run outside a trusted git directory without it: `Not inside a trusted directory and --skip-git-repo-check was not specified.`
- `< /dev/null` — `codex exec` reads stdin for additional input (`Reading additional input from stdin...`); pointing it at `/dev/null` gives that read an immediate EOF when you drive it from a script or CI.
- `NO_PROXY` — step 2; reqwest does not honour the system proxy's exclusion list for `127.0.0.1`.
- `wire_api = "responses"` — step 4; a chat wire against the `deepseek` entry would answer `501 not_implemented` (an inbound protocol the entry does not declare in `supports` answers `400` before that).
- `env_key = "ROUTER_TOKEN"` — the token a client sends router is router's own **inbound** token, not a provider credential; provider keys live only in the router process's environment.

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

In that run: the session key was hermes's own `prompt_cache_key` (`pck_…`-prefixed,
content-derived); one `hermes -z` turn was two requests (the main conversation plus a small
auxiliary call in its own `pck_` session — 240 input tokens in the run, not a misroute); and the
tool-call continuation hit the upstream prefix cache at 99.0% of its input tokens. Continuity is
a relation, per client and per turn — that turn read `prefix.continuity` 1.0, and a client that
changes its tool array between turns reads less; do not read 1.0 as a constant. Chat instead of
responses is a different, **unwitnessed** cell here — see
[the book](book/connecting-clients.md) before pointing hermes at `/v1/chat/completions`.

Both client blocks above are outside every gate — no conformance case reads a client config —
and the client versions they name were read on 2026-09-21, not maintained as a standing claim.

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

| Command | what it does |
|---|---|
| `router serve --config config.yaml` | starts the HTTP proxy — config-driven, one process |
| `router stats --config config.yaml --window 24h` | prints the window's figures from the trace and event log (spec §9.2) |
| `router setup [--non-interactive] [--config <path>]` | guided first configuration: the template pair, the questions, the edits in place |
| `router setup --check` | loads the file with the same loader `serve` runs, then checks every environment variable it names — no prompt, no write |

`serve`, `stats` and `setup` are the three subcommands the binary has. Without `--config`, all
three find the file by one rule ([`docs/spec.md` §4.12](docs/spec.md)): the XDG location
(`${XDG_CONFIG_HOME:-$HOME/.config}/router/config.yaml`), else `./config.yaml` — and when
neither exists, `serve` / `stats` refuse and name `router setup`, which creates the XDG file
(mode `0600`; any directory it creates `0700`).

`router setup` writes the pair from the two templates embedded in the binary — `config.example.yaml`
and the roster it names, `providers.example.yaml` — byte for byte, and it writes the **third** file the same
config names: the rule file its `plugins` entry points at (`rules/tool_output.toml`), copied from the rule
file embedded in the binary, so a fresh install's transform engine finds its rules instead of starting with a
dangling reference — created when it is absent, left untouched when it is there, replaced only by `--force`
with the `plugins` section selected
([ADR-046](design/decisions/ADR-046-setup-lands-the-rule-file.md)). It asks only about the keys
that are yours, editing them by anchored single-line edits; a config that still carries the
roster inline is **moved** into the roster file by the same run, so the wizard's output is one
shape ([ADR-038](design/decisions/ADR-038-setup-writes-the-pair.md)). `router setup providers`
edits whichever of the two files holds the entry.

`--window` is **required**: a report must state the window it covers. The grammar is `300ms` /
`90s` / `15m` / `1h30m` — there is no `d` unit, a day is `24h`. The figures the report prints
carry their `verified` / `inferred` label, and `--json` prints the same figures for a script.

`router replay --trace traces/x.jsonl --config config.yaml` and `router trace tail` are
**planned, not served**: they are not subcommands of this binary, and the parser refuses them
with a usage error and a non-zero exit — never a silently ignored flag. Today the trace record
**is** the interface: append-only JSONL, one decision record per completed request, readable with
any JSON tool, and the iteration loop replays it from [`autowork/harness/replay.py`](autowork/harness/replay.py).

## API

Inbound endpoints (the three are equivalent and mirror the upstream semantics per protocol):

| Method | Path | Protocol |
|---|---|---|
| POST | `/v1/chat/completions` | OpenAI chat completions |
| POST | `/v1/responses` | OpenAI responses |
| POST | `/v1/messages` | Anthropic messages |
| GET | `/health` | liveness, plus what this process actually loaded (and, when a `plan_policy` is configured, that family's account and probe deadline) |
| GET | `/metrics` | the last 900 seconds of this process's own `trace.dir`, in the Prometheus text exposition format (`text/plain; version=0.0.4; charset=utf-8`) — behind the same token guard as the three protocol endpoints (spec §4.16) |

`GET /metrics` **is** served: a scrape answers §9.2's own figures over the last 900 seconds — a
process constant, not a flag and not a query parameter — in the Prometheus text exposition format
(`text/plain; version=0.0.4; charset=utf-8`), behind the same token guard as the three protocol
endpoints (`/health`'s exemption is `/health`'s alone), and its contract is
[`docs/spec.md` §4.16](docs/spec.md).

The `model` field accepts `provider/model`, an alias from the config, or `auto` (v0.1 returns
`400` and says a plugin must take over). A `router_meta` response block — the plugin chain that
hit, the accounting of each transform step, and the session and cache state — is planned design
intent: v0.1 does not add it to the response body, and what it would report lives in the trace
record instead.

Every non-2xx answers the unified error body (a stub endpoint included); clients parse it and
should not string-match the message. A local refusal is one of them: `401`, with the correlation
header present — the quick start's head shows the single header that matters.

## Build & Test

```bash
cargo build --workspace
cargo test  --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

`cargo test` includes the 3×3 protocol matrix in `tests/conformance`. All four must be green
before a pull request. A card's own test run proves the card; the round is proven only by the
integration verify — the four gates on the merged HEAD — and a gate verdict is void if anything
lands after it.

## Layout

- `crates/router-core` — the domain core: the config schema, the decision pipeline, the byte-level body primitive, and the cost / cache / quota / breakeven engines with the trace contract.
- `crates/router-protocol` — the three-protocol codec: native-path usage normalization for the three wire shapes and the SSE block parser; the cross-protocol translation matrix is not implemented in v0.1.
- `crates/router-providers` — the upstream provider adapters: the HTTP transport, auth-header assembly, upstream error classification and the SSE response relay.
- `crates/router-proxy` — the axum data plane: byte-faithful forwarding, the SSE relay, `/health`, and the per-request accounting that closes a trace record.
- `crates/router-plugins` — the built-in tier-A plugins and their assembly: `serve` mounts what the `plugins:` list declares — today one kind, `builtin/transform_rules`, the rule engine over `rules/tool_output.toml` — and that engine's composition step runs on the request path, on the non-buffered route and the streaming route alike. What it *plans* is request-scoped: in passthrough mode nothing is planned, even when a rule would match.
- `crates/router-runtime` — the Cordis-semantics runtime: the effect / coeffect / fiber model and the declarative loader, implemented in R41-2 and driven by the tier-A assembly from the `plugins:` list since R41-3.
- `crates/router-plugin-sdk` — the out-of-process tier-B plugin protocol (UDS framing); a scaffold only, in v0.1.
- `crates/router-store` — the one local SQLite/WAL store (the event log and its projections) and the JSONL trace sink; the only writer of the state database.
- `crates/router-cli` — the `router` binary: `serve`, `stats` and `setup`, the three subcommands v0.1 has, plus config loading, the reload watcher and the startup resolution.
- `tests/conformance` — the 3×3 protocol matrix and the byte / cache invariants; a change that breaks them is not landed regardless of downstream wins.
- `autowork/` — the iteration loop: the harness (collect / replay / judge), corpora, traces and round files; `autowork/STATE.md` is the authoritative current-state document.

The other documentation homes — `book/`, `docs/`, `design/` — are the ones **Documentation** describes.

## Status

**What is served (v0.1 data plane, landed).** `serve` is config-driven, and the three protocol
endpoints forward **natively** — the client's own bytes, minus router-owned top-level keys, with
the resolved provider-native `model` id, never parsed and reserialized; the streaming path relays
the upstream's SSE events.

**The accounting.** Every terminal outcome — success, upstream 4xx/5xx, connect refusal, timeout —
leaves exactly one trace record; a **refused** request leaves one too (spec §6's pre-pipeline
class, `event_id: 0`, nothing priced). What its usage and cost figures say is what the upstream's
own carrier measured: a record with **no** carrier is marked `usage_missing: true` and is priced
nowhere — never as a zero. The local store holds the event log and its projections.

**One class is honestly outside that enumeration.** A stream the client disconnects from mid-flight
never reaches a terminal outcome, so it leaves its write-ahead event rows and **no trace record** —
the request is an `unknown_outcome`; the upstream may already have billed it, so the quota is not
charged again and no cost is invented (`design/DESIGN.md` §12.10.3 R5).

**The two read-back surfaces.** `router stats` reads those records back out (cost, cache, the plan
family's switches **and their verified cost** — a cost, not a saving), and `GET /health` reports
what this process loaded, including a configured plan family's account and probe deadline.

**A third surface is served, for a scraper.** `GET /metrics` answers the last 900 seconds of
this process's own `trace.dir` in the Prometheus text exposition format, behind the same token guard
as the three protocol endpoints (`/health`'s exemption is `/health`'s alone); the whole contract is
[`docs/spec.md` §4.16](docs/spec.md). What it exports is a closed list — §9.2's own figures,
rendered, minus the one named below — and a figure that carries one of §9.2's labels carries it as a
machine-readable `provenance` value: the four §4.16 fixes are `verified`, `inferred`, `measured` and
`count`. Not every series carries one — a plain count of records does not — so an inferred figure
cannot be read as a measured one. What it deliberately does **not** export is a closed list too: no
client bytes or message content, no key material (not even the name of the variable holding it),
nothing per-request and nothing keyed by traffic, no session identity, no per-provider or per-model
cost split, and no config echo. One figure is absent by design: the `unknown outcome requests` figure
lives in the event log, and the serving path never scans that log. The response names the omission
in-band every time, so its absence cannot be read as a zero.

**Not implemented in v0.1:**

- the six cross-protocol translation cells are not served: a route the **client names** in a cell that needs translation is answered `501 not_implemented`, while a **candidate the client did not name** is skipped rather than translated;
- the `replay` / `trace tail` subcommands: not subcommands of this binary — the parser refuses them with a usage error and a non-zero exit;
- the `router_meta` response block;
- tier-B (out-of-process) plugins;
- automatic model selection (`model: auto` answers `400` in v0.1; the slot is reserved for a plugin).

| | |
|---|---|
| Model choice | **Explicit** (provider, model) or an alias; automatic selection is a plugin slot, not enabled in v0.1 |
| Protocols | Inbound and outbound native passthrough for OpenAI chat completions / OpenAI responses / Anthropic messages |
| Cost | P0 cache fidelity → P1 input-side payload compression → P2 output-side discipline → P3 provider arbitrage |
| Iteration | the `autowork/` loop keeps iterating the product by round ([`autowork/program.md`](autowork/program.md)) |

There is no comparative speed claim anywhere in this file: no same-basis measurement against
another gateway exists, and none is quoted.

## Documentation

- [User book](book/SUMMARY.md) — the user-facing guide: introduction, getting started, connecting clients, protocols, cost and caching, plugins, observability and accounting, operations, FAQ, roadmap. Its *Introduction* and *Roadmap* chapters say they are outlines; the rest is the map you want.
- [Spec (WHAT)](docs/spec.md) — protocol contracts, config schema, observation and accounting conventions.
- [Design (HOW)](design/DESIGN.md) — crate layout, plugin runtime, cost engine, cache policy.
- [Decisions (WHY)](design/decisions/) — the ADRs, append-only.
- [Autowork](autowork/program.md) — the loop charter, its gates and its direction pool; `autowork/STATE.md`'s **Round Log** is the round-by-round index, and each round's own record is in `autowork/progress/`.

## Contributing

The four gates must be green before a pull request; one logical change per commit; all repository
text is English; and the measurement is not part of the change space — a pull request that turns a
red gate green by editing the gate is rejected. The full conventions are in
[`CONTRIBUTING.md`](CONTRIBUTING.md).

## Security

Report a vulnerability **privately** through GitHub's private vulnerability reporting — never a
public issue. See [`SECURITY.md`](SECURITY.md).

## Ops

- **No server-side session state.** Session identity is the client's own key (`prompt_cache_key`, then the configured headers, spec §4) — never `store` / `previous_response_id`. v0.1 does not inspect those two fields: they are forwarded byte-for-byte like every other client field and take no part in routing, so `state.stateful_inbound` in the trace is `false` on every request. Detecting inbound state and marking the trace record is planned, not implemented (known gap G-F).
- **One local store.** Local state is one SQLite/WAL file (`state/router.db`, spec §4.5, ADR-009) holding the event log and its projections; the trace stays the only analysis channel (ADR-005), and **no request or response body is stored**.
- **Back up the pair and the store together.** The config is a pair: the file plus the roster it names. Back up both, with the store, and see [Operations §Backup](book/operations.md#backup) for what a restore does when one half is missing.
- **Cache is the first-order cost lever.** Every rewrite must be **content-deterministic** (same content → same upstream bytes), because the upstream prefix cache is what the turns after it are billed against.
- **The trace is the authoritative record** of what each request was measured to cost, and `router stats` reads it back. `docs/spec.md` §9.3 lists the reporting surfaces that are not served yet, and the rule behind it: a documented-but-unreachable surface is a defect.

## License

Apache-2.0. See [`LICENSE`](LICENSE); the copyright line is in [`NOTICE`](NOTICE).
