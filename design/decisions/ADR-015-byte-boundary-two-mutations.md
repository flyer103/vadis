# ADR-015 — the byte boundary permits exactly two mutations: the second is replacing the *value* of the top-level `model` member with the route's native id

- Status: accepted
- Date: 2026-09-20
- Related: AGENTS hard constraints 1 (byte boundary) and 2 (content determinism); spec §2 (protocol contract / the two-mutation table) / §3 (route names) / §6 (`decision.*`); DESIGN §6 clause 1, §12.3.1 (the byte primitives), §12.10.7 (the landing and the supersession pointer); ADR-004 (native passthrough first), ADR-007 (span-faithful forwarding — item 2 superseded in one respect), ADR-012 (the mutable scope); CONF-27

## Background

v0.1's byte boundary (ADR-004 line 16, ADR-007 item 2) permitted **one** mutation: deleting top-level
router-owned fields (`router_meta` echo, routing hints). The client's `model` string travelled upstream
verbatim.

That is not a boundary a real provider can be served through. A client's `model` is a **route name** —
`provider/model` or an alias (spec §3) — and the alias space is the router's own vocabulary: the routing
abstraction, the per-provider roster and the alias indirection all exist so that the *client* does not
have to know which provider id a route resolves to (ADR-004). Sending the route name upstream asks the
provider to resolve a name only this process understands; DESIGN §12.10.7 records the resulting provider
`400 format_error` measured with a live client against deepseek and zai, for a body whose only defect was
the outbound `model` string.

So before Round 2 could serve anything, the contract had to answer a question it had left open — where
does the outbound `model` name come from — and the owner answered it in the contract itself (AGENTS
constraint 1, commit `3074957`): routing resolves first, the upstream is called with the route's native
model id alone, and the client's own string is kept as `decision.requested_model`. spec §2 (`52ecb5f`)
wrote the two-mutation table; DESIGN §12.10.7 stated where the rewrite is applied and carries the
supersession pointer for ADR-007. The decision log itself had no entry for it: appended history still
said "the only permitted rewrite = deleting top-level router-owned fields", which is now false. This ADR
closes that hole without editing either historical ADR.

## Decision

1. **The byte boundary permits exactly two mutations**, both byte-level, both scoped, both auditable:
   **(a)** deleting router-owned top-level members (whole members with their separators; invariant: the
   output is valid JSON and every retained member is byte-for-byte its input span); **(b)** replacing the
   **value** of the top-level `model` member with the resolved route's provider-native model id — the
   member's *value span only*: its key, its position, the separators and whitespace around it, and every
   other byte are the client's.
2. **Routing resolves before anything leaves the process.** The client's string never reaches a provider;
   it is recorded in the trace as `decision.requested_model` (present-and-`null` when the request carried
   none). Two client-side spellings that resolve to the same route produce **byte-identical** upstream
   requests, which is what makes the routing abstraction invisible to the prefix cache (CONF-27).
3. **Mutation (b) is a span replacement, never a re-encoding**, and it is a **replacement, never an
   insertion**: an absent `model` member is an error, not a fabricated member, and a non-string value is
   an error, not a coercion. Failure to produce the bytes fails the request (`500 internal`, stage
   `encode`); it must **never** fall back to forwarding the client's string — that is the defect this
   clause exists to remove. The reachable path cannot take that branch: a body whose `model` is missing or
   non-string is answered `400 invalid_request` before selection, and a resolved route always carries an
   id.
4. **The rewrite is per attempt and a pure function of (inbound bytes, route)** — no clock, no turn
   number, no RNG. The same inbound bytes plus the same route always yield the same outbound bytes, which
   is what keeps the outbound prefix stable across the turns of a session even though mutation (b) changes
   the body's length relative to the client's own bytes (AGENTS constraint 2: determinism, not
   immutability, is what the cache needs). In a fallback chain the rewrite sits **inside** the attempt
   loop, so each attempt's `upstream.submitted.body_hash` is that attempt's byte-final body.
