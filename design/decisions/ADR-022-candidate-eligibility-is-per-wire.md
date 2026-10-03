# ADR-022 — a candidate may serve only on its own wire: the failover walk never crosses the 3×3 matrix, and a refusal says why

- Status: accepted
- Date: 2026-09-22
- Related: AGENTS hard constraints 1 (the byte boundary — the router may not re-encode the client's body, so it
  may not put a chat body on a responses wire either), 4 (no unverified savings — this ADR adds no figure),
  6 (tests assert relations, not snapshots), 9 (the measurement is not in the search space: no gate, corpus or
  existing conformance assertion moves here — CONF-57 is **new**); ADR-004 (passthrough vs safe translation),
  ADR-007 / ADR-015 (span-faithful forwarding, the two permitted mutations), ADR-010 (the event log as state
  truth — why a "skip" is not an event), ADR-011 (the upstream-error taxonomy and the failover action set),
  ADR-014 (plan-first routing: the `overflow` route is a candidate of the same walk); spec §2, §4.2, §6, §8;
  DESIGN §12.8 (CONF-57), §12.10.5 note R1, **§12.10.9** (the landing); R11-F1 (the measured defect this ADR
  closes) — the loop's tree §7 and §10 recommendation 1

## Background

### What shipped, and the one place it is not one rule

The router resolves the client's `model` string to a route, then forwards. Failover (spec §4.2) is a **walk**:
a candidate chain — the resolved route first, the plan family's `overflow` route next when the request is
inside a family (spec §4.6 / ADR-014 item 5), then the global `fallback` list — is tried in order, and a route
that cannot take the request is **skipped**, not refused. Both forwarding paths own such a walk:

- **buffered** (`crates/router-proxy/src/forward.rs`): the candidate vector is built at `:939-949` and walked at
  `:961-988`. The walk skips a candidate for exactly three reasons today — its provider was already attempted
  in this request (`:962`), its provider is inside ADR-011's cooldown projection (`:965`), or it has **no
  transport** because no key for it exists in this process (`:982-988`; an unknown provider, `:979-981`, is
  skipped by the same shape);
- **streaming** (`crates/router-proxy/src/stream_forward.rs`): a candidate **enters** the chain only if
  `cfg.wire_api == proto_in && self.api_keys.contains_key(&cfg.name)` (`:530`) — i.e. the same "no key" skip
  **plus a wire check the buffered path does not make**.

Both paths do check the wire for the **resolved** route, before the walk, with the same two statuses the spec's
error table gives that situation: `!provider.supports.contains(&proto_in)` → `400 capability_unsupported`
(buffered `:774-789`, streaming `:352-365`), and `provider.wire_api != proto_in` → `501 not_implemented`,
message *"translation {in} -> {out} is not implemented in v0.1; only native routes are served"* (buffered
`:790-799`, streaming `:368-379`).

### The measured defect (R11-F1)

With the shipped example roster's own `fallback` list and a **chat** request whose resolved route is unkeyed,
the buffered walk skipped the unkeyed route and forwarded to the next candidate — a **responses**-wire entry.
A mock upstream received at `/responses` the body `{"model": "deepseek-v4-pro", "messages": [{"role": "user",
"content": "x"}]}` — the client's **chat** bytes, which the router is forbidden to re-encode and never
translated — and the client received a **responses-wire SSE body with HTTP 200**. The record said
`protocol_in chat / protocol_out responses / translated true` although **no translation code exists in the
tree**: `translated` is derived as `proto_out != proto_in` (`crates/router-proxy/src/accounting.rs:501`), a
comparison of two words rather than the report of a step that ran. `usage_missing true`, cost 0.

That is a protocol-fidelity violation on a **blocking** gate (the round's own matrix), served to a client that
asked for something else, and it is reachable from the shipped example config — which is why it is a contract
question and not a code tidy-up. Nothing in the suite covers it today.

### The reading that decides it

A candidate's wire is a property of the **(provider entry, inbound protocol)** pair and is knowable before any
request is handled — exactly like "this process holds no key for that provider". Both conditions mean *this
candidate cannot serve **this** request*, and the walk's existing answer to that is to continue. The resolved
route is a different question: there the client **named** the cell, so §8's 501 (and its 400) is the answer to
give, and it is the answer both paths already give.

## Decision

**1. Candidate eligibility is one rule, and it includes the wire.** A candidate route is attempted only if all
of the following hold: its provider entry exists in the roster; the process holds a key and a transport for it;
and **its provider's `wire_api` equals the inbound protocol**. The walk-order rules are unchanged (a provider
already attempted in this request is not re-attempted; a provider inside the cooldown projection is skipped and
`failover_from` names it, CONF-42). Because config validation already requires `wire_api ∈ supports`
(`crates/router-core/src/config.rs:1551`), the wire condition subsumes "the inbound protocol is declared in
`supports`": the rule is stated once, as `wire_api == proto_in`, on both paths.

