# ADR-019 — the byte boundary is a property of the passthrough path: a content transform exists only in an explicitly requested *transform mode*, and there it owes a declared edit list, three invariants and a labelled ledger

- Status: accepted
- Date: 2026-09-21
- Related: AGENTS hard constraints 1 (byte boundary), 2 (content determinism), 4 (no unverified
  savings), 9 (the measurement is not part of the search space); ADR-003 (the transform pipeline and
  its P0–P4 tiers — its "v0.1 ✅" column is **re-read** by item 3 below), ADR-007 (span-faithful
  forwarding; its closing consequence clause is this ADR's precedent), ADR-008 (rules as data, the
  inline test as a rule's only spec), ADR-012 (the mutable scope: `AGENTS.md`, `docs/spec.md` and
  `design/` are human-gated paths), ADR-014 (the session boundary as the place a policy may change),
  ADR-015 (the two-mutation boundary; its item 5 forbids widening the **passthrough** path),
  ADR-016 (primitive P6 `transform-chain` is `contract-only`; workflow W2); spec §2 (+ new §2.1) /
  §4.4 / §6 / §7 / §8; DESIGN §3, §6, §12.3, §12.3.1, §12.10.6, §12.10.7, **§12.12** (the landing);
  `rules/tool_output.toml` (the landed rule-file format); `autowork/program.md` direction **D3** and
  its gate

## Background

AGENTS hard constraint 1 is the repository's sharpest rule: *"The proxy forwards the original request
bytes. Exactly two mutations are permitted, both byte-level, both scoped, both auditable … **Never
touch message content, order, whitespace, or tool schemas on the passthrough path**."* ADR-015 fixed
the second mutation and closed the same hole from the other side: *"It is deliberately the smallest
possible second mutation … a third is a change to AGENTS constraint 1, which no round may make on its
own (ADR-012)."*

Two properties are what that rule protects, and they are both about **verifiability**, not about
conservatism:

1. **Auditability.** The outbound body is the client's bytes plus an enumerated, finite list of
   byte-level edits, so a reviewer compares *spans*, not documents (ADR-007). A byte that changed for
   an unlisted reason is a defect by construction.
2. **Prefix stability.** The client resends the whole body every turn (`store: false`, no
   `previous_response_id`, `prompt_cache_key` as the session identity), so the upstream prompt cache
   is decided entirely by router's outbound bytes: the same session measured
   `cached_tokens` 960/14409 on turn 1 and 14400/14520 (**99.2%**) on turn 2 (AGENTS). Order,
   whitespace, escapes, `tools` (`16` schemas in every request) and unknown fields are all in the
   prefix. Cache fidelity is the first-order lever; a model choice is second-order.

The product's headline promise is the *other* thing: `book/cost-and-caching.md` describes a transform
pipeline whose tiers are "cache fidelity → input-side payload reduction → output-side discipline →
provider arbitrage", `README.md` repeats it, and ADR-003's tier table marks P1 (input-side payload
compression) and P2 (output-side discipline) as **✅ in v0.1**. Content compression is, by definition,
a change to message content — the one operation constraint 1 names.

What is actually implemented is the opposite of that column: primitive **P6 `transform-chain` is
`contract-only`** (DESIGN §13.1; `router-plugins/src/lib.rs` is a stub, `transforms: Vec::new()` at
`router-proxy/src/accounting.rs:483` and `auth.rs:170`), CONF-16's own `#[ignore]` says *"no transform
exists to enable yet (the empty chain is measured by CONF-15); enabling this with a vacuous body would
be an always-true test"*, and R2's gate record reads *"PASS (empty transform chain, so no savings
rows)"*. The rule **data** is landed (`rules/tool_output.toml`, four rules, 13 inline tests, a stage
order and a `tee` marker) and points at an engine that does not exist. So today the repository
simultaneously promises content compression and forbids it, and neither statement is false: the
promise has no contract behind it, and the prohibition has no scope definition in front of it.

