# Security Policy

## Reporting a vulnerability

**Do not open a public issue for a security problem.** Report it privately through GitHub's
private vulnerability reporting: open the repository's **Security** tab and click
**Report a vulnerability** (the form is also at
<https://github.com/flyer103/router/security/advisories/new>). If that button is not offered,
open an issue whose *only* content is "security report — please enable private vulnerability
reporting", and wait; do not describe the problem in the issue.

A report we can act on contains: the vadis version or the commit hash, the relevant config
shape with every key value redacted, the smallest request that triggers the problem, and what
you observed. This is a small project without paid support, so reports are answered on a
best-effort basis.

## Supported versions

| Version | Supported |
|---|---|
| the latest `v0.1.x` release, and `main` | yes |
| anything older than the latest `v0.1.x` | no |

Fixes land on `main` and ship in the next `v0.1.x`. The project is pre-1.0: do not expect
backports to older tags.

## Threat model (one line)

`vadis` is a **local process**: it listens on the port your config names, it holds your
provider API keys (named in the config as environment variables, read from the environment at
startup, never written to disk), and it **forwards the client's own request bytes** to the
resolved upstream. Reaching the listening port is therefore gated by **inbound auth**: unless
`server.auth_token_env` names an environment variable whose value the request must present
(as `Authorization: Bearer …` or `x-api-key: …`, compared in constant time against a token
read once at startup), the three protocol endpoints answer `401` and reach no upstream — so a
caller without the token cannot spend your provider quota. With the key absent (the local
single-user mode) the gateway serves unauthenticated and that exposure returns: any caller
that can reach the port can spend quota. Anything that can read the process environment,
the local state database or the trace output can see your request metadata.

In scope: the data plane (byte handling, credential handling, route resolution), the local
store and the trace sink, the inbound-auth boundary (spec §4.7: constant-time comparison,
`GET /health` the one structurally exempt endpoint, the token read once at startup and
rotated only by restart, its value never printed or traced), and anything that makes the
above worse — a request that leaks the provider key **or the inbound token** into a log, a
trace record, an error body, or the upstream.

Out of scope: the provider's own handling of your data (report that to the provider), and a
deliberately exposed port or a config file shipped somewhere you do not control.

## What the local artifacts contain

- Provider keys are read from the environment only; the config file names the variable.
- Trace records and the SQLite state file hold **metadata** — route, model, usage, cost,
  prefix continuity — and no request or response body (README "Ops"; `docs/spec.md` §4.5;
  ADR-009). Treat them as sensitive anyway: they reveal what you were working on.
