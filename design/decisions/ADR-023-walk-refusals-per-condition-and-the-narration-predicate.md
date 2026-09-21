# ADR-023 — the walk's refusal is per condition, and the walk narrates only what it can serve

- Status: accepted
- Date: 2026-09-22
- Related: AGENTS hard constraints 1 (the byte boundary — unchanged here: no body is touched), 4 (no unverified
  savings — this ADR adds no figure), 6 (tests assert relations, not snapshots — both new cases assert a
  relation over their own rig's construction), 9 (the measurement is not in the search space: no gate, corpus or
  existing conformance assertion moves — CONF-58/CONF-59 are **new**, and the whole suite stays green with the
  rule below implemented, which is measured and quoted in "Honest boundaries"); ADR-022 (a candidate may serve
  only on its own wire — **this ADR is its continuation**, not its revision: Decision 1 there stands unchanged),
  ADR-010 (the event log as state truth — why a skip is not an event), ADR-011 (the upstream-error taxonomy,
  the provider-level exclusion, `failover_from`'s producer), ADR-012 (the evaluator and the frozen
  apparatus); spec §2, §4.2, §6, §8; DESIGN §12.8 (CONF-57/58/59), §12.10.9; R17-F1 and R17-F2 — the two
  findings this ADR adjudicates, registered in `autowork/progress/2026-09-22_04-42-49_R17-wire-gate.md` §5

## Background

R17 (ADR-022) closed R11-F1 — a chat body served on a responses wire — and froze the client-visible refusal
for a walk that ends with nothing served. Its own close-out then measured two cases **neither R17's
implementer nor its verifier had built**, on the round's own HEAD, with mock upstreams:

