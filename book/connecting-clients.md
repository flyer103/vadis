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

On macOS, codex's HTTP stack (`reqwest`) reads the system proxy settings and does
**not** honour the exclusion list for `127.0.0.1` — this failure is deterministic (4/4
runs with only the system proxy configured, no proxy variables in the environment). When
the system proxy is configured, requests to a locally bound router are sent into the
proxy instead, the router receives no connection at all, and the client reports
`503 Service Unavailable`. The symptom looks like "router is down"; the cause is the
proxy. hermes (`httpx`) did **not** reproduce this on the same machine (3/3 runs reached
the loopback router with only the system proxy configured — httpx appears to honour the
exclusion list, or not to read the macOS system proxy at all), so for hermes the export
is insurance rather than the repair of an observed failure. An earlier measurement that
forced a proxy into the environment explicitly (R1-3) did make httpx fail too — that is
a different experiment from the system-proxy-only one, and both are recorded. Export the
variable in the same shell (or the same service environment) that starts the client, not
only in the shell that starts router.

The contract clause and its measurement are in
[`docs/spec.md` §5](../docs/spec.md); the captured evidence is in `AGENTS.md` and
`autowork/STATE.md`.

## Endpoints to point a client at

| Method | Path | What it is |
|---|---|---|
| POST | `/v1/chat/completions` | OpenAI chat completions |
| POST | `/v1/responses` | OpenAI responses |
| POST | `/v1/messages` | Anthropic messages |
| GET | `/health` | liveness, plus what this process actually loaded (never requires a token) |

The three protocol endpoints are equivalent: same decision pipeline, same accounting, and
each mirrors its own protocol's upstream semantics. Which one you use is your client's
choice — codex speaks `responses`, most other tools speak `chat`, and an Anthropic-shaped
client speaks `messages`. `GET /metrics` appears in the README's endpoint table and is a
planned surface, not a served one today: [`docs/spec.md` §9.3](../docs/spec.md) is the list of
what is served and what is not.

## Per-client setup

**codex** declares a custom model provider in `~/.codex/config.toml`:

```toml
model_provider = "router"
model = "coding-fast"                   # a route: provider/model or an alias from your config
model_catalog_json = "~/.codex/models.json"

[model_providers.router]
name = "router"
base_url = "http://127.0.0.1:8790/v1"
wire_api = "responses"                  # must equal the route's native protocol (see below)
env_key  = "ROUTER_TOKEN"
```

and a catalog entry for the slug in `~/.codex/models.json` — the copy-paste JSON (verified
against codex-cli 0.137.0) is in the [README quick start](../README.md#quick-start). Then:

```bash
export NO_PROXY=127.0.0.1,localhost
codex exec --skip-git-repo-check -C /tmp "Reply with the single word: pong" < /dev/null
```

`wire_api` here is the one easy mistake: it must equal the **provider's** `wire_api` for the
route you picked, because v0.1 serves only native routes — a chat wire against the example
roster's `deepseek` entry (native `responses`) answers `501 not_implemented`. Pick a
chat-native route from your roster if your codex build cannot speak `responses`. The
`--skip-git-repo-check` and `< /dev/null` flags and the `NO_PROXY` export are explained line
by line in the README quick start.

The **failover chain never crosses protocols either**: a `fallback` entry — or a plan family's `overflow`
route — whose `wire_api` is not your inbound protocol is skipped exactly as an entry the router holds no key
for is skipped, so your request can never be answered on a wire you did not ask for. When nothing in the
chain can serve your protocol you get `502 upstream_error` with `details.stage: "no_available_route"` and a
`details.skipped[]` list naming every candidate it refused and why; the refusal's frozen shape is
[`docs/spec.md` §8](../docs/spec.md).

**hermes** speaks the same native `responses` wire as codex through its codex transport.
The setup below was smoke-verified end to end (two turns in one session, the second a tool
call, real upstream); the commands and outputs are quoted from that run.

1. Keep hermes in an isolated `HERMES_HOME` so it never touches your default profile. The
   smoke used `/tmp/r11-smoke/hermes-home`; any directory you own works the same way. Write
   that home's `config.yaml`:

```yaml
model: {default: coding-fast, provider: router, base_url: http://127.0.0.1:8790/v1, api_mode: codex_responses}
custom_providers:
  - {name: router, provider: router, base_url: http://127.0.0.1:8790/v1, model: coding-fast,
     api_mode: codex_responses, api_key_env: ROUTER_TOKEN, key_env: ROUTER_TOKEN}
```

2. Put the router token in that home's `.env` **twice**, same value both times:
   `ROUTER_TOKEN=<token>` (what the two `*_env` keys above name) and `ROUTER_API_KEY=<token>`
   (the credential name hermes actually resolves — see the pitfall below).

3. Smoke it — one turn, then its continuation in the same session:

```bash
export NO_PROXY=127.0.0.1,localhost
HERMES_HOME=/tmp/r11-smoke/hermes-home hermes -z "Reply with the single word: pong"
# rc=0, stdout: pong

HERMES_HOME=/tmp/r11-smoke/hermes-home hermes --continue -z \
  "Now use the terminal tool to run the shell command: echo r11-hermes-tool . Then reply with exactly its stdout."
# rc=0, stdout: r11-hermes-tool
```

Two pitfalls are load-bearing (both were hit for real during the smoke):

- **`api_mode: codex_responses` must be written explicitly.** That is what makes hermes post
  `POST /v1/responses` — the same native path codex uses. For a loopback `base_url` hermes's
  URL detection returns nothing and the entry resolves to `chat_completions`, i.e. the wrong
  wire against a responses-native route.
- **`api_key_env` alone is not enough.** hermes resolves a custom provider's credentials as
  `<PROVIDER>_API_KEY` — here `ROUTER_API_KEY` — and refuses with
  `No usable credentials found for provider 'router'. Set RAMP_ROUTER_API_KEY, ROUTER_API_KEY.`
  when it cannot find it. `key_env: ROUTER_TOKEN` in the entry plus `ROUTER_API_KEY` holding
  the token in `.env` is the combination that reached 200.

What that run left in the trace, so you know the healthy shape of a hermes session:

- The session is hermes's own `prompt_cache_key`, `pck_`-prefixed (the run recorded
  `pck_495971920e8c2f23d2112597` for turn 1); router invents no key of its own. The key is
  content-derived, so it holds steady only while the prefix-bearing parts (instructions, tool
  list) stay stable — the continuation turn was recorded at `turn_index` 2 in that session
  with `prefix.continuity` 1.0, i.e. the prefix carried over intact.
