# ADR-024 — a cooling skip's `failover_from` survives the refusal, and `skipped[]` is a property of the chain

- Status: accepted
- Date: 2026-09-22
- Related: AGENTS hard constraints 1 (no body byte is touched by this ruling — the field it settles is
  trace-side), 4 (no figure, no saving statement), 6 (the two new cases assert a relation over their own rig),
  9 / ADR-012 (the measurement apparatus is untouched: this ADR *reads* an existing assertion, allocates two
  new case ids and moves no gate, corpus or existing assertion), ADR-010 (why an event is not invented),
  ADR-011 item 4 (the pre-attempt cooldown skip and `failover_from`'s producer), ADR-022 and **ADR-023** (the
  walk's refusals — this ADR is ADR-023's continuation, not its revision: both of its rulings stand), ADR-005
  (the trace as the analysis truth); spec §4.2, §6 (the `failover_from` producer table), §8 (the refusal's two
  conditions and the `skipped[]` clause); DESIGN §12.8, §12.10.9; the findings **R19-F1** and **R19-F2**
  registered in `autowork/progress/2026-09-22_05-33-57_R19-refusal-shape.md` §5.

## Background

R19 (ADR-023) settled *which* body a walk that serves nothing ends with. Its own close-out then probed a chain
no card in the round had built — a **condition-N** chain whose head is refused by ADR-011's cooldown — and
measured the two media still disagreeing on the same request. Reproduced by this card's own rig (one config,
one request body, both media, HEAD `47ac23c`, mock/loopback upstreams, zero upstream calls and zero spend;
evidence `autowork/harness/r20-1/`):

| | client body | `details.skipped[]` | trace `result.failover_from` |
|---|---|---|---|
| buffered | `502`, the frozen sentence, `stage` + `skipped` + `upstream_status`/`error_class` `null` | `[d/m demoted, k/m keyless, w/m wire_mismatch]` | **`"d/m"`** |
| streaming | the same, plus `"stream": true` | `[k/m keyless, w/m wire_mismatch, d/m demoted]` | **`null`** |

Chain: `d/m` (chat, keyed, **in cooldown** — demoted by a real `403 quota_exhausted` + `retry-after: 60` on
the rig's first request, ADR-011 item 4, no seeded row and no sleep) → `k/m` (keyless) → `w/m` (responses
wire). Nothing is attemptable, so both requests are condition N.

Two frozen sentences and one frozen assertion were in play, pointing two ways:

- spec §6's producer table, the **cooldown row** — **unchanged** by R19-1 — says the pre-attempt cooldown
  refusal **sets** `failover_from` to the route the skip abandoned (ADR-011 item 4);
- DESIGN §12.10.9 (R19-1) says condition N "wrote nothing (no `failover_from` and no `failover.triggered`
  row — CONF-57 (b) asserts exactly this on both media)", and ADR-023 Decision 2's general clause ("written
  only when the request moves onto a candidate the walk will actually attempt") reads the same way;
- `tests/conformance/tests/conf_42_primary_cooling_down.rs` — **an existing assertion, outside the mutable
  scope** (AGENTS 9 / ADR-012) — asserts the first reading on the buffered path: the request that resolves to
  `p-api/m1`, finds its provider cooling and is refused `502` with nothing left to attempt (`:350`) leaves a
  record whose `result.failover_from` is `"p-api/m1"` (`:366`).

In code the buffered walk writes the field at the skip and the shared failure recorder reads it
(`forward.rs:976`; `forward.rs:641-644` via `:596`), while the streaming walk keeps it in a **local**
(`stream_forward.rs:623`, set at `:649-652`) and copies it into the request's facts only on the paths that
serve or continue (`:820`, `:932`, `:1003`) — never at its two refusal returns (`:1044-1059`, `:1060-1092`),
so `record_failure_trace` (`:220-221`) writes `null`.

The second finding is the array's *order*: `skipped[]` is documented as the chain's own order on **both**
paths, but the streaming list is seeded with the construction-time skips (`stream_forward.rs:631`) and the
in-walk `demoted` entry is **appended** when the walk reaches the candidate (`:652`), so it always lands last.
The buffered walk has one list and one pass (`forward.rs:964-1013`) and is in chain order by construction.
The count half of ADR-023's third ruling is unaffected (3 offered → 3 listed on both media), which is exactly
why R19-4's rigs — built for completeness, not order — could not see it.

## Decision