- **R17-F1** — a chain that **attempted** a candidate which failed and whose **tail is wire-ineligible** gets
  **two different refusal bodies**, one per forwarding path (ADR-022 Decision 3 and spec §8 both say "one
  shape … nothing else differs, so a client cannot tell which medium produced the refusal");
- **R17-F2** — `details.skipped[]` is documented as "every candidate the walk refused without attempting it",
  but a second candidate sharing a **keyless** provider appears in no member at all.

Both are reproduced by this card's own run of the R17 close-out's probes against the same HEAD binary
(`$HERMES_HOME/profiles/reviewer/cache/scratch/r17-4/probe_{refusal_asymmetry,skipped_gap}.py`,
results `asymmetry-repro.json` / `skipped-repro.json` beside this card's scratch dir), and this card added
five rigs of its own (`probe_walk_narration.py`) to separate the conditions — see "What was measured" below.
The mechanism, from the code at the round's HEAD:

- **The buffered walk's narration predicate is weaker than its eligibility predicate.**
  `forward.rs::next_candidate` (the route the walk says it is failing over *to*) filters on
  not-already-attempted, not-in-cooldown and **key/transport present**, but **not** on the wire. The walk's own
  loop, one screen below, filters on all of those **plus** `provider.wire_api == proto_in` (ADR-022 Decision 1).
  A wire-ineligible but keyed tail is therefore *narrated as the destination* (`failover_from` names the failed
  head, `failover.triggered.to` names the route that can never take this request, both priced at the
  destination's own table) and the walk then falls out of the loop into the walk-end body — which the buffered
  path emits unconditionally, with the last attempt's evidence attached.
- **The streaming walk pre-filters the same condition at chain construction** (`stream_forward.rs:541-548`),
  so the wire-ineligible candidate is never in its chain: the same failure finds no next candidate, returns
  in-loop, and the client gets the *attempt-exhausted* body. Hence the divergence: not two readings of the
  contract, but **two predicates** for one rule.
- **The buffered provider-level exclusion swallows a second model of a keyless provider**:
  `attempted_providers` is pushed on the keyless skip (`forward.rs:1006-1007`) and the loop's first test
  (`:967-969`) `continue`s before the eligibility tests without narrating anything. The streaming path's
  construction narrates **per route** (`:522-552`) and therefore lists both models — so here too the two media
  disagree with each other, not merely with the sentence.

### What was measured (this card's own rigs, mock/loopback upstreams, zero spend)

A five-rig probe (`probe_walk_narration.py`, results `walk-narration-head.json`) sends one buffered and one
streaming request per rig against one binary. On the round's HEAD:

| Rig | Chain (head attempted → tail) | buffered body | streaming body | buffered trace | streaming trace |
|---|---|---|---|---|---|
| A | `kp/m1` (keyed, 500) → `kj/m2` (**keyless**) | attempt-exhausted | attempt-exhausted | `failover_from: null` | `null` |
| B | `kp/m1` (keyed, 500) → `kp/m2` (**same provider**) | attempt-exhausted | attempt-exhausted | `null` | `null` |
| C | `kp/m1` (keyed, 500) → `mx/m` (**responses wire, keyed**) | **frozen `no_available_route`** + `skipped[{mx/m, wire_mismatch}]` + `upstream_status: 500` | **attempt-exhausted** | `failover_from: "kp/m1"`, `failover.triggered{to: "mx/m"}` | `null`, no event |
| D | `kp/m1` (500) → `mx/m` (responses) → `kn/m` (chat, keyed, 200) | 200 | 200 | `failover.triggered{to: **"mx/m"**}`, then `kn/m` attempted | `failover.triggered{to: **"kn/m"**}` |
| E | `kl/m1` (keyless) → `kl/m2` (same keyless provider) → `mx/m` (responses, keyed) — nothing attempted | frozen shape, **`skipped[]` = 2 entries** (`kl/m2` missing) | frozen shape, **`skipped[]` = 3 entries** | — | — |

Rows A and B say the divergence is **exactly the wire condition**: a keyless tail and a second model of the
attempted provider already produce the same body and the same trace on both paths, because
`next_candidate` already checks the transports map. Row C shows the frozen sentence
("every candidate provider is demoted, keyless or unavailable") and `stage: "no_available_route"` being
emitted **with a non-null `upstream_status`** — i.e. over a request whose head *was* native, keyed,
contacted, and whose 500 is the reason it failed. Row D shows the same predicate defect on a **served**
request: the buffered path's `failover.triggered.to` names the route that cannot serve. Row E shows the
`skipped[]` gap is an inter-media divergence, with the streaming side already candidate-granular.

## Decision

**1. The walk's refusal has exactly two conditions, and each has one shape that both paths produce.**
The streaming medium's pre-existing `"stream": true` member is the only permitted extra member; nothing else
differs, so a client cannot tell which medium produced the refusal (ADR-022 Decision 3's sentence, restated
per condition instead of once for a mix of two).

| Condition | When | The body | Client-visible markers |
|---|---|---|---|
| **N — nothing was attempted** | every candidate of the chain failed eligibility (unknown provider / no key or transport / wire mismatch / demoted, plus the resolved route itself) | the frozen `no_available_route` shape (spec §8): `502`, `error.type: upstream_error`, the frozen sentence verbatim, `details{stage: "no_available_route", skipped[], upstream_status: **null**, error_class: **null**}` | `stage` present and `skipped[]` present; both evidence members `null` |
| **E — an attempt was classified and nothing served after it** | at least one candidate was submitted to an upstream and the walk ended with no candidate served (including: the attempt failed and no candidate could be attempted after it) | the attempt-exhausted shape the `upstream_error` row's first limb already describes: `502`, the class-based sentence (`upstream error ({class}) and the fallback chain is exhausted`; the deterministic variant for `format_error`/`content_policy_blocked`; the connect variant when the last attempt never reached the upstream), `details{upstream_status, error_class}` | **no `stage`, no `skipped[]`** |

Consequences of the mapping, and the reason it is this way round:

- **`details.stage: "no_available_route"` means "no upstream was contacted".** That is what spec §8's
  behaviour clause already says, and it becomes true *by construction*: the member is emitted only when
  nothing was attempted.
- **The frozen sentence is never emitted over a contacted request.** In condition E the sentence would be
  false ("every candidate provider is demoted, keyless or unavailable" — the head was none of the three), and
  a frozen sentence a client may match verbatim is worth exactly as much as its truth. The evidence
  (`upstream_status` / `error_class`) is the actionable part of that refusal, and it is what §8's first limb
  already promises.
- Condition E's sentence is the one the in-loop exhaustion sites already emit, so the in-loop sites move
  **not at all**; only the walk-end sites (which are the ones that must say which condition they are in) do.

**2. The narration predicate is the eligibility predicate: the walk names a destination only if that
candidate could serve this request.** A candidate reaches the per-request candidate chain iff its provider
entry exists, this process holds a key/transport for it, its `wire_api` equals the inbound protocol, its
provider was not already attempted in this request, and its provider is not in ADR-011's cooldown projection
— the same five conditions the buffered loop already applies, stated once. Consequences:

- `result.failover_from` (spec §6) and the `failover.triggered` event are written **only when the request
  moves onto a candidate the walk will actually attempt**. In condition E there is no such candidate, so
  neither is written: the refusal carries the last attempt's evidence instead. That is what the streaming
  path has always done (its chain is pre-filtered), so this is a uniformity statement, not a new rule.
- The `to` member of `failover.triggered` names a route that can take the request (rig D above).
- The **cooldown skip's** `failover_from` (ADR-011 item 4, CONF-42) is *not* touched: that skip names the
  route the walk moved the request **off**, and it is unchanged. The provider-level in-request exclusion
  (ADR-011 item 4) is likewise unchanged.
- As a consequence of 1+2, the buffered walk end is reachable **only** with nothing attempted: every
  attempt outcome returns in-loop (either it serves, or its `next_candidate` is `None` and the in-loop
  exhaustion body is emitted), so condition N cannot carry an attempt's evidence.

**3. `skipped[]` is per candidate, and in a condition-N refusal it is complete.** One entry per candidate the
chain offered, in the chain's own order, each with exactly one of
`unknown_provider` / `keyless` / `wire_mismatch` / `demoted`. The invariant a case can assert over its own
rig: **in a `no_available_route` refusal, `|skipped[]|` equals the number of candidates the chain offered**
(rig E: three candidates → three entries, the two models of the keyless provider listed with the same
reason). A provider the walk refused as keyless must narrate **every** candidate of that provider; the
provider-level exclusion may not swallow the second model.

**No fifth reason word is needed, and the "already attempted" case needs no carve-out clause**: a request that
attempted anything is condition E, whose body has no `skipped[]` at all. The two rules (1 and 3) therefore
describe the same space without overlap, and the vocabulary stays exactly the four words ADR-022 froze.

**4. Where each shape lands (the implementing round's write set, symbols, not prose).**
Condition N keeps its three existing sites: the buffered walk end (`forward.rs`, the `no_available_route`
body at the end of `forward_inner`), the streaming empty-chain return, and the streaming loop-end branch that
is already conditional on nothing having been attempted. Condition E keeps all six in-loop sites
(`exhausted_failure` after a failed response, the connect-failure return, the `unknown_outcome` return, and
their three streaming twins) unchanged. The one site that must move: the streaming loop-end **fall-through**
(what follows the `attempted.is_empty()` branch) currently emits the frozen sentence with no `stage` and no
`skipped[]`; it must emit condition E's body, so that no path can emit the frozen sentence with an attempt
behind it even if that arm is ever reached. The buffered walk end needs **no** discriminant: with Decision 2
in place, no attempt-bearing ending reaches it.

## Alternatives considered

- **Extend the frozen shape to the mixed chain** (drop the streaming split; give the mixed chain `stage`,
  `skipped[]` **and** a non-null `upstream_status` on both paths). Rejected: the frozen sentence and the
  `stage` member would then assert "no upstream was contacted" over a request that was contacted, and
  `stage` would stop distinguishing the two conditions — the one thing a machine-readable member is for.
  §8's own trigger row already assigns that chain to its first limb ("an upstream error and the fallback chain
  is exhausted"), so this choice also keeps the row's two limbs meaning two things.
- **Keep the shapes and patch the buffered walk end only** (reproduce the exhausted sentence there when
  something was attempted). Rejected as the *shape of the fix*: the walk would still narrate a displacement
  onto a candidate that cannot serve — `failover_from` and a `failover.triggered.to` naming the
  wire-ineligible route, with a re-prefill cost priced at *its* table — so the same root cause would keep
  producing a second, quieter divergence between the media. Fixing the predicate removes the body, the trace
  field and the event divergence together (rigs C and D).
- **Narrow `skipped[]` to provider granularity** ("one entry per provider the walk refused before attempting
  it"). Rejected: it would need new dedupe logic in three branches that today narrate per candidate, it would
  **withdraw** the route-level information the frozen `route` member exists to carry, and it would introduce
  the opposite asymmetry — the streaming path already lists both models of a keyless provider (rig E). The
  two media can only agree at candidate granularity.
- **Add a fifth skip reason for "the provider was already attempted"** so that `skipped[]` could literally
  hold every non-attempted candidate. Rejected as dead vocabulary: under Decision 1 such a request is
  condition E, whose body carries no `skipped[]`, so no client-visible body could ever contain the word.
- **Refuse to touch this and re-register the findings.** Rejected: two frozen documents promise one shape for
  both paths, the divergence is client-visible, and a client currently cannot tell "nothing could serve" from
  "something failed and nothing could serve after it" — the distinction the two conditions exist to make.

## Rationale

- **The condition, not the site, decides the body.** R17 froze one body and named three sites; the sites are
  two different *conditions*, and the freeze never said which one the mixed chain was. Naming the conditions
  makes the bodies follow from the request's own history (was anything attempted?) — a fact both paths can
  observe identically — instead of from where each path happens to notice exhaustion.
- **One rule means one predicate, and a predicate that is stated twice drifts.** R17-F1 is exactly a second
  statement of "which candidates may serve" that had lost the wire condition. Making the narration predicate
  the eligibility predicate removes the class, not the instance.
- **The machine-readable members must not lie.** `stage` is the member a client keys on; the fix makes it
  true rather than widening it. `skipped[]` is the member a case asserts a relation over; the fix makes the
  relation hold over the chain's own construction (three offered → three listed).
- **Nothing here is a saving.** No transform, no token delta, no trace field added or removed, no figure.

## Consequences

- **Code.** `router-proxy/src/forward.rs`: `next_candidate` gains the wire condition (and thereby the
  provider-existence check, which `provider()` resolves); the keyless skip records its provider so a second
  model of it is narrated; comments updated. `router-proxy/src/stream_forward.rs`: the loop-end fall-through
  emits condition E's body. `router-core` untouched; no public type changes; `details` is free-form per code,
  so no `error.type` is added and §8's table does not move.
- **Client-visible.** (i) A mixed chain's refusal changes on the **buffered** path from the frozen shape to
  the attempt-exhausted shape (the streaming path already gave that; both now agree). (ii) A condition-N
  refusal's `skipped[]` gains the entries for extra models of a keyless provider, on **both** media. (iii)
  `errors[0].details` on the record follows the body, as always.
- **The record's shape does not move.** No field is added or removed and no value moves on a served request,
  so `TRACE_SCHEMA_VERSION` stays 2. What does move, in condition E of the chain that has no destination:
  `result.failover_from` and one `failover.triggered` row are no longer written (the streaming path never
  wrote them) — a fact about a displacement that never happened, which ADR-010's vocabulary already refuses
  to invent.
- **The measurement apparatus is untouched** (AGENTS 9 / ADR-012): CONF-57's assertions stay as they are —
  its rig is condition N on both arms and every one of its expectations is still met — and the two new cases
  (CONF-58, CONF-59) are new files. No gate definition, corpus or existing assertion changes. Measured: with
  the rule implemented, the full suite is **368 passed / 0 failed / 12 ignored (85 result lines)** — the same
  totals as the round's HEAD, so the freeze requires no existing assertion to move.
- **Docs.** spec §4.2 (the narration sentence), §6 (`failover_from`'s row: written only when there is a
  destination), §8 (the row's two limbs, the behaviour clause, the frozen shape and the `skipped[]`
  invariant); DESIGN §12.10.9 (this landing) and §12.8 (CONF-58/CONF-59, and the occupancy slip fixed).

## Honest boundaries

- **Everything here was measured with mock/loopback upstreams, on one binary, with zero real upstream calls
  and zero spend.** No live client or live upstream exists anywhere in this round.
- **The binary is the round's own**: `/tmp/r17-4-target/debug/router`, built at `8264f61`. The `crates/`,
  `tests/`, `docs/`, `design/` and `book/` bytes at HEAD are identical to that commit's
  (`git diff --stat 8264f61..HEAD -- crates/ tests/ docs/ design/ book/ config.example.yaml` → empty), so the
  measured behaviour is HEAD's.
- **The proposed rule was implemented experimentally to prove it is implementable and non-breaking** — in a
  throwaway worktree (`/tmp/r19-1-exp`, removed afterwards; its `crates/**` bytes are **not** committed by
  this card, whose diff is docs-only), with the probes re-run against the patched binary
  (`walk-narration-exp.json`: rigs A–E all agree across media; rig C's `failover_from` is `null` on both and
  no `failover.triggered` row is written; rig D's `to` names `kn/m` on both; rig E lists three entries on
  both) and the full suite run twice (once per fix: 368/0/12 both times). **That experiment is evidence for
  this ADR, not a delivery**: the implementation is the next card's work, on its own branch, with its own
  four-gate verdict.
- **What this ADR does not decide or verify**: the mapper (ADR-022's rejected alternative C) — untranslated
  cross-wire service stays barred and translation stays unimplemented; the SSE refusal's chunk-boundary
  semantics; the `unknown_provider` skip's reachability at request time (v0.1 refuses a non-roster fallback
  entry at load, so the code path has no witness); `state.sticky_hit` (R11-F2, still open); the streaming
  loop-end fall-through's reachability — the probes never reached it, so its new shape is frozen but
  unasserted, and the implementing round may keep it as a safety net (it must not emit condition N's
  sentence with an attempt behind it) but must not claim a witness for it.
- **Not verified here**: any live client behaviour, any real upstream, and any performance effect (the change
  is one predicate per candidate; the operative latency figure stays R4's carried p50 ≈ 1 ms / p99 ≈ 6 ms).
- **The `book/` sentence is still true but is now incomplete** (`book/connecting-clients.md`, the
  "failover chain never crosses protocols" paragraph: it describes the `stage: "no_available_route"` case).
  A one-clause addition naming the second condition is owed to the next round that opens the book; this
  card's write set does not include `book/`, so it is recorded here rather than landed quietly.
- **Nothing in this ADR is a saving statement, and none may be derived from it.**

## Reversibility

Reversible, cheaply, in both directions: the rule is one predicate plus one body at one fall-through site, so
restoring the previous behaviour is deleting the wire condition from `next_candidate` and putting the frozen
sentence back at the streaming walk end (the previous divergence returns with it, and one table in this ADR's
"what was measured" section names it). What is *not* reversible is a client that has parsed `stage` as "no
upstream was contacted" — which is why the choice made here is the one that makes that reading true, rather
than the one that leaves the reporting sites shortest.