- The first `hermes -z` turn is **two** requests, not one: the main conversation plus one
  small auxiliary call (240 input tokens in the run) in its own `pck_` session. Expect the
  extra session in the trace; it is not a misroute.
- The tool-call continuation turn was served almost entirely from the upstream prefix cache:
  10752 of 10861 input tokens cached (99.0%), `prefix.continuity` 1.0. For comparison, the
  same shape of turn on codex measured `prefix.continuity` 0.6923 in the same smoke. That
  number is attributed, not open: the interrupted predecessor request was ruled out at the
  byte level (it and the completed turn share an identical 18-block hash prefix), and the
  cause was codex itself changing its `tools` array within the session — block 9, a
  ~12-token tool definition, block hash `ad41a391…` → `7796b665…` — while with tool bytes
  stable the same comparison reads 1.0. Clients do change their tool definitions between
  turns. Judge continuity per client and per turn; do not read 1.0 as a constant.

The **chat wire is a different, unwitnessed cell**: switching hermes to
`api_mode: chat_completions` requires a chat-native route of your own — the example roster's
`deepseek` (which `coding-fast` points at) is responses-native, and chat against it answers
`501 not_implemented`. The round's smoke did send hermes down the chat wire against the
roster's two chat-native routes and witnessed **no 200**: the attempt ended `502` after
failover (primary `429 rate_limit` → fallback `401 auth`, cost 0) — local upstream
credentials and quotas, not the gateway. Until you have your own chat-native provider, treat
hermes-over-chat as untested here.

**claude code** keeps the earlier advice — same shape (override the base URL, keep the token
in the environment) — and was **not** part of the witnessed matrix above.

**What router does with the token a client sends it.** The token a client sends router is *not*
the credential router uses upstream. Router ignores the client's `Authorization` / `x-api-key`
header for provider calls: the credential it hands a provider is read from the environment of
**the router process**, from the variable that provider's `api_key_env` names — the config names
the variable, never the value. Inbound headers are read for one thing only, the session key
sources of your `session` config, and they are never forwarded upstream. The client's token
therefore has exactly one job: it is the client's key to **router itself**, and it only matters
once inbound auth is turned on (next section). With auth off, whatever token your client sends
is ignored.

Set the model field to an explicit `provider/model` or to an alias defined in your config. What you
write there is a **route, not a provider model name**: router resolves it and sends the provider its
own native model id, so `deepseek/deepseek-v4-pro` and an alias that points at it reach the upstream
identically. Your own string is not lost — the trace records it as `decision.requested_model`
alongside the native id it resolved to. Do **not** paste the bare native id into a client: it is not a
route and not an alias, so it resolves to nothing and comes back as `404 unknown_model`. `auto` is
deliberately not enabled in v0.1: it returns an explicit error instead of guessing, and the selector
slot is reserved for a plugin.

One shape deserves a warning because it looks like a typo and is not. Some vendors sell a 1M-context
variant of a model as a **separate model id with an `[1m]` suffix** — the shipped example roster has
such a plan entry, so its route really is `zai-plan/glm-5.3[1m]`, suffix and all. If your fingers keep
writing the bare name, point the client at the alias instead (`glm-plan` in the example config): the
alias and the full route string reach the upstream byte-identically, and the alias is the one that
does not move when the vendor renames the variant.

