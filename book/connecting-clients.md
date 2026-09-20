# Connecting clients

Status: written for v0.1. This is the onboarding path for real clients; wherever a detail is
contractual, this chapter links to the clause instead of restating it.

Getting a client onto router is a small edit to its base URL plus a bearer token. It has one
prerequisite that, when missed, makes the gateway look broken while nothing is wrong with it.

## Prerequisite: let localhost bypass the system proxy

Do this **before** pointing any client at router:

```bash
export NO_PROXY=127.0.0.1,localhost
```

On macOS, clients that read the system proxy settings (`reqwest` in codex, `httpx` in
hermes) do **not** honour the proxy exclusion list for `127.0.0.1`. When the system proxy
is configured, requests to a locally bound router are sent into the proxy instead, the
router receives no connection at all, and the client reports `503 Service Unavailable`.
The symptom looks like "router is down"; the cause is the proxy. Export the variable in the
same shell (or the same service environment) that starts the client, not only in the shell
that starts router.

The contract clause and its measurement are in
[`docs/spec.md` §5](../docs/spec.md); the captured evidence is in `AGENTS.md` and
`autowork/STATE.md`.

## Endpoints to point a client at

| Method | Path | What it is |
|---|---|---|
| POST | `/v1/chat/completions` | OpenAI chat completions |
| POST | `/v1/responses` | OpenAI responses |
| POST | `/v1/messages` | Anthropic messages |
| GET | `/health` | liveness, plus what this process actually loaded |

The three protocol endpoints are equivalent: same decision pipeline, same accounting, and
each mirrors its own protocol's upstream semantics. Which one you use is your client's
choice — codex speaks `responses`, most other tools speak `chat`, and an Anthropic-shaped
client speaks `messages`. `GET /metrics` appears in the README's endpoint table and is a
planned surface, not a served one today: [`docs/spec.md` §9.3](../docs/spec.md) is the list of
what is served and what is not.

## Per-client setup

**codex** declares a custom model provider:

```toml
[model_providers.router]
base_url = "http://127.0.0.1:8790/v1"
wire_api = "responses"                  # or "chat"
env_key  = "ROUTER_TOKEN"
```

**hermes / claude code** are the same shape: override the base URL, keep the token in the
environment. In every case the client's own API key is what router authenticates the
provider call with — the key lives in the environment of the router process (the config
file names the environment variable, never the value), and the token the client sends
router is the client's own bearer token.

Set the model field to an explicit `provider/model` or to an alias defined in your config. What you
write there is a **route**, not a provider model name: router resolves it and sends the provider its
own native model id, so `deepseek/deepseek-v4-pro` and an alias that points at it reach the upstream
identically. Your own string is not lost — the trace records it as `decision.requested_model`
alongside the native id it resolved to. Do **not** paste the bare native id into a client: it is not a
route and not an alias, so it resolves to nothing and comes back as `404 unknown_model`. `auto` is
deliberately not enabled in v0.1: it returns an explicit error instead of guessing, and the selector
slot is reserved for a plugin.

## What you get back

The upstream response. Two additions are **planned design intent, not served in v0.1** — do not
build against them yet:

- **`router_meta`** in the response body: the plugin chain that applied, the per-step
  transform accounting, the resolved session and the cache state. When it lands, this block is
  router's own and is removed again on the way out when the request is forwarded upstream.
- **`X-Router-Request-Id`** on every response, **`X-Router-Session`** when a session was
  resolved, and **`X-Router-Lossy`** when the translation was lossy — the header set
  [`docs/spec.md` §8](../docs/spec.md) requires. On a streamed response all three must already
  be present in the response head, before the first event.

What v0.1 sends today: `X-Router-Request-Id` on the streaming path, and
`X-Router-Failover-From` on a response served by a fallback route instead of the primary one.
The buffered path carries the request id inside an error body rather than as a header, and the
other two headers land with the translation cells they describe.

## Session identity

Router takes the session identity from the client's own signals, preferring the body's
cache key (`prompt_cache_key`) and falling back to configured headers; the sources and
their order are the `session.key_sources` of your config. Do **not** invent a session id
when the client already sends one: the upstream's own cache is keyed by the client's value,
so a different key would make router's view of the session and the provider's cache
disagree. When no source is present the request simply has no session (recorded as such,
`sticky_hit` false) — that is a normal state, not an error.

## Where your data lands

Both paths are resolved **relative to the directory containing the config file**, not the
directory you happen to run `router` from:

- **Traces** — append-only JSONL, one file per rollover interval, under the configured
  `trace.dir`. This is the analysis record (one decision record per request) and the only
  channel to the analysis side.
- **The local state store** — one SQLite file at `<config dir>/state/router.db`, holding the
  event log (the state truth) and the projections built from it. Both are `state/`, and
  `state/` is gitignored.

Neither stores request or response bodies: the log keeps a body hash and a pointer, so a
payload can be checked when you captured it out of band and is honestly absent when you did
not. Backup, inspection and what the store does when it cannot be opened are in
[Operations](operations.md).

## Smoke checklist per client

1. `export NO_PROXY=127.0.0.1,localhost` (and keep it for the client's whole lifetime).
2. Start the gateway and confirm `curl -s http://127.0.0.1:8790/health` answers, and that
   the report names the plugins, the providers whose keys are present, and the resolved
   state path you expect.
3. Run **one full session**, not a single request: an agent's first turn misses the cache
   and the following turns should hit it. Watch the session in the trace, or run
   `router stats --config config.yaml --window 1h` over it, and confirm the first turn is
   priced as a miss and the later turns are not.
4. Confirm the identity fields line up: the trace's session equals the client's cache key,
   and `turn_index` increases within the session.
5. If anything looks wrong, check prefix continuity between turns first — that number, not
   the model choice, is what tells you whether a transform is breaking the upstream cache.

The three protocol endpoints are served today (the [README](../README.md) status lists what has
landed). What still answers `501 not_implemented` is a route that would need cross-protocol
translation — an inbound protocol that is not the provider's own `wire_api` (see
[Protocols](protocols.md)). The two prerequisites above are the ones you need before any of it
can reach router.

## Authoritative sources

- [`docs/spec.md` §5](../docs/spec.md) — the access prerequisite, as a contract clause with
  the measurement behind it.
- [`docs/spec.md` §2](../docs/spec.md) — the inbound endpoint per protocol and the outbound
  selection rule.
- [`docs/spec.md` §3](../docs/spec.md) — model selection semantics (`provider/model`,
  alias, `auto`).
- [`docs/spec.md` §4.1](../docs/spec.md) and [§4.5](../docs/spec.md) — where traces and the
  local store live, and how a relative path is resolved.
- [`docs/spec.md` §6](../docs/spec.md) — the observation contract, including what the trace
  records about a session and a turn.
- [`design/DESIGN.md` §12.10](../design/DESIGN.md) — the data plane and storage landing
  (streaming behaviour, config resolution, store).
- [`README.md`](../README.md) — copy-pasteable client snippets and the `NO_PROXY` line.
- [`AGENTS.md`](../AGENTS.md) — the recorded localhost/proxy incident and the stateless
  client-traffic evidence.