ADR-007 already drew the line this ADR has to finish drawing, in its own consequence clause: *"Any
'just normalize it in passing' request … counts as a violation of hard constraint 1: either abandon
that transform, or **register it explicitly as a transform at the plugin layer** and bear the cache
cost (spec §6's `cache_impact`)."* What that sentence does not say is **where such a transform may
live**, what the byte promise becomes there, what keeps it from silently destroying the cache, and
which number may be called a saving. Those four questions are this ADR.

## Decision

### 1. The passthrough path is defined, and only a request may leave it

A request is on the **passthrough path** iff it did not ask for a transform. Absence of a request is
therefore a byte-level guarantee, not a configuration accident:

- **No config key, plugin, alias, route or default may put a request into transform mode.** The
  operator decides *which* transforms exist and their parameters (that is the config, ADR-008); the
  client decides *whether* they apply to its request. An operator-forced transform is refused not
  because it is dangerous but because it would make constraint 1's first sentence false in general
  and turn the fidelity assertions (CONF-01/02/03/10/11/27) into statements about a configuration
  rather than about the path.

### 2. One opt-in channel: a request header

```
X-Router-Transform: passthrough | transform      (absent or "passthrough" ⇒ the byte path)
```

- Any other value is `400 invalid_request`, decided **before** the body is read — the same stance as
  `deny_unknown_fields` (§12.5): *"I changed it but it did not take effect"* is the most expensive
  silent failure, and a typo must not silently disable a saving the client asked for.
- Read by `router-proxy`, never by `router-core`: the proxy resolves the header into a `TransformMode`
  value and passes it in, exactly as `server.auth_token_env` keeps the environment out of
  `router-core` (§12.5, §12.11). A header is also **not a body byte**, so the opt-in itself cannot
  perturb the prefix block domain, `body_hash`, or the two-mutation table.
- The client is configured by the same operator who could have written a config key, so this channel
  costs the operator nothing; per-request is what makes "closed mode" assertable (item 5, I3).

### 3. What transform mode changes is the *promise*, not the boundary

In transform mode the outbound body is the client's bytes with **an ordered, declared list of
value-span edits at addressed paths**, plus mutations (a) and (b) — and nothing else:

- **Span-faithful, never a reserialize.** An edit replaces a node's *value span* (the same discipline
  as `set_top_level_string`, one level deeper: a path such as `input[7].output`), located by the same
  single-pass scanner the whitelist deleter, the `model` rewrite and the prefix-block extractor share
  (ADR-007 item 3, DESIGN §12.3.1/§12.10.6). Every byte outside the declared spans is the client's,
  byte for byte. A parse → reserialize round trip is forbidden here exactly as it is there.
- **Payloads only, never intent.** Edits may target tool/environment payloads (tool results, command
  output, search results, diffs, logs) and are selected by declaration (`match_tool` / `match_kind`),
  never by guessing at content. **No edit may touch a user or assistant message, the system
  instruction, the tool schemas, or any structural member** — ADR-003's item 5 boundary and
  `rules/tool_output.toml`'s hard constraint 2, raised here from a convention to a rule of the mode.
  A transform that changes *what the model is asked to do* (instruction injection, response-clamping)
  is **not admitted by this ADR**; that is a semantic change and needs its own decision.
- **Declared and accounted.** Each step writes a ledger entry — rule id, the edited path, bytes and
  (inferred) tokens in and out, cache impact, and the `verified`/`inferred` label — and the record
  names the mode. An edit that cannot be described this way is not admissible as a transform.
- **Fail-safe.** A rule that fails to load, fails to compile, fails its inline tests or fails to
  apply does not edit anything: the payload passes through verbatim and the record carries
  `errors[].kind = "transform_error"` (spec §8, DESIGN §12.3). Degradation is declared, never partial.

### 4. Three invariants, each an assertion that can fail

**I1 — Content determinism.** For a fixed effective transform set `S`, `out = f(inbound_bytes, S)` is
a pure function: identical inbound bytes with the same `S` produce identical outbound bytes, and `f`
reads no clock, no turn index, no session history, no environment and no RNG (AGENTS 2). *Sufficient
rule for authors:* the engine is **text-in / text-out per node** (`rules/tool_output.toml`'s stages
already are), so determinism is a property of the rule, not of the pipeline.

**I2 — Prefix monotonicity, per fixed effective set.** Within one session and with `S` unchanged, if
request N+1's inbound body extends request N's by an append, then `out(N)` is a **byte prefix** of
`out(N+1)`, and `prefix_continuity` stays 1.0. *Sufficient rule:* an edit may depend only on **the
node it edits** plus the stable rule set — then every turn re-derives the same bytes for every
already-present node. A non-local shape (dedup keeping the *earliest* occurrence) can still be
monotone and must prove it on a fixture; anything that rewrites or removes a node **as a function of
the conversation's length or position** (rolling windows, "compress everything older than N turns",
summarisation) breaks I2 by construction and is P4 — deferred, and the reason it stays deferred is
that it raises the bill silently.

**I3 — Closed-mode byte equality.** For any request whose mode is `passthrough`, the outbound bytes
equal the client's modulo exactly mutations (a) and (b) — no third edit, ever — **even when a
fully-populated transform configuration is loaded and its rules would match that very content**. This
is the limb that keeps the existing fidelity cases non-vacuous now that a transform is configurable,
and it is the assertion that fails loudly if a mode check is ever wired wrong.

