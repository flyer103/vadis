# Protocols

Status: outline only. The wire contract — including every lossy point — is normative in
`docs/spec.md` §2; this chapter explains it to a user, it does not restate it.

router speaks the three protocols agents actually use, on both the inbound and the
outbound side, and it prefers to pass bytes through untouched.

## Outline

- **Three inbound protocols**: OpenAI chat completions, OpenAI responses, Anthropic
  messages — equivalent semantics, one decision pipeline behind all three.
- **Endpoints and shapes** you point clients at, and the health/telemetry endpoints next
  to them.
- **Native passthrough first**: when the inbound protocol matches the provider's declared
  wire API, the request body is forwarded as original bytes, with only router-owned fields
  removed — message content, order, whitespace and tool schemas are never rewritten.
- **Deterministic translation second**: when inbound and outbound protocols differ, the
  translation is a pure function of content and stable config — the same content always
  produces the same upstream bytes, so prefix caching survives across turns.
- **The lossy points** translation must handle explicitly (system instructions, tool calls
  and results, reasoning content, cache breakpoints, usage shape) — with the rule that
  anything dropped is recorded as lossy rather than silently re-encoded.
- **Capability declaration**: each provider declares which inbound protocols it supports;
  an undeclared combination is refused with an explicit error instead of a best-effort
  translation.
- **Streaming**: server-sent events are proxied, and the router's own response headers are
  emitted before the first event.
- **Unknown fields pass through**: protocol evolution is tolerated, never silently
  rewritten.

## Authoritative sources

- [`docs/spec.md` §2](../docs/spec.md) — the protocol contract and the lossy-point table.
- [`docs/spec.md` §8](../docs/spec.md) — unified error body and the error-type-to-HTTP
  table (including capability failures).
- [`design/DESIGN.md` §7](../design/DESIGN.md) — the translation layer as designed.
- [`design/decisions/ADR-004-protocol-passthrough-safe-translation.md`](../design/decisions/ADR-004-protocol-passthrough-safe-translation.md)
  — why native passthrough wins and what "deterministic translation" obliges.
- [`tests/conformance/`](../tests/conformance) — the 3×3 protocol matrix that enforces
  byte equivalence and prefix stability.