**1. ADR-011's pre-attempt cooldown skip writes `result.failover_from`, and a refusal does not undo it.**
The field names the route the cooling projection refused before any attempt, **whether the walk then serves,
fails, or ends with nothing served**; so a condition-N refusal whose chain opened on a cooling route carries
it, on **both** forwarding paths, and the record's other displacement fields stay clear (no
`failover.triggered` row, `plan_switch` null unless the abandoned route was the family's `primary`). A
condition-N refusal whose chain offered **no** cooling route still carries `null` (CONF-57 (b)).

- **Grounded in an existing assertion, not in a preference.** `conf_42_non_primary_abandon_is_failover_only`
  (`tests/conformance/tests/conf_42_primary_cooling_down.rs:350`, `:366`) fixes this reading for the buffered
  arm, and that file is part of the measurement apparatus: AGENTS constraint 9 and ADR-012 put it outside the
  loop's mutable scope, so `null` is not a value this ruling may choose. The sentence that reads the other way
  (DESIGN §12.10.9) and the summaries that echo it were written over chains with **no** cooling candidate —
  they are statements about the attempts a walk makes, and they are narrowed here, not the assertion.
- **The medium that changes: streaming.** The walk's own `failover_from` must reach the request's facts before
  the terminal failure record is written, at both refusal returns (`stream_forward.rs:1044-1059` and the
  fall-through `:1060-1092`), exactly as the served and continue paths already do (`:820`, `:932`, `:1003`).
- **Not touched.** The keyless / wire / unknown skips write no `failover_from` (spec §8, CONF-57 (b)), and a
  failed attempt with no candidate after it writes none either (spec §6 row 2, CONF-58 (a)). The cooldown row
  is the one producer that needs no destination: the route it abandons is a fact of the request whether or not
  the walk went on, and the client-visible body — not the record — is where ADR-023's "one shape per
  condition" rule speaks.

**2. `skipped[]` is a property of the chain, not of the medium: in the chain's own order, and element-for-element
identical on both paths.** "The chain's own order" is the order the walk itself iterates: the resolved route
first, the family's `overflow` where the plan policy inserts it, then `fallback` in config order (spec §4.2).
The assertable invariant (spec §8): in a `no_available_route` refusal the `route` sequence of `skipped[]` is
exactly the offered candidates that were not attempted, in that order, and the two media's arrays are equal
element for element — so the whole client body differs by `"stream": true` alone.

- **The medium that changes: streaming.** Each entry must carry its chain position (or the list must be put in
  chain order before serialisation); seeding with the construction-time skips and appending the in-walk
  `demoted` entry (`stream_forward.rs:631`, `:652`) is what breaks the order. The buffered arm is in chain
  order by construction and is untouched; CONF-59's chain has no in-walk `demoted` entry, so it is already
  green on both media and is not touched either.
- **Completeness is unchanged** (ADR-023 Decision 3, CONF-59): one entry per candidate the chain offered. This
  decision moves the order only.

## Alternatives considered

- **`null` — "a displacement with no destination is not narrated", applied to the cooling skip too.** Rejected:
  it is not available. It would require both narrowing spec §6's unchanged cooldown row *and* changing
  `conf_42_non_primary_abandon_is_failover_only`'s assertion, i.e. a change to the measurement apparatus —
  a human decision under AGENTS 9 / ADR-012, not a loop outcome.
- **Drop the field on the buffered arm instead, so both media say `null`.** The same rejection (the recorded
  half of it, and the field would then disagree with the sentence §6 already carries). It also costs more
  code: the buffered walk sets the field at the skip and the walk end is the only place a *condition-N*
  record is written, so the equality would have to be maintained by a suppression at the walk end rather than
  by a producer rule.
- **State nothing and let each medium keep what it has.** Rejected: two frozen sentences promise that a client
  cannot tell which medium produced the refusal, and the array's order is client-readable, so the promise
  would be false in the record by construction for exactly the class of chain a cooldown creates.
- **Order `skipped[]` by anything other than the chain** (reason word, alphabetical, sorted in the serializer).
  Rejected: the order carries information — which candidate the walk reached first, and with it the chain the
  request actually offered — and a second definition would make the buffered arm wrong instead of making the
  streaming arm right. A sort by the chain position is the same rule, expressed in a serializer.
- **Leave the order to the cases (compare as a set).** Rejected: §8's order clause is a promise to a client
  that parses a sequence, and the two arrays differing in order is precisely the observable that made R19-F2 a
  finding rather than a stylistic note.
- **Dedupe `skipped[]` to provider granularity** so the order question cannot arise. Already rejected in
  ADR-023 (it withdraws the route-level information the `route` member exists to carry, and the streaming path
  narrates per candidate by construction).

