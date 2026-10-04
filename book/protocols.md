# Protocols

Status: written for v0.1. The wire contract — including every lossy point — is normative in
`docs/spec.md` §2; this chapter explains it to a user and does not restate it.

vadis speaks the three protocols agents actually use, on both the inbound and the outbound
side, and it prefers to pass bytes through untouched. Which combinations are legal is not a
guess: your provider roster declares it.

## The three inbound protocols

OpenAI chat completions, OpenAI responses and Anthropic messages. All three enter the same
decision pipeline — one session resolution, one transform chain, one selection, one guard
chain, one accounting path — and each mirrors its own protocol's semantics on the way out.

## What "supported" means, and what an undeclared cell does

Each provider entry declares two things:

- **`wire_api`** — the provider's *native* wire format. This is the format in which vadis
  can forward the request as the client's own bytes.
- **`supports`** — the set of inbound protocols vadis may serve *to that provider*,
  including `wire_api` itself. These are the legal cells of the 3×3 inbound × outbound
  matrix.

A request whose inbound protocol is outside a provider's `supports` is **refused with an
explicit error** (`400`, `error.type: capability_unsupported`). There is no best-effort
translation for an undeclared cell: declaring a format your provider does not document is
how a gateway starts inventing wire formats, and "it usually works" is not a contract.

The shipped example roster (`providers.example.yaml`, named by the shipped config — the pair is the file to
read for the current declarations) is a useful illustration of the rule: the DeepSeek entry declares the
`responses` wire format with all three inbound protocols supported, while the other entries
declare `chat` as their wire format with a narrower `supports` set — so a `responses`
inbound request aimed at one of those is refused rather than translated on a hunch.

## Native passthrough first

When the inbound protocol matches the provider's `wire_api`, the request body is forwarded
as the client's original **bytes**. Exactly two mutations are permitted, both byte-level span
edits, never a parse-and-reprint of the body (that is the most common way a gateway quietly
changes what the provider sees):

- **removing vadis-owned fields** (the `vadis_meta` echo and routing hints), against a
  whitelist constant;
- **replacing the value of the top-level `model` field** with the provider's own model id —
  the route you asked for, spelled the way that provider's API expects it. Message content,
  ordering, whitespace, tool schemas and unknown fields are never rewritten.

You can rely on the byte boundary in a checkable form: after those two edits, the remaining
bytes are byte-for-byte the client's, and neither edit changes a prefix block's hash (the
`model` field is not part of the prefix, and vadis-owned fields are not either).

## Deterministic translation second

Translation is the design's second mode, and it is **not implemented in v0.1**: vadis
serves only the native diagonal of the 3×3 matrix — the cells where the inbound protocol
is the provider's own `wire_api`. The other six cells refuse with a typed
`501 not_implemented` whose message names the cell that is missing. What follows is the
contract those cells are held to when one lands: it is written here and in
[`docs/spec.md` §2](../docs/spec.md), and no code path implements it yet.

When a translation cell lands, the translation must be a pure function of content and
stable config: the same content always produces the same upstream bytes, and nothing may
depend on wall-clock time, turn number or randomness. That determinism is what keeps the
upstream prefix cache alive across turns — a translation that reshuffles its output
between turns destroys the cache and *raises* cost. (It is also why a rule or translation
that is not deterministic is a defect, not a tuning question.)

The lossy points a translation must handle explicitly, one by one:

- **system instructions** — kept at a stable position at the very front of the prefix;
- **tool calls and tool results** — mapped both ways with ids and order preserved;
- **reasoning content** — passed through where the target format has a slot for it;
  re-encoding it is forbidden, and dropping it is permitted only as a *marked* loss;
- **cache breakpoints** — injected on the target's own signal, by a stable rule;
- **usage shape** — normalized into vadis's internal usage so numbers stay comparable.

Whenever a translation drops something, the response must carry the `X-Vadis-Lossy`
header and the decision record must list it; silent loss is the one outcome that is not
allowed. No v0.1 code path emits that header — it becomes real together with the first
translation cell.

## Streaming

Server-sent events are relayed, not re-interpreted. Vadis forwards the upstream's bytes in
order, flushes each event as it arrives, and writes the three `X-Vadis-*` headers before
the first event. It does not re-frame the stream, reorder or normalize `event:`/`data:`
lines, and it does not add a terminal marker of its own.

Two consequences worth knowing before you point an agent at it:

- **The relay is bounded, not just a stall.** The configured upstream attempt timeout bounds the
  whole attempt on elapsed time, so both a gap with no upstream bytes *and* a stream that keeps
  sending bytes but runs past that timeout end the relay: the client sees a truncated stream rather
  than a hang. Point a slow, long-running response at it — a long reasoning stream is the obvious
  case — and it is this timeout you will meet; raise it if that is the shape you serve. The exact
  rule is [`docs/spec.md` §4.2](../docs/spec.md) and DESIGN §12.10.3 R4
  ([`ADR-044`](../design/decisions/ADR-044-the-bound-is-total-elapsed-time.md)).
- **A failure after the first event cannot be failed over.** Once events have reached the
  client, the output is committed; vadis terminates the stream using the protocol's own
  in-band failure shape where one exists, and otherwise simply ends it without the
  protocol's terminal marker. It never fabricates a successful ending.
- **If you disconnect, vadis stops the upstream call.** It does not keep draining a stream
  nobody is reading — that would bill tokens with no reader. If the response never
  completed, the request is recorded as an outcome vadis cannot verify (see
  [Operations](operations.md)).

## Errors and evolution

Every non-2xx answer — including the stub endpoints and the capability refusal above —
uses one error body shape (type, message, request id, optional details), so a client never
has to parse a provider's own error prose. The type-to-HTTP table lives in
[`docs/spec.md` §8](../docs/spec.md).

Unknown fields are passed through verbatim, on the way in and on the way out. Protocol
evolution is tolerated: adding a field to your client's request does not require a router
change, and vadis never silently drops what it does not recognize.

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