5. **It is deliberately the smallest possible second mutation.** The reviewer's checklist for the
   passthrough path is "two mutations"; a third is a change to AGENTS constraint 1, which no round may
   make on its own (ADR-012).
6. **`model` is not part of the prefix domain** (spec §6: `messages` / `input` / `tools` plus the
   system-instruction position), so mutation (b) moves no block hash: `prefix_blocks[]` and
   `prefix_continuity` keep measuring conversation fidelity, not routing.
7. **The translated path is bound by the same rule.** A translated cell's encoder must emit the route's
   model id as well; mutation (b) is the native path's form of it (lands with the translation matrix,
   R2-3/R3; the native path is asserted by CONF-27).

## Alternatives considered

- **Require clients to send provider-native ids and keep the boundary at one mutation** — rejected: it
  deletes `/v1` routing, aliases and the per-provider roster from the client's view, i.e. the very
  abstraction the gateway exists to provide, and it puts each provider's id space into every client's
  configuration.
- **Parse → reserialize the body with `model` swapped** — rejected: it rewrites field order, escapes,
  numeric literals and whitespace, so every prefix byte changes and the upstream prompt cache for the
  whole conversation is destroyed (the finding ADR-007 is built on), and `body_hash` / `prefix_blocks[]`
  stop being identities of the request.
- **Carry the model in a provider-specific header or URL path instead of the body** — rejected: for the
  three wire APIs in scope the model is a body member; a header would leave the body naming a route the
  upstream rejects.
- **Rewrite every occurrence of the key `model` (nested ones included)** — rejected: it would corrupt
  user content that legitimately contains a `model` key and it destroys the auditability property
  ("compare two spans, not two documents").
- **Let the provider resolve the alias** — rejected: providers do not know our alias space.
- **Record the change only in DESIGN §12.10.7** (where the supersession pointer already lives) — rejected
  as the sole record: `design/decisions/` is the append-only decision log, and a reader of it alone would
  still conclude the boundary permits one mutation. §12.10.7 keeps the pointer; this entry is its
  decision-log counterpart.

## Rationale

- **Two mutations is the smallest change that makes real providers usable.** One mutation cannot serve a
  provider whose model vocabulary differs from the router's routing vocabulary; more than two stops being
  auditable by span comparison.
- **Auditability is the constraint that ranks the options.** Mutation (b) keeps the editing discipline
  ADR-007 established (a single-pass span scan, no parse → reserialize, no mutable view of the body) and
  extends it by one clearly-bounded primitive rather than by a general body editor.
- **Determinism covers the length change.** The concern that "the outbound bytes are not identical to the
  client's" is not the cache risk; the risk is *instability across turns*. A deterministic function of
  (inbound bytes, route) is stable for a session on one route, which is the property the cache needs.
- **The failure mode was not configurable around.** The alternative to mutation (b) was not "a slower
  route" but "no working route": every real provider rejects the request.

## Consequences

- The passthrough path's review checklist, conformance expectation and audit are all "exactly two
  mutations"; the boundary constant lives in one place (`ROUTER_OWNED_TOP_LEVEL_KEYS` for (a)) plus one
  primitive (`set_top_level_string` in `crates/router-core/src/body.rs` for (b)), both covered by
  adversarial tests.
- ADR-004 line 16 and ADR-007 item 2 read as history from here on. The current boundary is AGENTS
  constraint 1 + spec §2 + DESIGN §6/§12.10.7 + this ADR; the historical files are not edited
  (append-only).
- The trace's `decision.requested_model` exists for this reason, and
  `upstream.submitted.body_hash` is per-attempt rather than per-request.
- The native-path conformance is CONF-27 (alias ≡ direct byte-identical; every other byte — whitespace,
  escapes, multi-byte UTF-8, trailing newline — unchanged).
- When the translation matrix lands, its encoders inherit rule 7; a translated cell that emits the
  client's route name upstream would be a contract violation, not an implementation detail.
