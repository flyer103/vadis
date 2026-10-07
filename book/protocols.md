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

## A declared cell is a native cell

Each provider entry declares two things:

- **`supports`** — the set of inbound protocols vadis serves **to that provider**. Every member
  of this set is a **native cell**: the client's own bytes are posted to that cell's own
  complete endpoint (the entry's `urls`), with no translation, re-encoding or re-ordering. The
  three protocols above are the values this set can hold, and each declared cell carries its
  own URL — a declared cell with no endpoint is a load error.
- **`wire_api`** — the entry's *native* protocol: the single cell it treats as its diagonal.
  It is a **declaration about the entry, not a gate**: it restricts nothing. Config validation
  requires only that it be one of the cells the entry declares, so an entry that serves three
  protocols still names one of them as its native form. Everything a reader once derived from
  "the inbound protocol equals `wire_api`" is now read from `supports`.

A request whose inbound protocol is outside a provider's `supports` is a cell of a provider
that does not exist, and vadis never invents one. The answer depends on **who asked**:

- for the route the **client named**, an explicit error — `400`,
  `error.type: capability_unsupported`;
- for a **candidate the client did not name** — a plan family's route, or a `fallback` entry —
  the candidate is **skipped** exactly as an entry vadis holds no key for is skipped, and the
  walk continues. Your request can therefore never be answered on a wire you did not ask for;
  when no candidate can serve it, the request is refused in the shape
  [`docs/spec.md` §8](../docs/spec.md) freezes (`502 upstream_error`,
  `details.stage: "no_available_route"`).

There is no best-effort translation for an undeclared cell: declaring a format your provider
does not document is how a gateway starts inventing wire formats, and "it usually works" is
not a contract.

The shipped example roster (`providers.example.yaml`, named by the shipped config — that pair
is the file to read for the current declarations) illustrates the rule: each entry's `supports`
lists exactly the wire forms its vendor documents, and each listed cell has its own endpoint
in the entry's `urls`. The DeepSeek entry documents all three forms, so it declares all three
cells and answers on any of the three endpoints. Where a cell is not listed, it does not exist
for that entry — vadis answers the `400` above rather than guessing a wire format the vendor
never published.

## Native passthrough, on every declared cell

Whenever the inbound protocol is one the provider entry declares, the request body is
forwarded as the client's original **bytes**, posted to that cell's own complete URL. Exactly
two mutations are permitted, both byte-level span edits, never a parse-and-reprint of the body
(that is the most common way a gateway quietly changes what the provider sees):

- **removing vadis-owned fields** (the `vadis_meta` echo and routing hints), against a
  whitelist constant;
- **replacing the value of the top-level `model` field** with the provider's own model id —
  the route you asked for, spelled the way that provider's API expects it. Message content,
  ordering, whitespace, tool schemas and unknown fields are never rewritten.

You can rely on the byte boundary in a checkable form: after those two edits, the remaining
bytes are byte-for-byte the client's, and neither edit changes a prefix block's hash (the
`model` field is not part of the prefix, and vadis-owned fields are not either).

## No translation: a cell is served, or it does not exist

vadis ships no content translator. A cell is served **iff** the entry declares it, and the
bytes that go out on that cell are the client's own bytes. Nothing on the serving path
re-encodes a body, so `protocol.translated` is `false` and `protocol.lossy` is `[]` on every
record this build writes — and `protocol_out` equals `protocol_in` **by construction**, because
a route is only ever attempted on a cell it declares.

A deterministic content mapper — the design's second mode, which would let a client be served
on a cell its provider does not declare — is **not implemented in v0.1**. It remains the
contract those future cells are held to, written here and in
[`docs/spec.md` §2](../docs/spec.md) so that the first one to land has something to satisfy:

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
translation cell. None of the lines above describes anything this build does; each describes
a mapper a translation cell would need.

## Streaming

Server-sent events are relayed, not re-interpreted. Vadis forwards the upstream's bytes in
order, flushes each event as it arrives, and writes the three `X-Vadis-*` headers before
the first event. It does not re-frame the stream, reorder or normalize `event:`/`data:`
lines, and it does not add a terminal marker of its own.

Three consequences worth knowing before you point an agent at it:

- **The relay is bounded twice, and neither bound is a stall.** The configured **upstream attempt
  timeout** bounds two things on the relay: how long the upstream may take to produce its response
  **head**, and the **gap** between upstream bytes, one read at a time. A stream that is *continuously
  busy* is **not** cut off by it — a long reasoning stream now runs to completion as long as it is
  never idle for longer than that timeout. What ends a busy stream is the **whole-request** bound,
  `server.request_timeout`: it caps the entire request, the head phase and the relay together. Either
  way the client sees a truncated stream rather than a hang, and **which knob to raise depends on the
  symptom** — a stream that dies while it was still sending bytes has outrun the **request timeout**;
  one that dies mid-body after going quiet has outrun the **upstream attempt timeout** for its
  provider's gaps. The exact rule is [`docs/spec.md` §4.2](../docs/spec.md) and DESIGN §12.10.3 R4
  ([`ADR-053`](../design/decisions/ADR-053-the-bound-is-head-arrival-only.md)).
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
- [`docs/spec.md` §4.2](../docs/spec.md) — the candidate walk's eligibility rule: a candidate
  is attempted only on a cell it declares.
- [`design/DESIGN.md` §7](../design/DESIGN.md) — the translation layer as designed.
- [`design/DESIGN.md` §12.10](../design/DESIGN.md) — the data plane: provider adaptation and
  the byte-level requirements of the streaming path.
- [`design/decisions/ADR-051-a-declared-cell-is-native.md`](../design/decisions/ADR-051-a-declared-cell-is-native.md)
  — why every declared cell is native, why `wire_api` names the diagonal and gates nothing,
  and why an undeclared cell is a `400` (or a skip), never a translation.
- [`design/decisions/ADR-004-protocol-passthrough-safe-translation.md`](../design/decisions/ADR-004-protocol-passthrough-safe-translation.md)
  — why native passthrough wins and what "deterministic translation" obliges.
- [`design/decisions/ADR-007-span-faithful-forwarding.md`](../design/decisions/ADR-007-span-faithful-forwarding.md)
  — the byte boundary as a type-level rule.
- [`tests/conformance/`](../tests/conformance) — the 3×3 protocol matrix that enforces byte
  equivalence and prefix stability.
