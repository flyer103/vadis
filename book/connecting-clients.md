# Connecting clients

Status: outline only. This chapter is the onboarding path for real clients; the detailed
contracts it references live in `docs/spec.md`.

## Prerequisite: let localhost bypass the system proxy

Do this **before** pointing any client at router:

```bash
export NO_PROXY=127.0.0.1,localhost
```

On macOS, clients that read the system proxy settings (`reqwest` in codex, `httpx` in
hermes) do **not** honour the proxy exclusion list for `127.0.0.1`. When the system proxy
is configured, requests to a locally bound router are sent into the proxy instead, the
router receives no connection at all, and the client reports `503 Service Unavailable`.
The symptom looks like "router is down"; the cause is the proxy. Verified behaviour and
the codex log line are recorded in `AGENTS.md` and `autowork/STATE.md`.

## Outline

- **The proxy prerequisite above** — the single most common reason a local gateway appears
  broken, and the one step that must be done first.
- **codex**: declare a custom model provider with `base_url`, `wire_api` (`responses` or
  `chat`) and the environment variable holding the router token; the request carries the
  client's session identity automatically.
- **hermes / claude code**: same shape — override the base URL, keep the bearer token in
  the environment.
- **Choosing a model**: `provider/model`, a configured alias, or `auto` (which v0.1
  rejects with an explicit error rather than guessing).
- **What you get back**: the upstream response plus the `router_meta` block — plugin chain
  and per-step transform accounting, resolved session, cache state.
- **Session identity**: taken from the client's own cache key when present, so sessions
  line up with the upstream's own caching; do not invent a session id when the client
  already sends one.
- **Smoke checklist per client**: one full session, watch `router stats` for the session,
  confirm the first request costs a miss and the following ones hit the cache.
- **Never touch content on the way through**: the proxy is byte-faithful, so a client that
  works directly against the provider works through router.

## Authoritative sources

- [`docs/spec.md` §5](../docs/spec.md) — the access prerequisite, as a contract clause with
  the measurements behind it.
- [`docs/spec.md` §3](../docs/spec.md) — model selection semantics (`provider/model`,
  alias, `auto`).
- [`docs/spec.md` §2](../docs/spec.md) — inbound endpoints per protocol.
- [`AGENTS.md`](../AGENTS.md) — the recorded localhost/proxy incident and the stateless
  client-traffic evidence.
- [`README.md`](../README.md) — copy-pasteable client snippets and the `NO_PROXY` line.