**2. A wire-incompatible candidate is skipped — never refused — and it is skipped in the keyless class.** It is
not attempted, so it produces no `upstream.submitted` intent row, no `error.classified` row, no
`failover.triggered`, no **`failover_from`** and no `plan_switch` entry, and no `errors[]` member. Rationale:
`failover_from`'s contract is *the route a failure moved the request off* (spec §6's producer table) — nothing
failed here and nothing moved, and naming it would mint a switch that never happened, with a re-prefill cost
that does not exist. The client learns nothing from the skip until the walk ends.

**3. When no candidate may serve, the client gets one frozen shape, on both paths:**

```
HTTP 502
{"error": {"type": "upstream_error",
           "message": "no available route: every candidate provider is demoted, keyless or unavailable",
           "request_id": "<req-N>",
           "details": {"stage": "no_available_route",
                       "skipped": [{"route": "<provider>/<model>", "reason": "<reason>"}, …],
                       "upstream_status": <u16|null>,
                       "error_class": <string|null>}}}
```

with `<reason> ∈ {unknown_provider, keyless, wire_mismatch, demoted}`, one entry per candidate the walk
refused **without attempting** it, in the chain's own order. The streaming path's pre-existing `"stream": true`
marker is the only permitted extra member on that medium's refusal; **nothing else differs between the two
paths**, because "which candidates may serve" is now one rule and a client must not be able to tell which
medium produced the refusal. The sentence is a human sentence and is frozen verbatim — "unavailable" is its
honest umbrella for the wire reason; the machine-readable truth is `details.skipped[]`.

**4. The resolved route keeps its own answers, and the walk's skip is not a substitute for them.**
`400 capability_unsupported` (inbound protocol ∉ `supports`) and `501 not_implemented` (declared cell, other
wire) fire for the route the **client named**, on both paths, exactly as today. The wire gate applies to
**candidates the client did not name**; a 501 for the resolved route does not cause the walk to serve a
fallback instead. §8's 501 row and this ADR's refusal shape stay two sentences, not one.

**5. The trace tells the truth about both fields it has.** For a **skip** the record belongs to the request
that was eventually served: `protocol.out == protocol.in`, `translated == false`, `failover_from clear`,
nothing else added. For an **exhausted walk** the record is a terminal failure (`502`, `usage_missing: true`,
nothing charged, `errors[0].kind == "upstream_error"` carrying the same `details` object the client saw);
`protocol.out` keeps naming the resolved route's provider wire — which the pre-flight guarantees equals
`protocol.in` — and **`protocol.out` may never name a wire other than the inbound protocol**. `translated` is
restated as an **event**: it is true only when the attempt that carried the request re-encoded the body through
a mapper. v0.1 has no mapper, so `translated` is `false` on every record this build writes, and the comparison
form at `accounting.rs:501` is no longer an admissible producer — it was the fabrication R11-F1 exhibited.
`protocol.lossy` stays `[]`.

**6. The refusal is a candidate-level fact, so it is a walk predicate and not a load-time rule.** A roster may
legally carry entries for several wires, and any of them may be the one a given client asks for; the
incompatibility exists only between an entry and **this** request's inbound protocol. Nothing is refused at
config load on account of it.

## Alternatives considered

- **Refuse the whole request with a 501 (or 400) as soon as a *candidate* is wire-incompatible.** Rejected:
  it would let a config's tail break a request its head can serve — a chat request with a keyed chat-native
  primary and a responses-wire fallback is served today and must stay served. The 501's meaning is "the cell
  **you** asked for is not implemented"; widening it to "some entry you never asked for cannot take this
  protocol" would make one status code mean two different things and would contradict §8's own row.
- **Serve the walk anyway when the *resolved* route is 501.** Rejected: it converts "your route needs
  translation" into "here is a different route's answer", i.e. it hides a missing capability and answers with
  something the client did not ask for — the opposite of what §2's "every translation cell must explicitly mark
  its lossy points" and §8's 501 row exist to enforce. M4's 13/13 witness of that refusal shape is also a
  frozen record this round must not quietly retire.
- **Implement the mapper now.** Rejected as scope, not as value: a translator is a lossy, determinism-critical
  content step (ADR-004, ADR-019) with its own contract, its own lossy register and its own conformance cases.
  This ADR only stops the router from *pretending* one ran.
- **Refuse such a roster at config load.** Rejected: see Decision 6 — the condition is a property of the
  request, and refusing legal rosters would be a new load-time rule with no defect behind it.