**A change to the effective set mid-session is allowed and must never be silent.** A session whose
client starts (or stops) asking for a transform changes the bytes at that turn; from then on the new
set is stable and I2 holds again. The record shows it: the ledger's rule ids and the mode move, and
`prefix_continuity` (the *predictor*, spec §6) drops for exactly that turn. Freezing the set for a
session's whole life (ADR-014's pattern) is deliberately **not** required here — a client that asks is
obeyed, and the cost is one declared prefix break, which is the price the metric exists to show.

### 5. Measurement: a saving is a difference between two worlds

One observation of a served request measures **the request**, never the saving: the counterfactual
world (the same content served without the edit) does not appear in the trace. Hence:

- At decision time a step's delta is **`inferred`** — a local byte-length attribution, because the
  dependency allowlist has no tokenizer (GAP-Q14), so `saved_input_tokens` from an edit is an estimate
  and says so.
- A figure becomes **`verified`** only where the pair exists: a control turn with the transform on
  and off over the same content, read from the upstream's own normalized `usage` (spec §7), or the
  replay path computing the counterfactual with the same code (DESIGN §9 — designed, **not served**).
  `autowork/program.md`'s D3 gate (*"inline tests green + the cache regression passes + verified net
  gain > 0"*) is exactly such a pair, and it is the only place a saving may be claimed.
- The ledger's honest arithmetic is `net = saved − added`: the added side counts the tee marker, a
  payload's re-encoding and any replaced text. A rule whose **verified** net is ≤ 0 is not adopted.
- Gates and external claims read `verified` only (AGENTS 4); `prefix_continuity` is a predictor and an
  absent measurement is never 0.

### 6. The relation to AGENTS constraint 1, and the text it needs

This ADR does not widen the boundary on the passthrough path: ADR-015's item 5 stands unchanged, and
there is still exactly one deletion primitive and one value-replacement primitive there. What it
supplies is the **definition of the path constraint 1 names** ("on the passthrough path"), because
without a definition that phrase is doing no work — no request is ever *not* on it.

Two sentences are needed in `AGENTS.md` for the binding artefact to say what the contract now says.
`AGENTS.md` is a human-gated path (ADR-012 item 2) and this decision does not edit it; the exact text
is handed over:

```text
A request that did not ask for a transform is on the passthrough path; on the passthrough path no
config key, plugin or default may enable a content transform, and exactly the two mutations of
constraint 1 apply. A request that asked for one (ADR-019) is on the transform path: there the
promise degrades to a declared, span-level edit list over tool/environment payloads, a labelled
ledger entry per step, and the three invariants (content determinism, per-set prefix monotonicity,
closed-mode byte equality). Never touch user intent, the system instruction, order, whitespace or
tool schemas on any path.
```

### 7. The first transforms, and what is *not* first

Ordered by expected value against the measured traffic shape (an agent loop resending tool payloads
every turn), and nothing here is a measurement:

1. **Tool/environment payload trimming** (`rules/tool_output.toml`'s `bash-log-noise` /
   `grep-hits-budget` / `diff-budget` class: strip noise lines, truncate long lines, cap lines, keep
   the `tee` marker). This is the largest lever available: it removes bytes from the region that
   grows fastest and that the client re-sends unchanged, so I1/I2 are satisfied by per-node rules.
   **Expected, not measured** — the honest number is D3's paired measurement, and the rule file's own
   fixtures are shapes (a synthetic log loses ~8 of ~12 lines), not evidence.
2. **Nothing else yet.** Output-side discipline (instruction injection, `max_tokens` clamping) is
   semantics-changing and excluded by item 3; P4-class rewriting is excluded by I2. A second admitted
   transform should be a rule *within* tier 1, admitted one at a time by D3's gate, not a new tier.

## Alternatives considered

- **(B) v0.1 does no content transform at all** (only content-preserving levers: prefix stability,
   caching, stickiness, plan-first) and the book is corrected to match — **rejected as the contract,
   adopted as the status.** As a contract it makes the product's headline capability permanently
   unimplementable-in-principle: constraint 1 would forbid the only shape the book promises, so the
  book could never be written to match the product except by deleting the product's value. As a
   status it is exactly true today (item 4 of the Background) and the book is corrected regardless —
   which is why this ADR and the book edit land together.
- **(C) Only appends may be permitted** (stable trailing fragments, prefix monotone by construction)
  — **rejected as the ruling**: it cannot express the removal of payload bytes, which is where the
  measured tokens are (the whole point of P1). Its discipline is **adopted wholesale** as invariant
  I2, applied to all transforms rather than to appends only.
- **Widen the boundary itself: a third mutation on the passthrough path, no opt-in** — rejected: it
  makes the fidelity assertion a function of configuration, erodes AGENTS 1 for every request, and
  would need the AGENTS edit to describe the *default* path rather than a second one.