## Requiring a token (inbound auth)

Router can require a token from every client that talks to it. This is off unless you turn it on,
and it is a one-line change to your config plus one environment variable:

```bash
# 1. make a token (any 32 random bytes are enough; the value is a secret)
export ROUTER_TOKEN=$(openssl rand -hex 32)
```

```yaml
# 2. write server.auth_token_env in the router process's config: it names the
#    variable whose value is the expected token — the name, never the value
server: { addr: "127.0.0.1:8790", upstream_attempt_timeout: 60s, request_timeout: 10m,
          auth_token_env: ROUTER_TOKEN }
```

3. Give the same value to each client. Both header forms are accepted, so either works:

```bash
curl -s http://127.0.0.1:8790/v1/chat/completions \
  -H "Authorization: Bearer $ROUTER_TOKEN" -H 'content-type: application/json' \
  -d '{"model":"<provider>/<model>","messages":[{"role":"user","content":"hi"}]}'

curl -s http://127.0.0.1:8790/v1/chat/completions \
  -H "x-api-key: $ROUTER_TOKEN" -H 'content-type: application/json' \
  -d '{"model":"<provider>/<model>","messages":[{"role":"user","content":"hi"}]}'
```

For codex and hermes the token goes in the same variable their `env_key` names (the snippet above
uses `ROUTER_TOKEN`) — they send it as `Authorization: Bearer <token>`. Note the endpoint in
these examples is the chat one: pair it with a chat-native route from your roster (the example
file's `zai` or `kimi` entries), or use `/v1/responses` for a responses-native route like
`coding-fast` — v0.1 refuses the cross-protocol cell with `501 not_implemented` (see
[Protocols](protocols.md)).

Four things worth knowing before you rely on this:

- **`GET /health` never needs a token.** It is the liveness probe; keep it token-free so your
  supervisor (and your own `curl`) can see whether the process is up. Its `auth` member also tells
  you which mode this process is in (`required`, with the variable's name, or nothing required).
- **A wrong or missing token is `401 unauthorized`**, in the same error shape as every other refusal
  ([`docs/spec.md` §8](../docs/spec.md)). Nothing was sent upstream, so a `401` tells you about your
  client's setup and never about a provider.
- **Writing the key and leaving the variable unset is a startup refusal, not a quiet downgrade.**
  If `ROUTER_TOKEN` is empty or missing in the router process's environment, `router serve` exits
  with a non-zero code and says which variable it wanted. That is deliberate: a gateway that answers
  "I could not find your token, so I skipped the check" is worse than one that refuses to start.
- **No key at all = local, unauthenticated mode** — today's behaviour, and what a single-user
  localhost setup wants. Nothing changes for existing configs.

To rotate the token, change the value in the environment and restart the process; the token is read
once at startup.

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
   the report names the plugins, the providers whose keys are present, whether inbound auth is
   required (its `auth` member), and the resolved state path you expect. With auth on, check the
   inverse in the same breath: `/health` still answers **without** a token, and a protocol endpoint
   without one comes back `401 unauthorized`.
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
[Protocols](protocols.md)). The proxy prerequisite above is the one you need before any of it can
reach router at all; the token (if you turned inbound auth on) is what gets a request through the
front door.

## Authoritative sources

- [`docs/spec.md` §5](../docs/spec.md) — the access prerequisite, as a contract clause with
  the measurement behind it, and §5.1 for the token side of it.
- [`docs/spec.md` §2](../docs/spec.md) — the inbound endpoint per protocol and the outbound
  selection rule.
- [`docs/spec.md` §3](../docs/spec.md) — model selection semantics (`provider/model`,
  alias, `auto`).
- [`docs/spec.md` §4.7](../docs/spec.md) — inbound token auth: the config key, the two accepted
  header forms, the startup refusal when the environment variable is missing, the `/health`
  exemption, and what a refused request leaves in the trace.
- [`docs/spec.md` §4.1](../docs/spec.md) and [§4.5](../docs/spec.md) — where traces and the
  local store live, and how a relative path is resolved.
- [`docs/spec.md` §6](../docs/spec.md) — the observation contract, including what the trace
  records about a session and a turn.
- [`design/DESIGN.md` §12.11](../design/DESIGN.md) — where the guard, the comparison and the
  refused request's trace record land in the code.
- [`design/DESIGN.md` §12.10](../design/DESIGN.md) — the data plane and storage landing
  (streaming behaviour, config resolution, store).
- [`README.md`](../README.md) — copy-pasteable client snippets and the `NO_PROXY` line.
- [`AGENTS.md`](../AGENTS.md) — the recorded localhost/proxy incident and the stateless
  client-traffic evidence.