- **Leave the walk as it is and fix only the trace marker (`translated`).** Rejected: the record would stop
  lying while the wire still carried a chat body to a responses endpoint. Protocol fidelity is the blocking
  gate; the observation is not a substitute for the behaviour.
- **Copy the streaming path's construction-time filter literally into the buffered path** (filter the candidate
  vector where it is built, and early-return `502` when it comes out empty). Rejected as the *shape of the fix*,
  though its predicate is exactly the rule adopted: the buffered path's refusal must also report cooldown
  skips and the last attempt's evidence, which only the walk-end knows (that is where the shape in Decision 3
  is built today).

## Rationale

- **One rule, two paths.** The streaming path already had the wire test; the buffered path did not. Stating the
  rule at the level of "which candidates may be served" — rather than at "where each path filters" — is what
  makes the two paths' answers to a given config and request the same, which is the point of the freeze.
- **The skip class is chosen by contract, not by taste.** `failover_from` and the priced switch exist for a
  displacement that consumed an attempt; a static ineligibility did not. This is also why the wire skip needs
  no event: ADR-010's vocabulary has no row for "a fact that never happened".
- **The refusal shape is machine-readable so the case can assert a relation.** A test that asserted a snapshot
  of the failure body would be a change-detector; `details.skipped[]` lets the case assert *"every candidate the
  config offered appears once, with the reason the walk actually applied, and the foreign mock received zero
  requests"* — a relation over the rig's own construction, with no number to keep in step.
- **Nothing here claims a saving.** No transform, no token delta, no figure: D3 stays unmet.

## Consequences

- **Code (the implementing round's write set).** One predicate added to the buffered walk
  (`crates/router-proxy/src/forward.rs:961-988`), the streaming chain's refusal arms extended from a bare
  "keyless or unavailable" shape to the frozen one (`stream_forward.rs:553-561` and `:1001-1013`), the same
  shape built at the buffered walk end (`forward.rs:1430-1441`), and `translated`'s producer restated
  (`accounting.rs:501`). No public type changes; `details` is free-form per code, so no `error.type` is added.
- **Docs.** spec §2 gains the walk's rule beside the 3×3 selection rule, §4.2 gains the candidate-eligibility
  sentence and points at the refusal, §6 restates the `protocol` segment's two derived values, §8 gains the
  `no_available_route` clause and its body. DESIGN §12.10.9 is the landing; §12.8 allocates **CONF-57**.
- **The record's shape does not move.** No field is added or removed and no existing value changes on a served
  request, so `TRACE_SCHEMA_VERSION` stays 2; the only value that moves is one on the already-`502`
  walk-exhausted class, and the record remains a `usage_missing` failure priced nowhere.
- **What a reader must now assume.** A served request's `protocol.out` equals `protocol.in` **by construction**,
  so a foreign `protocol.out` on a served record is a defect, not a configuration to explain away.

## Honest boundaries and verification owed

- The defect was measured **once**, at one commit, against a **mock** upstream (R11-F1, reproduced twice); the
  fix is witnessed by the new conformance case, not by a live upstream. No live client is re-run by this round.
- `details.skipped[]` is new client-visible vocabulary. No gate, corpus or existing assertion is touched by
  adding it — the ids in `tests/conformance/` are untouched and CONF-57 is a new file — but a client that
  parsed the old failure body verbatim would see new members in `details`.
- The wire gate closes the *served* path's cross-wire reachability. It does **not** implement translation, does
  not touch the resolved route's 400/501, and leaves R11-F2 (`state.sticky_hit` on the streaming path) open —
  that finding is a separate card.
- Nothing in this ADR is a saving statement, and none may be derived from it.

## Reversibility

Reversible, cheaply, in both directions: the rule is one predicate and the refusal shape is one failure body.
Restoring the old behaviour is deleting the predicate (and the shape returns to its pre-ADR wording); widening
it later — for example when a mapper exists — is choosing a different eligibility rule for a candidate whose
wire differs, at which point this ADR is **superseded** (append-only: a new ADR states the new rule and cites
this one). What is *not* reversible is a served cross-wire request: those bytes exist once delivered, which is
why the ADR freezes the refusal rather than the tolerance.

## Publication note (2026-10-03, R62-2)

The `R<n>` labels and finding ids in this document name iterations of the project's own
private analysis loop — a loop that is not part of this repository, so no label here is
resolvable by a reader of it; they are kept as the provenance of the decision. This
publication pass removed only the dead-pointer class: every reference into that loop's
working tree (its file paths and round-record names, its state record, charter, execution
model and replay contract, its scripts and module names, and the kanban card ids), each
replaced by the neutral phrase the sentence needs. Nothing else moved — no figure,
threshold, `§`/`ADR`/`CONF` id, code sample or contract sentence.
