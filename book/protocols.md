# Protocols

Status: written for v0.1. The wire contract — including every lossy point — is normative in
`docs/spec.md` §2; this chapter explains it to a user and does not restate it.

router speaks the three protocols agents actually use, on both the inbound and the outbound
side, and it prefers to pass bytes through untouched. Which combinations are legal is not a
guess: your provider roster declares it.

## The three inbound protocols

OpenAI chat completions, OpenAI responses and Anthropic messages. All three enter the same
decision pipeline — one session resolution, one transform chain, one selection, one guard
chain, one accounting path — and each mirrors its own protocol's semantics on the way out.

## What "supported" means, and what an undeclared cell does

Each provider entry declares two things:

- **`wire_api`** — the provider's *native* wire format. This is the format in which router
  can forward the request as the client's own bytes.
- **`supports`** — the set of inbound protocols router may serve *to that provider*,
  including `wire_api` itself. These are the legal cells of the 3×3 inbound × outbound
  matrix.

A request whose inbound protocol is outside a provider's `supports` is **refused with an
explicit error** (`400`, `error.type: capability_unsupported`). There is no best-effort
translation for an undeclared cell: declaring a format your provider does not document is
how a gateway starts inventing wire formats, and "it usually works" is not a contract.

The shipped example roster (in `config.example.yaml`, which is the file to read for the
current declarations) is a useful illustration of the rule: the DeepSeek entry declares the
`responses` wire format with all three inbound protocols supported, while the other entries
declare `chat` as their wire format with a narrower `supports` set — so a `responses`
inbound request aimed at one of those is refused rather than translated on a hunch.

## Native passthrough first

When the inbound protocol matches the provider's `wire_api`, the request body is forwarded
as the client's original **bytes**. The only permitted mutation is removing router-owned
fields (the `router_meta` echo and routing hints): message content, ordering, whitespace,
tool schemas and unknown fields are never rewritten. Concretely, the deletion is a
byte-level span edit against a whitelist constant — never a parse-and-reprint of the body,
because that is the most common way a gateway quietly changes what the provider sees.

You can rely on the byte boundary in a checkable form: after removing router-owned fields,
the remaining bytes are byte-for-byte the client's, and the field removal never changes a
prefix block's hash.

## Deterministic translation second

When inbound and outbound protocols differ, the translation is a pure function of content
and stable config: the same content always produces the same upstream bytes, and nothing
depends on wall-clock time, turn number or randomness. That determinism is what keeps the
upstream prefix cache alive across turns — a translation that reshuffles its output between
turns destroys the cache and *raises* cost. (It is also why a rule or translation that is
not deterministic is a defect, not a tuning question.)

The lossy points a translation must handle explicitly, one by one:

- **system instructions** — kept at a stable position at the very front of the prefix;
- **tool calls and tool results** — mapped both ways with ids and order preserved;
- **reasoning content** — passed through where the target format has a slot for it;
  re-encoding it is forbidden, and dropping it is permitted only as a *marked* loss;
- **cache breakpoints** — injected on the target's own signal, by a stable rule;
- **usage shape** — normalized into router's internal usage so numbers stay comparable.

Whenever something is dropped, the response carries the `X-Router-Lossy` header and the
decision record lists it. Silent loss is the one outcome that is not allowed.

## Streaming

Server-sent events are relayed, not re-interpreted. Router forwards the upstream's bytes in
order, flushes each event as it arrives, and writes the three `X-Router-*` headers before
the first event. It does not re-frame the stream, reorder or normalize `event:`/`data:`
lines, and it does not add a terminal marker of its own.

Two consequences worth knowing before you point an agent at it:

- **A stall is bounded.** A gap with no upstream bytes longer than the configured upstream
  attempt timeout ends the relay, and the client sees a truncated stream rather than a
  hang.
- **A failure after the first event cannot be failed over.** Once events have reached the
  client, the output is committed; router terminates the stream using the protocol's own
  in-band failure shape where one exists, and otherwise simply ends it without the
  protocol's terminal marker. It never fabricates a successful ending.
- **If you disconnect, router stops the upstream call.** It does not keep draining a stream
  nobody is reading — that would bill tokens with no reader. If the response never
  completed, the request is recorded as an outcome router cannot verify (see
  [Operations](operations.md)).

## Errors and evolution

Every non-2xx answer — including the stub endpoints and the capability refusal above —
uses one error body shape (type, message, request id, optional details), so a client never
has to parse a provider's own error prose. The type-to-HTTP table lives in
[`docs/spec.md` §8](../docs/spec.md).

Unknown fields are passed through verbatim, on the way in and on the way out. Protocol
evolution is tolerated: adding a field to your client's request does not require a router
change, and router never silently drops what it does not recognize.

## Authoritative sources

- [`docs/spec.md` §2](../docs/spec.md) — the protocol contract, the outbound selection rule
  and the lossy-point table.
- [`docs/spec.md` §8](../docs/spec.md) — the unified error body and the type-to-HTTP table
  (including capability failures and response headers).
- [`design/DESIGN.md` §7](../design/DESIGN.md) — the translation layer as designed.
- [`design/DESIGN.md` §12.10](../design/DESIGN.md) — the data plane: provider adaptation and
  the byte-level requirements of the streaming path.
- [`design/decisions/ADR-004-protocol-passthrough-safe-translation.md`](../design/decisions/ADR-004-protocol-passthrough-safe-translation.md)
  — why native passthrough wins and what "deterministic translation" obliges.
- [`design/decisions/ADR-007-span-faithful-forwarding.md`](../design/decisions/ADR-007-span-faithful-forwarding.md)
  — the byte boundary as a type-level rule.
- [`tests/conformance/`](../tests/conformance) — the 3×3 protocol matrix that enforces byte
  equivalence and prefix stability.