- **Operator-forced transform mode (a config key)** — rejected in item 1; it also breaks the stance
  that a client's own request says what happens to it.
- **A router-owned opt-in *body* field** (the `router_meta`/routing-hint channel, mutation (a)) —
  rejected for v0.1: it extends `ROUTER_OWNED_TOP_LEVEL_KEYS`, and it puts the opt-in *inside* the
  bytes the closed-mode assertion examines. Recorded as an additive future channel.
- **A second endpoint for transforming requests** — rejected: two identities for one conversation
  (session, prefix ledger, stickiness) and a duplicated data plane, for a mode that is one field.
- **Transform the response instead of the request** (post-model rewriting) — rejected: the client
  re-sends the response as history, so a rewritten response is a rewritten request one turn later,
  without any of this discipline; and the response is not the client's bytes to edit. A response
  transform is not a transform in this sense.
- **Let a local tokenizer measure the saving directly** — rejected: it is off the dependency
  allowlist, and an unverifiable tokenizer in the money path is worse than an estimate that is
  labelled as one (GAP-Q14's existing ruling).
- **Require the transform to be reversible byte-for-byte at request time** (keep the original and
  swap back) — rejected as the *audit* mechanism: the modes's auditability is the declared edit list,
  not a retained copy; retention is `tee`, which is already declared unimplemented for retrieval
  (spec §4.4, and out of scope by `book/roadmap.md`).

## Rationale

- **It keeps both protected properties as assertions where they can be tested.** The default path's
  guarantee stays a byte equality with a finite edit list, testable with no configuration in the
  picture (I3's adversarial limb), and the transform path's guarantee is *declared* — which is
  strictly more information than "the bytes were equal", because a reviewer of a transform can see
  what it did.
- **The mode is what makes the promise honest.** The failure the repository is guarding against is not
  a transform that is off; it is a transform that changes bytes with nobody able to say which or why
  (ADR-007's undocumented rewrite). A requested mode with a ledger, a label and a declared fail-safe
  removes that class.
- **An opt-in costs the operator nothing and buys the invariant.** The operator already writes the
  client's configuration, so a header is as reachable as a config key — and unlike a config key it
  leaves the default path's guarantee unconditional.
- **It corrects the reader-facing promise without deleting it.** The book gains the true statement
  ("the lever that saves tokens today is cache fidelity and plan-first; payload compression is a mode
  that is not implemented yet") while the capability stays on the roadmap, which is the only
  arrangement in which the book, ADR-003 and the code can all be true at once.

## Consequences

- **Trace (additive, no `schema_version` move).** The `transform` group gains a mode field (present
  always, like `result.plan_switch`) and each step's ledger entry gains the edited path and its byte
  counts; `transforms[]` keeps its omitted-when-empty behaviour, which is why the mode is a separate
  field and not inferred from a non-empty array. spec §6 and DESIGN §12.6/§12.12 carry the shapes.
- **The fidelity cases keep their wording** (CONF-01/02/03/10/11/27) and gain the closed-mode limb of
  I3 as an assertion; that is a conformance change, so it belongs to the card that lands the mode and
  to a human-allocated ID (§12.8's rule — no ID is allocated by this ADR). CONF-16 leaves `#[ignore]`
  in the same change, with a fixture that is not vacuous.
- **P6's register row stays `contract-only`.** Its *container* clause (DESIGN §13.1) gains this ADR
  and spec §2.1/§4.4/§6 as contract homes, and its invariant gains "only in a requested mode"; the
  state column does not move, because no code lands here, and a register allowed to drift is the leak
  it claims to name (§13's own rule).
- **No new dependency, no store change, no new endpoint.** The mode is one header, one type, one
  ledger field and one shared composition step (§12.10.7) — the same step both forwarding paths
  already call, which is what keeps the streaming path from growing a second copy of the edit path
  (the leak pattern L2).
- **ADR-003's tier table is re-read, not edited.** Its "v0.1 ✅" column becomes a statement about
  *design*, not about shipped code: P0 (cache fidelity) is largely wired, P1/P2 are contract-only
  under this ADR, P3's plan half is wired (ADR-014), P4 stays deferred. The correction is recorded
  here because the ADR is append-only and the table is not this ADR's file.
- **A saving may not be reported until a pair exists.** Until replay or a control turn exists, every
  transform figure in `router stats` is `inferred`, `verified_savings_tokens` stays 0, and the book
  says so — the alternative (reporting byte-length math as measured savings) is the one thing
  constraint 4 forbids outright.
- **One gap is registered** (new, `docs/spec.md`-shaped, in DESIGN §12.9): a rule's `match_kind` is a
  "payload category declaration", but nothing in the wire format or the config says where the category
  comes from. It is the implementing card's to settle; the transform contract does not depend on it
  (`match_tool` is sufficient for tier 1).