## Rationale

- **The record should be a function of the request's history, not of the code path.** Both media run the same
  walk over the same chain; a field whose value depends on which arm noticed the refusal is a second definition
  of one fact — the class R17-F1 and R19-F1 share.
- **Where a frozen clause and a frozen summary collide, the clause that has a witness behind it wins, and the
  summary is narrowed.** Here §6's cooldown row has an assertion (`:366`) behind it and the §12.10.9 sentence
  has none for this shape; narrowing the sentence is the change that keeps every existing case green and makes
  the text describe the shapes that exist rather than the shape one rig happened to build.
- **The machine-readable members must not lie.** `details.stage` already means "no upstream was contacted"
  (ADR-023); `skipped[]` is the array a client parses for *what was refused*; the suite's own vocabulary needs
  the array's order to be a statement about the chain, or a reader has to know which arm answered.
- **Nothing here is a saving.** No transform, no token delta, no trace member, no figure: only the *value* of
  an existing v2 field on already-`502` classes moves.

## Consequences

- **Code.** Only `crates/router-proxy/src/stream_forward.rs`: the walk's local `failover_from` reaches the
  request's facts at the two refusal returns, and the walk's skip list carries the chain position so the array
  serialises in chain order. No `router-core` / `router-store` byte; no public type; `details` is free-form per
  code, so §8's type table does not move; `TRACE_SCHEMA_VERSION` stays **2** (an existing field's *value* on an
  already-`502` class, and an array's order).
- **Client-visible.** (i) The streaming arm's `skipped[]` is in the chain's own order — the only change inside
  a client body, and it is what restores §12.10.9's "nothing else differs, so a client cannot tell which medium
  produced the refusal". (ii) The buffered arm's body does not move at all. (iii) The trace record's
  `result.failover_from` stops depending on the medium.
- **The measurement apparatus is untouched** (AGENTS 9 / ADR-012): no gate definition, no corpus, no L1
  envelope, and no existing conformance assertion — this ruling *allocates* two new case ids, **CONF-64** (the
  record: a condition-N refusal whose chain opened on a cooling route carries the abandoned route on both
  media, with the cooling-free chain as the control) and **CONF-65** (`skipped[]`'s chain order and the two
  media's element-for-element equality). Both land as new files with the implementation they witness.
- **Docs.** spec §6 (the cooldown row's scope), §8 (the `skipped[]` order clause, the two producers of
  `failover_from`, the condition-N record); DESIGN §12.10.9 (the eligibility paragraph, the narration
  paragraph's consequence, the two-refusal paragraph, the trace-truth paragraph, the witness paragraph) and
  §12.8 (the allocation).
- **The owed `book/` clause is unaffected** and stays owed (ADR-023's boundary note; `book/` is not in this
  round's write set).

## Honest boundaries

- **Everything was measured with mock/loopback upstreams, one binary, zero real upstream calls and zero
  spend**, and the rig's artifacts are committed in this repository
  (`autowork/harness/r20-1/r201_mock.py`, `r201_rig.py`, `r20-1-rigD-raw.json`) rather than left in a scratch
  directory — the R11-F7 class is not repeated here. The run is a **red control**: it asserts the ruling's
  expected shape, and at HEAD it fails on exactly the three streaming-side checks (the two findings).
- **One chain shape is measured**: one cooling head, one keyless candidate, one wire-ineligible candidate,
  both media, one request body. Mixed chains a third party might build (two cooling routes; a cooldown skip on
  a non-head candidate that a later candidate is then attempted after; a cooling route behind an attempted
  head) are **not** measured by this card, and the ruling's wording is chosen to cover them by rule rather than
  by measurement.
- **The streaming loop-end fall-through** (`:1060-1092`) is still unreachable by every probe this loop has run
  (ADR-023's boundary stands, R19-N2); it is covered by this ADR's rule because it is one of the refusal
  returns the rule names, not because it was exercised.
- **Not verified here**: any live client or real upstream behaviour, and any performance effect (the changes
  are one field copy per refusal and one index per skip entry).
- **Nothing in this ADR is a saving statement, and none may be derived from it.**

## Reversibility

Reversible, cheaply, in both directions: restore the streaming walk's local-only field (delete the two
assignments at the refusal returns) and re-seed the skip list by seeding-and-appending again. What is *not*
reversible is a client that has parsed `skipped[]` as the chain's order — which is why the array's order is the
side that moves, rather than the buffered arm's, and the value that already has an assertion behind it is the
side that stays.
