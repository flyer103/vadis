# ADR-004 — native passthrough first; deterministic cross-protocol translation; v0.1 does no server-side state

- Status: accepted
- Date: 2026-09-19

## Background

The requirement is to support 3 protocols (OpenAI chat completions / OpenAI responses / Anthropic messages).
At the same time "taking care of the prompt cache" requires the prefix bytes the upstream sees to be stable
across turns. If the translation layer has no determinism constraint, it becomes the biggest cache-breaking
source. We must also answer: is server-side session state (`store:true` / `previous_response_id`) needed?

## Decision

1. **native passthrough first**: when the inbound protocol == the provider's `wire_api`, the only permitted
   mutation is deleting vadis-owned fields; every other byte is forwarded as is.
2. **Cross-protocol translation must be deterministic**: a mapper is a pure function of
   `(content, stable config)`, and the same content always produces the same upstream bytes; lossy points
   must be registered explicitly (the list in spec §2) and written into the trace.
3. **v0.1 does no server-side state**: an inbound `store:true` or a non-empty `previous_response_id` →
   **sticky routing to the same (provider, model)** for fidelity + the trace marks `stateful_inbound`; only
   when stickiness cannot be guaranteed is it a 400.

## Evidence (measured, 2026-09-19)

A local packet capture of codex CLI 0.137 (`wire_api="responses"` → ZAI), one complete round including one
tool call:

| Observation | Value |
|---|---|
| request body top-level fields | `client_metadata, include, input, model, parallel_tool_calls, prompt_cache_key, reasoning, store, stream, tool_choice, tools` |
| `store` | `false` (both rounds) |
| `previous_response_id` | absent in both rounds |
| `input` length | 3 items (first round) → 6 items (after the tool call) → **full resend** |
| `prompt_cache_key` | identical in both rounds (= session id), the upstream response echoes the field |
| upstream cache | `cached_tokens` 960/14409 → 14400/14520 (99.2%) |

The static evidence agrees: `codex-rs/core/src/client.rs` hard-codes `store: false` and the HTTP path sets
no `previous_response_id` (that field belongs only to the WebSocket incremental transport and remote
compaction); `hermes/agent/transports/codex.py` likewise has `"store": False` + `prompt_cache_key=session_id`.

## Consequences

- The cost of the missing server-side state is extremely low, while the complexity of implementing it
  (session table, invalidation, concurrency, cross-provider semantic differences) is high; that complexity
  is deferred until there is a measured need (the `stateful_inbound_rate` metric keeps monitoring it).
- `prompt_cache_key` is promoted to a first-class source of session identity, used for the sticky table and
  the cache-ledger key.
- Clients resend everything ⇒ the fate of the prefix cache is decided entirely by vadis's transforms ⇒
  `prefix_continuity` becomes a blocking gate.
- If codex switches to the WebSocket transport, the HTTP-only v0.1 depends on its fallback to HTTP (the
  client already has `fallback_to_http`); this is recorded as a future observation item.
