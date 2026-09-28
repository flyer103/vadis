# ADR-042 — the exact-match response cache: the authority that reverses an exclusion, the wall-clock collision stated as an owner decision, the byte boundary and what a hit does not claim

- Status: accepted
- Date: 2026-09-27
- Kind: **contract**. This card changes **no product byte**: `crates/**`, `tests/conformance/tests/**`,
  `config.example.yaml`, `.github/**` and `README.md` are outside its write set. What lands here is the authority
  record, the determinism collision's resolution, the key and the store, the trace and config contract, the
  measurement, and the DESIGN landing (§12.22 + §12.6 + §12.8 + §13.6). **The case and the code land together in
  one card** (§12.4), so the tree is never red in between — the R50-0 → R50-1 shape.
- Authority: the **owner's direction of 2026-09-27, item 3 of four**, relayed to the loop through this round's
  card `t_a338250d`; quoted verbatim in §1.1.
- Supersedes, **in the narrow sense of §1.2**: the **exclusion as an absolute** — `design/DESIGN.md` §13.6's
  cache row (`:4043` **at the base `bf96207`** — the revision this round replaces it in; at HEAD the row is `:4131`) and ADR-036's own-reasoning bullet (`ADR-036:288-294`), plus the competitiveness plan's
  *filing* of the item (`$HERMES_HOME/plans/2026-09-25_130500-router-competitiveness-plan.md:122-124`). It
  supersedes **nothing else** — §1.3 enumerates what it does **not** license, and the *semantic* cache stays
  exactly where it was (`docs/spec.md:19`, `:134`).
- Related: `AGENTS.md` constraints **1** (the byte boundary — the client's bytes and exactly two permitted
  mutations), **2** (**content determinism — never of turn number, wall clock, or RNG**), **3** (the
  observation boundary), **4** (no unverified savings), **5** (no fabricated prices), **8** (docs before code),
  **9** (**the measurement is not part of the search space**); ADR-002 (the Cordis runtime and the keyed config
  diff), ADR-005 (the trace is the only product → autowork channel), ADR-006/018 (integer `Nano`, one
  `currency`), ADR-009/010 (the log is the truth; intent before effect), ADR-012 (**the never-mutable paths**
  and the L1 envelope), ADR-015 (the byte mutations), ADR-019 + spec §2.1 (the mode channel and the three
  transform invariants), ADR-036 (**what a plugin may never own**, and the plugin-surface map), ADR-037 D6
  (`config_digest`), ADR-040 D5 (the keys a reload refuses), ADR-041 §4 (the single-owner rule's shape, which
  §9 mirrors); spec §1 (`:13`), §2 (`:24`), §2.1 (`:81`), §4 (`:137`), §4.3 (`:427`), §4.4 (`:463`), **§4.17
  (new)**, §6 (`:1749`), §7 (`:2086`), §9.2 (`:2371`), §9.3 (`:2500`); DESIGN §4 (`:65`), §12.6 (`:730`),
  §12.8 (`:959`), §12.10.4/§12.10.5 (`:1615`ff), §12.12 (`:2891`), **§12.22 (new)**, §13.1 (`:3985`), §13.2
  (`:3999`), §13.6 (`:4098`); `config.example.yaml:127-157`; `crates/router-plugins/src/assembly.rs:155,210`;
  `crates/router-proxy/src/forward.rs:457`; `crates/router-core/src/trace.rs:28,70,295-309`.
- Cases: **`CONF-88`** (the capability is **off by default** — green at the base *by construction*, and the
  arm a later config edit must not be able to move) and **`CONF-89`** (**a hit is the recorded bytes**: the
  byte-equality, the record's shape, the ledger's reconciliation). Both ids are claimed from a measurement
  (§10.3), both land **with the code they witness**, and `CONF-89` is red at this round's base (no store
  exists, so no hit can happen) while `CONF-88` is green at it (an absent capability is off).
- Numbering note (observation, not a decision): the register's newest landed entry is **`ADR-041`** (`:1`),
  and `ADR-041:28-31` records that **`ADR-042` was unused**. This ADR takes the number the round's cards fixed
  for it; the next free number is `ADDR-044`-free — i.e. `ADR-043` is the newest *other* entry, so the next
  free number after this one is **`ADR-044`**.
- **Line-number convention (so every `path:line` below can be checked).** Every reference resolves at **this
  branch's HEAD** — the revision that carries this ADR, by tree rather than by a fixed ancestor commit id — and,
  where a number is a *measurement of the base* commit `bf96207`, the sentence says so. This round's own cards
  shift both documents **and three code files** under it; every number below is re-derived at this branch's HEAD
  by the card that shifted it, as §11.4 promises, and §0 names its own revision where it quotes text this round
  replaces.

---

## 0. The step-0 assertions (the first fails, and the exclusion has moved)

The card asked for four assertions. Three hold as stated; **the first does not, and the exclusion has moved** —
so it is quoted where it actually is, and the line the card remembered is quoted too, because a document that
wrote this ADR against a remembered line number would have cited a row about `router-core/src/prefix.rs`.
Every line number in this section is re-derived at this branch's HEAD, like every other number in this ADR
(§11.4); the **quoted text** is the base commit `bf96207`'s, which is both the revision the card's assertion was
written against and the only revision that still carries the `excluded` row this round replaces — at HEAD that
row sits at `design/DESIGN.md:4131` and states the reversal.

| # | asserted | what HEAD actually says | verdict |
|---|---|---|---|
| 1 | *(the card's assertion, measured at the base `bf96207`)* `design/DESIGN.md:3917` carries the exclusion reasoning | **False, and the exclusion moved.** `design/DESIGN.md:4004` is §13.2's module-map row for `router-core/src/prefix.rs` (at the base, `:3917`): *"\| `router-core/src/prefix.rs` \| P5 (`prefix_blocks[]`, `prefix_continuity`, `extract_prefix_blocks:82`, `prefix_continuity:266`) + P8 (`attribute_tokens:235` is the GAP-Q14 proportional attribution → `inferred`) \| a tokenizer — the allowlist has none, which is why every derived token figure is labelled \|"*. The exclusion reasoning is at **`design/DESIGN.md:4043` at the base `bf96207`** (§13.6's "other surface \| mountable \| the contract it answers" table): *"\| semantic / exact-match response cache \| yes, but **excluded** \| a hit removes the upstream call ⇒ no `usage` object ⇒ the saving is `inferred` forever (AGENTS 4), and the 1 client request = 1 upstream call correspondence the accounting rests on goes (ADR-036, \"What this ADR does not decide\") \|"* | **quoted where it was** (`:4043`, at the base; `:4131` at HEAD), and §12.6 replaces that row |
| 2 | `docs/spec.md` still lists the response cache among v0.1's non-goals | **True.** `docs/spec.md:13` is the heading — *"**v0.1 non-goals** (explicit exclusions — do not add them on the side):"* — and `docs/spec.md:19` is the row: *"\| Semantic response cache, context summarization \| Large conflict surface with prefix caching; a measured ledger is needed first (P4) \|"* | holds; §11.3 amends the row **additively** |
| 3 | no `ADR-042` exists yet | **True.** `ls design/decisions/ADR-042*` → *No such file or directory*; the register holds `ADR-001…ADR-041` + `ADR-043` (42 files). | holds |
| 4 | `main` is at the R50 merge | **True, as the merge being an ancestor of the cut.** The cut point is `bf96207` = `origin/main`, and `git merge-base --is-ancestor 54e4864 HEAD` → **rc=0**: the R50 merge `54e4864` (*"Merge round/50-metrics — R50: `GET /metrics`, the operator's scrape surface (ADR-041)"*) is in this branch's history. The branch is `round/51-cache`, cut from `main` at `bf96207`. | holds |

---

## 1. The authority for building a capability the record said not to build

### 1.1 The owner's authorisation, verbatim and dated

**The owner's direction of 2026-09-27 (item 3 of four)**, relayed to the loop by this round's card
`t_a338250d` (card body, "Round R51", first paragraph), verbatim:

> *"exact match only, **off by default**, **declared in the trace**, and **never counted by any gate**"*

The relay states where it sits in the owner's ruling of that day: *"Owner-authorised 2026-09-27 (item 3 of
four)"* — the same four-item ruling whose **item 2** R50 carried (ADR-041 §1.1 quotes it: *"authorised: amend
`CONF-46` and `spec` §9.3, and implement `/metrics` to contract"*). Three facts about the sentence are part of
the record, and none of them may be inferred away:

1. **It is an owner act, not a loop outcome**, and the record had already filed this item as one. ADR-036:25
   says of the exclusion: *"The semantic / exact-match response cache exclusion in D6 is this ADR's own
   reasoning, not an owner's word."* An artefact that is not an owner's word is exactly what an owner's word may
   reverse — and the reversal is the owner's, recorded here, not a round reopening a decided question. The
   standing loop-side rule (`autowork/harness/r49-0/CLAIM-SOURCES.md:360`, row W2, quoting the cut-spec card
   `t_1d86acb2`) is *"do not reopen a rejected item; a reopening must supply a measurement the dossier does not
   have"*; that rule binds **the loop**, and §7 supplies the measurement anyway.
2. **It is dated and scoped**: *this* capability — the **exact-match** cache — and four of its properties
   (exact match only; off by default; declared in the trace; never counted by a gate). §1.3 lists what it does
   not reach.
3. **It authorises the capability, not its shape.** The key, the store, the lifetime rule, the record's fields
   and the module's home are **not** in the owner's sentence; they are this change's ruling, which is what §3,
   §4 and §9 freeze. The one question the ruling *poses* without answering — whether the lifetime may be
   time-based — is named in §2.4 as the owner's, not taken here.

### 1.2 What the ruling supersedes — and what the prior artifacts actually said

Every claim in this table was re-read at the base `bf96207` (this card's HEAD when it was written) or at the path named; the commands and
their raw output are in `autowork/harness/r51-0/anchors.txt`.

| Artifact | What it says, verbatim or closely | Superseded? |
|---|---|---|
| `design/DESIGN.md:4043` (§13.6's cache row, **at the base `bf96207`** — this round replaces it; at HEAD the row is `:4131`) | *"\| semantic / exact-match response cache \| yes, but **excluded** \| a hit removes the upstream call ⇒ no `usage` object ⇒ the saving is `inferred` forever (AGENTS 4), and the 1 client request = 1 upstream call correspondence the accounting rests on goes (ADR-036, \"What this ADR does not decide\") \|"* | **Yes, the `excluded` verdict — and only it.** The row's *reason* survives intact and is the reason this ADR spends its longest section on the record class (§4) and the label (§5): the saving is `inferred` for ever, and the 1 : 1 correspondence **is** deliberately broken. The row is replaced in §12.6 by one that says so. |
| `design/decisions/ADR-036-minimal-core-and-plugin-surface.md:288-294` | *"**Semantic / exact-match response caching.** Excluded by this ADR's reasoning, not by an owner's word: a hit removes the upstream call, so no `usage` object exists and the saving can only ever be `inferred` — it would be the first shipped feature whose headline benefit cannot enter a gate (AGENTS 4), and it breaks the 1 client request = 1 upstream call correspondence the accounting rests on. If it is ever built it must be a plugin, **off by default, disclosed in the trace, and excluded from every gate**"* | **The exclusion, yes; the four conditions, no — they are obeyed.** Every clause after "If it is ever built" is a *requirement this ADR discharges*: a plugin (§9.2), off by default (§6.1), disclosed in the trace (§10), excluded from every gate (§5.2). ADR-036:25's classification of the exclusion as its own reasoning is what makes the reversal the owner's to make. |
| `$HERMES_HOME/plans/2026-09-25_130500-router-competitiveness-plan.md:122-124` (§T3-B) | *"Attractive (every L2/L3 competitor has one) but it breaks *\"one client request → that provider call\"* … If built, it must be off by default, declared in the trace, and never counted by a gate."* | **Yes, as a filing.** It is the same requirement list as ADR-036's, filed under "the real feature gaps (strategic, each collides with a frozen constraint)"; the owner's ruling is that filing being taken. Nothing in it measured anything. |
| `$HERMES_HOME/plans/2026-09-25_130500-router-competitiveness-plan.md:132-134` (**"Not doing"**) | the heading *"## Not doing (write it down so the loop does not drift into it)"* and its line *"**Semantic cache** · MCP / A2A / gRPC ingress · dashboards / `/metrics` before v0.2 · SDKs · …"* | **The `/metrics` clause was already discharged by ADR-041 §1.2; the *semantic* clause is not touched and stays filed.** This ADR builds an **exact-match** cache: a byte-identical repeat of one session's own request, matched by digest of the client's bytes. Similarity matching, embedding lookup and any content-equivalence claim are the *semantic* cache, and they stay out (§1.3, `docs/spec.md:19`). |
| `docs/spec.md:19` | *"\| Semantic response cache, context summarization \| Large conflict surface with prefix caching; a measured ledger is needed first (P4) \|"* | **No — the row is amended additively, not falsified** (§11.3). R49's contract predicted exactly this and required this shape: *"an off-by-default exact-match cache does not falsify `docs/spec.md:19`'s *semantic* response-cache non-goal (a different thing), so that row stays"* (`CLAIM-SOURCES.md:360`). The row's *only* defect after this round is silence, not error: §1's table would otherwise not name a capability the config can turn on. §11.3 replaces the row with one that states both halves, and the drift of that claim row is registered (`R51-0-F1`, §14). |
| `autowork/program.md:34-42` (the gate table) | the four blocking gates — protocol fidelity, cache, cost, latency — and the warning-level semantic corroboration | **No, and this ADR binds itself to them** (§5.2, §6.2). Gate definitions are outside the loop's mutable scope (AGENTS 9 / ADR-012); nothing here moves one. |
| `tests/conformance/**`, the four frozen corpora | the assertions and the corpus digests (`607ffac6…`, `0f8b7edb…`, `ab3cf279…`, `e07405bf…`) | **No.** No assertion, no gate and no corpus digest is touched: `CONF-88`/`CONF-89` are **added** with the code they witness (§10.3), and the measurement in §7 reads the corpora without editing them. |

### 1.3 What the authorisation does **not** license

- **A savings claim.** Nothing in the sentence says "this saves money", and §5 makes the label permanent:
  every figure a hit produces is `inferred`, and no gate may count one. A reader who finds the phrase "off by
  default, never counted by a gate" and concludes *"so it may be advertised"* has read it backwards: the four
  clauses are exactly the *price* of building it.
- **A semantic cache, or any content-equivalence claim.** Matching is a **digest of the client's bytes** (§3.1);
  no tokenizer, no normalization, no similarity, no model, no embedding enters the decision.
- **Any change to constraints 1–4.** The byte boundary (§3), content determinism (§2), the observation boundary
  (§8) and the `verified`/`inferred` labelling rule (§5) are constraints this capability must satisfy, not
  properties it may renegotiate. An amendment to any of the four is an `AGENTS.md` decision (§14 opens none).
- **A time-based lifetime.** §2.4 names the question and refuses to answer it for the owner.
- **Any change to a gate definition, the fixed corpus, a conformance assertion or the L1 envelope** (AGENTS 9 /
  ADR-012: *"the gate definitions, the fixed corpus, the conformance assertions and the L1 envelope are outside
  the loop's mutable scope"*). §6.2 states the consequence: the gate corpus and the L1 envelope run **with the
  cache off**, and **no gate, corpus or assertion may be changed to accommodate it**. If a future round wanted
  one changed, that is a human decision, not a loop outcome.
- **Any other surface, route or key.** No `/v1/cache`-style endpoint, no CLI subcommand, no query parameter, no
  second store, no persistence of the store to disk (§3.4).

---

## 2. The determinism collision (constraint 2) — the deepest question in this round

### 2.1 The collision, stated in the constraint's own words

`AGENTS.md` constraint 2: *"Every transform must be a pure function of (content, stable config) — never of turn
number, wall clock, or RNG."* A response cache whose store is keyed on content **and bounded in time** answers a
request differently depending on **when** the request arrived: the same bytes, the same session and the same
config produce a hit at 10:00:00 and a miss at 10:00:01. That is a transform whose output depends on **the wall
clock**, and it collides with constraint 2 **as written**. The collision is not hypothetical — the naive
implementation walks straight into it, because the configuration already carries a lifetime:

```
docs/spec.md:146   session:  { key_sources: ["prompt_cache_key", "header:session-id", "header:thread-id"], ttl: 12h }
```

`session.ttl` is the obvious value to reuse for a session-scoped store, and reusing it would make every hit
verdict a function of elapsed wall-clock time. Two facts keep the trap avoidable rather than merely survivable:

- **`session.ttl` reaches nothing on the wire today.** spec §4.15's own invariant table, row 4: *"A revision
  that changes only keys the upstream never sees — `session.ttl`, the cache and breakeven defaults, prices,
  quotas, `currency`, `region`, a path or a default — produces byte-identical outbound requests, so a live
  conversation's cached prefix is untouched"* (`docs/spec.md:1504`), and `docs/spec.md:1128` lists
  `session.key_sources` among the keys that are *"display-only for the same reason"*. So today `session.ttl` is
  a reporting/grouping knob with **no serving-path effect**; giving it one would be a *new* wire-visible fact
  smuggled in through a key the reload promises can change freely.
- **The prefix promise is a statement about bytes that are sent.** constraint 2's operative sentence is *"The
  prefix on turn N must be a prefix of turn N+1"* — about the outbound bytes of two turns. A cache never
  rewrites a turn; it can only make a turn's call not happen (§2.5).

### 2.2 The contract this ADR freezes: **no time-based lifetime**

The store's lifetime is **the plugin's fiber** (§9.2) and nothing else:

- **No expiry by clock.** No TTL, no `session.ttl` reuse, no "stale after N seconds", no timestamp read on the
  lookup path. An entry leaves the store only by **eviction under the frozen bound** (§3.4) or by the fiber's
  own unload (a config change that rebuilds the entry, or `disabled: true`).
- **What is therefore deterministic.** For one session, one revision and one client byte sequence, the sequence
  of hits, misses and evictions is a pure function of that sequence and the store's frozen constants: no clock
  read, no RNG, no turn counter consulted as an input.
- **What constraint 2 protects stays protected.** For the same client byte sequence, every request the cache
  *does* forward carries **byte-identical outbound bytes** to the request the same build without a cache would
  have forwarded — in the same order. The cache can only **remove** calls from the sequence; it can never alter
  one (§2.5). That is the property the conformance suite can assert, and it is the property an operator cares
  about, because upstream prompt-cache stability is about the bytes that arrive.

### 2.3 The alternatives, with their costs

| Lifetime rule | Gain | Cost | Verdict |
|---|---|---|---|
| **(a) Per-session, content-keyed, no time expiry** — the store lives as long as the fiber, entries leave only by the frozen bound | constraint 2 is satisfied literally: no clock, no turn counter, no RNG; the hit pattern is replayable from the byte sequence | a long-lived process holds stale response bytes indefinitely; a client that repeats a request an hour later gets the recorded bytes (which is *also* the feature's definition — see the honesty sentence, §3.2) | **FROZEN HERE** (§3, §9) |
| **(b) A time bound accepted as a declared deviation** — a TTL, in effect a clock read on the lookup path | entries age out; memory self-heals; matches the intuition of "a cache" | constraint 2 is **violated as written**: the same bytes hit or miss by elapsed time, so no gate can replay the feature's own behaviour; it would need `AGENTS.md`'s constraint 2 amended or a **declared deviation** recorded at the owner's level, exactly the class a round may not take | **NOT TAKEN — the owner's decision** (§2.4) |
| **(c) No bound at all** — every distinct key kept for the process's life | nothing is ever evicted; the simplest rule | memory is bounded only by traffic: response bodies are the upstream's choice of size, so one busy session can exhaust the process | rejected: a resource bound is a constant, not a clock, and it is the cheaper of the two rules to state |
| **(d) Content-only key, no session component** | serves repeats across sessions | a response recorded in one session would be served to another: the session is the unit of upstream affinity and of the record's own `session` attribution, so the record would name a session the bytes did not come from | rejected: §3.1 |
| **(e) Persist the store across restarts** (the SQLite store, a file) | hits survive a restart | adds a second durable artefact and a schema, inside the state dir whose writer lock and event-log discipline are P7's; buys nothing the owner asked for | rejected for this round; a widening with its own trigger (`R51-0-F4`), §14 |

### 2.4 The owner decision this ADR **surfaces instead of taking**: may the lifetime be time-based?

**The owner's ruling as relayed carries no lifetime wording at all.** The four clauses quoted in §1.1 say what
may be matched, what the default is, what the trace must disclose, and what no gate may count — nothing about a
TTL. The "TTL" in this repository's vocabulary is `session.ttl` (`docs/spec.md:146`), and the only way a TTL can
enter this feature is by an implementer reusing that key, which is precisely the move §2.1 shows to be a
constraint-2 collision.

The decision this ADR does **not** take, and its owner:

| question | this ADR's position | who decides | trigger |
|---|---|---|---|
| May a store entry expire by **wall clock**? | **Refused for this round**, and refused as a *loop* decision in any round: a time bound is a declared deviation from `AGENTS.md` constraint 2 as written, and constraint 2 is the charter's, not the loop's. The default shape frozen in §3 has no clock read. | **the owner** — a one-line ruling would change §3.4's eviction rule and nothing else | the owner asks for the store to age out; or a serving incident shows unbounded staleness mattering in practice |

Note what the refusal does **not** mean: it is not a claim that a TTL is wrong engineering. It is a claim that
**the loop may not decide it**, because it changes what "the same request" means across time and therefore what
a replay of a trace can assert. That is the same boundary ADR-041 §3.3 drew around the metrics window (a frozen
constant, with "a configurable window" registered rather than smuggled in).

### 2.5 What "the prefix on turn N+1" means when turn N was a hit

Precise definition, because the naive reading ("the prefix continuity of the hit's turn") has no referent:

- On a **hit**, no request leaves the process. The turn has **no outbound bytes** — not a rewrite, not a
  restart, an absence. `prefix.continuity` for that record stays what it always is for a request with no
  previous *outbound* request of the session: the ratio against the session's previous request's blocks, i.e.
  an ordinary value computed from the blocks the request does carry; the field is **not** redefined and **not**
  forced to `1.0` or `null` by a hit.
- On the **next miss** of that session (`N+1`), the outbound bytes are **exactly** the bytes the same build
  with the cache off would send for turn `N+1`: the cache did not touch them, and the client's own bytes for
  `N+1` are untouched (constraint 1's two mutations, unchanged). The prefix statement therefore holds **between
  the turns that are sent**: for the subsequence of turns the cache forwards, prefix monotonicity is the same
  property it was, measured on the same bytes.
- **The one honest difference**, stated so no reader has to find it: an *upstream-registered* prefix cache used
  to be warmed by turn N's call. If turn N was a hit, its call did not happen, and the upstream never stored
  turn N's prefix. So the *upstream's* cache state after a hit is weaker than after a miss at N — cost that the
  router does not pay (no call, no charge) but that a subsequent turn may pay in `cached_tokens`. This is not a
  determinism violation (nothing here depends on a clock) and it is not hidden: it is the same fact as "a hit
  makes no claim about the upstream", and it is measurable as a `cached_tokens` comparison on a later turn in
  the same session. It is registered as a reportable consequence (`R51-0-F5`, §14) rather than asserted here,
  because **the frozen corpus cannot measure it** (§7.3).

---

## 3. The byte boundary (constraint 1): the key, what a hit proves, and what it never stores

### 3.1 The key — five components, each with its owner and its reason

```
key := ( protocol.protocol_in          — from the request's own path (spec §2)
       , config_digest                 — the loader's own digest of the revision in force (ADR-037 D6)
       , identity.session              — spec §4's `key_sources`, preferring `prompt_cache_key` (§4, :146, :1758)
       , transform_mode                — spec §2.1's word: the mode this request asked for (§2.1)
       , sha256(inbound body bytes as received) )   ← the content component, and the ONLY one that is bytes
```

- **No normalisation happens, and there is nothing to normalise.** The digest is taken over the body bytes
  **exactly as the request carried them** — the same bytes constraint 1 calls *"the client's bytes"* — before,
  and independently of, the two permitted mutations (`router_meta`-class key deletion and the top-level `model`
  value replacement, ADR-015). No parse → reserialize, no whitespace folding, no key reordering, no
  `prompt_cache_key` stripping, no case folding. The two mutations remain **exactly two** (constraint 1), and
  neither feeds the key: the key is computed from the client's own bytes, so a change to a mutation cannot
  silently change what is matched.
- **Why the other four components are not bytes, and why each is necessary.** `protocol_in` is path-derived, so
  the same bytes posted to two protocol endpoints would otherwise collide while resolving differently.
  `config_digest` is the revision: without it a hit could straddle a config change that moved a route, a price or
  a rule set. `session` is a header-or-body fact, and where it comes from a header (`header:session-id`, the
  composed corpora's own source) it is *not* in the bytes at all; two sessions can legitimately carry identical
  bodies, and a response recorded in one makes no claim about the other (upstream affinity is per session).
  `transform_mode` is a header (spec §2.1: *"The opt-in is a request header"*), so two requests with identical
  bodies and different mode words produce *different outbound bodies* — the key must separate them.
- **A component that is already inside the digest is not repeated.** The client's own `model` string, its
  `stream` scalar, its `store` flag and everything else live in the body bytes and are therefore already
  covered; the key names only what the bytes cannot express.
- **The whole key is reconstructible from the record.** §10.2's `cache.key_digest` plus the record's own
  `protocol.protocol_in`, `config_digest`, `identity.session` and `transform_mode` are the five components: an
  auditor can recompute a hit's key from the record that discloses it, and from the raw request if it is kept.
  **This is a design invariant, not a nicety** — it is what makes §4's reconciliation checkable by a stranger.

### 3.2 What a hit proves — and the sentence that is the honesty of the whole feature

A hit proves exactly two things:

1. **these exact bytes were sent before** — in this session, under this revision, under this protocol, under
   this mode; and
2. **the response bytes returned then are the response bytes returned now**, verbatim, byte for byte.

It therefore makes **no claim about what the upstream would answer now**. These are recorded bytes from an
earlier moment, served again. That sentence is the feature's contract, and it is the vocabulary a reader of a
hit's record must have:

> **A hit is a replay, not a prediction.** The bytes came from a recorded response; the router asserts nothing
> about what the provider would answer for this request at this moment — not its content, not its price, not
> its liveness.

Two consequences are part of the contract rather than of its commentary. `result.upstream_status: null` and
`result.upstream_ms: null` on a hit (§10.1) — an absent measurement, never a `0` (spec §6: *"an absent
measurement is absent, not 1.0 and not 0.0"* is that file's own stance). And the reported figure derived from a
hit is `inferred` for ever (§5).

### 3.3 What is never stored (fail-closed, and why each exclusion exists)

| not stored | why |
|---|---|
| a response whose HTTP status is not `2xx` | an error is not an outcome worth replaying: an upstream `429`/`503` recorded once and replayed later would present a transient refusal as a present fact, which is worse than a miss and unreadable in the ledger. Only a successful, complete response is a candidate. |
| an **incomplete** body — a stream abandoned mid-way, a client disconnect, an upstream that died mid-stream, a body the router could not read to its end | "the response bytes returned then" has no referent for a partial response. A prefix of a stream is not a response; storing one and replaying it would hand a client a truncated answer as a complete one. Nothing is stored until the body is complete. |
| a response whose body exceeds the store's byte bound (§3.4) | the bound is a memory bound, not a reason to store something partial. A too-large body is simply not a candidate. |
| anything at all, when the request carries no session identity | the session is a key component (§3.1); with no session there is no key, so the request is neither looked up nor stored. Fail-closed, and it keeps `session: null` from becoming a shared bucket. |

### 3.4 The store: in-process memory, one owner, a frozen bound

```rust
// crates/router-core/src/response_cache.rs  (the domain half; no I/O, no clock)
pub const MAX_ENTRIES: usize = 1024;          // the entry bound
pub const MAX_STORED_BYTES: u64 = 64 << 20;   // the byte bound (64 MiB) — an entry bound alone cannot bound memory,
                                              //  because a response's size is the upstream's choice
```

- **In-process memory only.** No file, no SQLite table, no row in the event log (§4.5's state store is not
  touched — P7 keeps one writer per state dir, and this capability is not a writer of state). A restart is a
  cold store, which is a *miss* for every session: it changes **how many calls** are made, never **what any
  call sends**.
- **Eviction is FIFO by insertion, under both bounds** — a pure function of the insertion sequence and two
  constants, which is what §2.2's determinism claim rests on. A new entry that would exceed either bound evicts
  in insertion order until it fits; an entry whose own size exceeds `MAX_STORED_BYTES` is never stored.
- **The bounds are frozen constants, stated in-band** (spec §4.17), in the shape ADR-041 §3.3 froze the metrics
  window in: no config key, no query parameter. "A configurable capacity" is the registered widening
  (`R51-0-F3`, §14).
- **The store holds response bytes and nothing derived from them.** It is not a second ledger: no token
  estimate, no price, no count that a report could sum (§9.3).

---

## 4. "One client request = one upstream call" — what replaces it

The equality is **deliberately broken**: on a hit, one client request produces zero upstream calls. What is
preserved instead is a set of four properties, each of which an auditor can check from the trace alone.

### 4.1 One trace record per client request, and the record says no call was made

- **Every client request still writes exactly one `DecisionRecord`** (spec §6's heading; ADR-005). The hit is not
  a new record class in the *sense of being optional* — the request that a hit served is recorded exactly as any
  other request is, from the same writer (`Accountant`, DESIGN §12.6's single-writer rule).
- **The record carries the fact that no upstream call was made** in the vocabulary that already means it:
  `usage_missing: true` (spec §6: *"`usage_missing: true` means \"no usage was measured for this request\" … A
  request that never reached an upstream is in the same class"*, `docs/spec.md:1792-1794`) together with
  `result.upstream_status: null` and `result.upstream_ms: null` (§10.1).
- **The group's presence is the assertion.** `cache` (§10.1) is written **iff** this record's response came from
  the store. A reader asks one question — *does `cache` exist?* — and gets one answer. That is the shape
  `transforms[]` already uses (*"`transforms[]` **iff** bytes changed"*) and that `transform_mode` exists to
  disambiguate; no second word is invented for "a hit happened".

### 4.2 The record names which prior record the bytes came from

`cache.replayed = { request_id, session, turn_index }` (§10.1) — the **id of the record the bytes came from**,
its session, and the turn index that record itself reported. The `request_id` is the join key the whole trace
already uses (spec §6's identity group; the `events` join is on `request_id` + `event_id`), so the reference is
resolvable with the tooling that exists, and the revision needs no fourth member: the key pins `config_digest`
(§3.1), so the source record is necessarily of the **same revision** as the hit — stated here so a reader does
not have to derive it.

### 4.3 The reconciliation property, in one line

> **A sum over records cannot double-count a replayed response:** a hit's record contributes **no** usage, **no**
> money and **no** upstream call to any sum — it is `usage_missing`-class (spec §6: *"The flag is what keeps such
> a record out of every rate, every sum and every gate — `router stats` counts it on its own line and prices it
> nowhere"*, `docs/spec.md:1796-1797`) — while the bytes it returned are attributed to the **source** record's
> id, which is the record that owns the one measurement.

Three checks an auditor can run, and none of them needs the store:

1. `Σ` over a window's records of the measured `usage` is **unchanged** by the presence of hits (each hit adds
   nothing to the sum).
2. For every record with a `cache` group, `cache.replayed.request_id` names a record in the same window (or a
   still-older one, if the window cuts it) whose `identity.session` equals the hit's own — i.e. the reference is
   *inside* the session, never across sessions, which is §3.1's session component restated as a trace check.
3. The number of upstream calls in the window equals the number of records **without** a `cache` group that
   carry an `upstream_status`; equivalently, hits can never be counted as calls.

**A cache that makes the ledger unreconcilable is a defect regardless of how much it saves** — which is why
these three checks are limbs of `CONF-89` (§12.4) and are the reason the store holds no derived figure (§3.4).

### 4.4 Where the lookup sits in the pipeline, and what that buys

The lookup is the **last step before the upstream attempt**: after admission (spec §4.7), after the body is read
and bounded (§4.13), after route resolution (§3), after the guard chain (§4.6/§4.2) and after the transform
compose step (§2.1/§12.12). A hit replaces **exactly one** thing: the call. Consequences, all of them
deliberate:

- **A refusal is never softened by the cache.** A request the guard refuses, or a route the walk cannot serve,
  is refused — the cache does not answer for it, and cannot become a policy bypass.
- **The record of a hit is the ordinary record of that request** with one group added: same identity, same
  `decision.provider`/`model` (the route this request **would** have called — the record names what it is *not*
  calling), same `decision_ms`, same `prefix` computation, same `config_digest`. That is what makes the
  round's strongest verifiable property stateable: **a cache-on run and a cache-off run of the same client
  traffic produce records that differ only inside the hit's own group** (plus the omitted call's own fields).
- **The clock that is read is the pipeline's existing one, not a cache clock.** No new time source enters
  (§2.2); `result.overhead_ms` measures what it always measured, so the latency gate's quantity is unchanged.
- **A cheaper path (a lookup before resolution) is explicitly not taken**, and is registered as a widening if
  profiling ever asks for it — taking it would change the record's own fields (a hit would have no resolved
  route to name), which is the thing this section exists to avoid.

---

## 5. The label (constraint 4): `inferred`, always, and no gate may count it

### 5.1 Why, in one sentence

**The `usage` a hit's figure would rest on was measured by a *different* request, and the counterfactual — what
this provider would have charged and returned for these bytes now — was never measured at all**, so the figure
is a local, counterfactual-adjacent arithmetic and is `inferred` by spec §7's own definition (*"local tokenizer
estimate, no control"*, `docs/spec.md:2091`) — permanently, not until a pair exists: no control turn can exist
for a request that was never sent.

### 5.2 No gate may count it

- **Gate definitions are outside the loop's mutable scope** (AGENTS 9 / ADR-012), and the cost gate counts only
  `verified` figures (`autowork/program.md:40`: *"the `verified` $ and token ledger of a fixed-trace replay"*;
  `:32`: *"Only numbers in the `verified` convention may enter a gate or an external report"*). A hit's figure is
  `inferred`, therefore it cannot enter one — this is a consequence of the frozen rules, not a new favour.
- **The label is carried on the record, and the exclusion is structural.** `cache.verdict: "inferred"` (§10.1,
  the same one-word vocabulary `forward.rs:457` already writes — *`verdict: "inferred"`* with its comment *"A
  decision-time figure is `inferred` and says so"*), and the record is `usage_missing`, which the readers already
  exclude from every rate, sum and gate (spec §6). **No new word is invented**: `verified`/`inferred` is the
  vocabulary, and a hit never says the other one.
- **The gate corpus and the L1 envelope run with the cache off** (§6.2).

---

## 6. Off by default: the flag, the default, and the case that says so

### 6.1 The flag and its default

The capability is a **tier-A plugin mounted from the `plugins:` list** (ADR-036's own requirement: *"If it is
ever built it must be a plugin"*, `ADR-036:292`), and it is **off** unless the operator asks for it:

```yaml
plugins:
  - id: response-cache
    kind: builtin/response_cache
    config: { enabled: false }        # enabled: false IS the default: the key may be omitted entirely
```

- **`config.enabled` is the capability's own opt-in, and its default is `false`.** An entry that omits it, or
  writes `false`, is **mounted inert**: it registers nothing into the serving path, reads no store, writes no
  record and answers no request differently from a build that does not know the kind.
- **Two switches, one reason each, stated so they are not confused.** `disabled: true` is the *mechanism's*
  start-up switch, honoured for every plugin (spec §4.3: *"`disabled` is honoured at start-up"*,
  `docs/spec.md:434`, `:448`); `config.enabled` is the *capability's* opt-in. They are not synonyms: the
  mechanism's default is "the fiber is loaded", so if the capability's default were "on", **listing the plugin
  would enable it** — and that is the failure this key exists to prevent. The rule, in one sentence: *the fiber
  may be loaded and the capability still off.*
- **`config.example.yaml` does not ship a cache entry** either: the example config is the exemplar, and an
  exemplar that mounts an inert plugin invites the copy-paste that flips one word. The spec documents the kind
  and its one key (§4.17); the example file is not touched by this round.

### 6.2 The case that says so, and the frozen measurement it protects

- **`CONF-88`** asserts the default (§12.4): with the exemplar root (no cache entry) and with an entry whose
  `config.enabled` is absent or `false`, a byte-identical repeat inside one session is **still forwarded
  upstream** — two calls, two records, no `cache` group anywhere, and the outbound bytes of both calls
  byte-identical to the same build's bytes with the kind absent. It is **green at this round's base by
  construction** (the kind does not exist, so nothing can be on), which is a property, not a weakness: it cannot
  be made green by the implementation, so it pins the *default* rather than the feature.
- **Its red control is a sabotage run, not a case.** A build whose loader defaults that key to `true` must turn
  `CONF-88` red on its first limb; the rig that proves it is recorded under `autowork/harness/r51-*/` (§12.5's
  control list), in the shape R50-1 used for `CONF-46` (`autowork/harness/r50-1/sabotage-control-conf46.md`).
  The point of recording it is that a later config edit **cannot** enable the capability without a case turning
  red.
- **The gate corpus and the L1 envelope run with the cache off.** The four frozen corpora (§7), the four blocking
  gates (`autowork/program.md:34-42`) and the L1 envelope's own parameter runs are executed against roots that
  do not mount the capability; no gate reads a cache field, and no gate, corpus, threshold or assertion may be
  changed to accommodate the feature (AGENTS 9 / ADR-012). If a future round wanted the corpus to include a
  cache-enabled arm, that is a human decision — and §7.3 explains why the corpus **cannot** answer the question
  such an arm would ask anyway.

---

## 7. The measured payoff — a number, before any claim

### 7.1 What was measured, and how

**The frozen corpora** are the only corpora this repository has, and `harness.replay.Corpus` is the one reder of
them; the measurement is a script beside this ADR:

```
cd autowork && uv run python harness/r51-0/measure-repeat-rate.py
   → autowork/harness/r51-0/repeat-rate.txt   (the printed receipt, quoted below)
   → autowork/harness/r51-0/repeat-rate.json  (the same numbers, machine-readable)
   → autowork/harness/r51-0/corpus-verify.txt (all four corpora `ok: True`, digests unmoved)
```

Method, in full, because the number is worth nothing without it:

- the corpora are **loaded through the signed-corpus rule set** (`harness.replay.Corpus`, replay.py:204-285),
  which **refuses** any item whose bytes do not hash to the manifest's own `sha256` — so loading *is* the
  integrity check, and a corrupted body is a refusal rather than a number;
- the key under test is `(session, sha256(body bytes as stored))` — session-scoped, content-keyed, **no
  normalisation of any kind** (the counting path opens no parser on the bytes);
- an item is a **repeat** iff an **earlier** item of the **same session** (the corpus's own order: capture items
  by `seq`, else manifest order) carries the same body digest;
- the store is assumed **perfectly warm** — no eviction, no capacity bound, everything ever recorded still held.
  That is an *upper bound* on what the feature can serve, so the reported rate cannot be flattered by a tuning
  choice;
- a second figure is measured so the session component's cost is a number rather than an assertion: the
  **cross-session identical** class (one body digest under two different sessions), which a session-scoped key
  cannot serve and a content-only key could.

### 7.2 The number

| | codex-pair-2026-09-22 | codex-pair-2026-09-22-grafted | l2-composed-pair-2026-09-24 | l2-composed…-live | **total** |
|---|---|---|---|---|---|
| items | 14 | 14 | 3 | 3 | **34** |
| sessions | 2 | 2 | 2 | 2 | 8 |
| distinct `(session, content)` keys | 14 | 14 | 3 | 3 | **34** |
| **within-session byte-identical repeats** | **0** | **0** | **0** | **0** | **0** |
| repeat rate | 0.0 | 0.0 | 0.0 | 0.0 | **0.0** |
| cross-session identical digests | 0 | 0 | 0 | 0 | **0** |
| items with no session | 0 | 0 | 0 | 0 | 0 |
| client `stream` scalar | 14 true | 14 true | 3 false | 3 false | 28 streaming / 6 buffered |

**The honest headline: 0 of 34 — a measured zero (0.0%).** Every request in the four frozen corpora is
distinct bytes, so the class an exact-match, session-scoped cache can serve **is empty on this corpus**, and the
cross-session class is empty too (so a session-blind key would buy nothing either). A documented null with a
measured rate is a legitimate outcome of this round; the ADR carries it without drama, and §14 records what it
means for the feature's justification.

### 7.3 Why the class is empty — measured, not argued

1. **Every session in the capture corpora is a cumulative resend.** Both codex sessions' seven items grow
   strictly: `35690 → 36784 → 37324 → 38648 → 39632 → 40215 → 42519` and
   `35690 → 36412 → 36961 → 38293 → 39280 → 39691 → 41995` (and the grafted freeze's final turns jump to
   129850 / 367081). The bytes of turn N are a strict prefix of turn N+1's input, so two turns of one session
   cannot be the same bytes — that is what the shape *is*.
2. **The same script in a second session is not byte-identical.** `it-01` and `it-08` are the identical
   script's first turn in the corpus's two sessions and are the same length (35690 B) — and differ in **22
   bytes**, the first at offset **35576**, and the difference lies **entirely inside the two `prompt_cache_key`
   value spans** (measured: substituting one session id for the other reproduces the other body exactly). The
   session id is inside the client's own bytes, so "the same content in another session" is not "the same
   bytes" — which is the same fact as §3.1's rule that the session is a key component.
3. **The composed corpora are pairs, not repetitions** — one control turn and one treatment turn per arm, by
   construction (`l2-composed-pair-2026-09-24`'s own manifest: the pair *is* the measurement).

### 7.4 What this number does and does not license

- **It does not license a benefit claim.** "0 of 34 on the frozen corpus" is what a savings statement must carry
  as its rate: any sentence of the form "the cache saves X" is `inferred` **and** must state the rate it was
  computed from (spec §7's reporting requirement: convention + sample size + window, and §5 here). Extrapolating
  from 0.0% forward would be exactly the error the convention exists to prevent.
- **It does not condemn the feature either.** A corpus of *measured turns* is not a corpus of client behaviour:
  the capture corpora exercise one script's context growth, and a real client going through a gateway is
  expected to re-send things a replay set would never contain (a retry after a client-side timeout, an editor
  re-running an unchanged command, two tool calls with identical arguments). **The number that would justify or
  refuse the feature does not exist in this tree**, and it cannot be manufactured inside the loop: it needs real
  traffic through a cache-enabled root, which is a *measurement* on live traffic that a later round may propose
  and the owner may authorise. Registering that honestly is part of this ADR's job (`R51-0-F6`, §14) — the
  alternative, asserting a plausible repeat rate, is the class of claim constraint 4 forbids.

---

## 8. The observation boundary (constraint 3)

- **What the cache reads**: the request's own bytes (to digest them), the request's own path, session and mode
  word, and the revision's `config_digest` — all values that already exist on the request path, read where the
  pipeline already reads them. **Nothing else**: no file, no `autowork/` path, no store (`Query`), no config
  re-read, no clock.
- **What the cache writes**: entries in its own in-process memory (§3.4) and — through the existing writer,
  never its own — the record of the request it served, whose only new content is the `cache` group (§10.1). It
  writes **no** event-log row of its own: the hit's request writes its own `request.received` anchor exactly as
  any request does, and a hit's session already has its binding (the identical bytes were sent in this session
  before), so no binding write is due. Nothing durable is written by the cache; **the trace JSONL stays the only
  product → autowork channel** (ADR-005), and a hit is a record *in* it, not a second path *out* of the process.
- **The reverse direction is equally closed**: nothing under `autowork/` is read by this capability; a grep of
  the module for `autowork` must stay **0**. The DESIGN landing states the rule where the code lives (§12.22),
  so the next reader finds it there rather than here.
- **What a consuming round may do with it**: a hit's record is evidence *of the product's own behaviour* like any
  other record — it may be counted, but only ever as an `inferred` figure, and never as a gate input (§5). It is
  not a new observation medium, and no measurement definition may cite the store itself (which is not durable and
  therefore not observable at all after a restart).

---

## 9. The single-owner rule for the key and the store

Mirroring ADR-041 §4's shape — one derivation, no parallel second one — enforced **structurally**, in signatures,
not by review.

### 9.1 The layer table

| layer | the owner | the shape |
|---|---|---|
| the key | `router_core::response_cache::ResponseKey::for_request(&RequestFacts) -> ResponseKey` — **one derivation**, in `router-core`, pure, no clock | the five components of §3.1, built from values the pipeline already holds; the digest is the only one computed here |
| the store | `router_core::response_cache::ResponseStore` (in-memory, FIFO, two frozen bounds) — **one implementation** | `lookup(&ResponseKey) -> Option<&RecordedResponse>` and `record(&ResponseKey, RecordedResponse)`; `RecordedResponse` carries the bytes, the status and the **source reference** (§4.2) |
| the mount | the plugin `builtin/response_cache` in `router-plugins`, registered in the assembly's registry beside `builtin/transform_rules` (`assembly.rs:155`) | an entry with `config.enabled: false` mounts **inert** (§6.1); the fiber owns the store instance, so the mechanism's own unload semantics are the store's lifetime (§2.2) |
| the seam | **one call site** in `crates/router-proxy/src/forward.rs`, the last step before the attempt (§4.4) | `match cache.lookup(&key) { Some(rec) => serve(rec), None => attempt(...) }` — the buffered path's only new branch; the streaming path's twin is the same owner, called at the same position |
| the record | `Accountant` — **the existing single writer** (DESIGN §12.6) | the hit is one more class through `commit`, with `usage_missing: true` and the `cache` group; no second writer, no second record shape |
| the label | `inferred` — one word, one meaning, `forward.rs:457`'s own (`verified`/`inferred`, spec §7) | `cache.verdict`, a `&'static str` |

**The required extraction list, closed** (the whole of the refactor the implementation card owes):

1. The key derivation and the store live in **one new module**, `router-core/src/response_cache.rs`, and the
   request path holds **one** handle to them. Nothing in `router-core::body`, `prefix`, `cost`, `plan`,
   `trace` or `store` moves.
2. The store is reached through the **plugin's assembled handle** at exactly **one call site** in
   `forward.rs` — the same discipline `compose_transform_stage` already follows for the transform chain; if the
   streaming path needs the same branch, it calls the same owner rather than growing a second copy (the L2a/L2b
   leak class is not to be extended by this round).
3. That is all. No existing function changes its signature; no existing record field changes meaning.

### 9.2 What the one-owner rule forbids here

- A **second key derivation** anywhere — in the plugin, in `router-cli`, in a test helper that a case then
  asserts against. The case tests the owner.
- A **second store** — no per-protocol map, no per-session side map, no cache-of-the-cache, no persistence
  (§2.3(e)).
- A **second writer** of the trace, and any **derived figure** kept in the store (§3.4): the store holds bytes
  and the source reference, nothing a report could sum.
- A **second label word**: `verified` is never written by this path.

---

## 10. The trace and config contract landed here

### 10.1 `spec §6` — the group a hit adds (additive; `schema_version` stays **2**)

```jsonc
"cache": {                       // present IFF this record's response came from the store — never null, never empty
  "verdict": "inferred",         // always (constraint 4); the same word forward.rs:457 writes
  "key_digest": "<64 hex>",      // sha256 of the client's inbound body bytes AS RECEIVED (§3.1) — the one key
                                 //  component the record does not already carry
  "replayed": { "request_id": "<the source record's id>", "session": "<same as this record's>", "turn_index": 3 },
  "replayed_bytes": 20481        // how many response bytes were returned from the store — a count of bytes
                                 //  returned, not a measurement of an upstream, not a price and not a saving
}
```

Rules that belong to the contract rather than to the example:

- **Present iff a hit** (a map that may not carry a `null`, a `{}`, or a `verdict: "verified"`).
- **`schema_version` stays 2.** It is an additive optional group in the class DESIGN §12.6 defines: *"an added
  key never moves the version"* (`:872-873`), and a consumer that tolerates unknown fields reads an older record
  unchanged. A record **without** `cache` is not a hit and must never be read as one.
- **The record's other fields on a hit**, fixed here so no implementer guesses: `usage_missing: true` (§4.1);
  `result.status` = the **recorded response's own HTTP status**; `result.upstream_status: null` and
  `result.upstream_ms: null` (absent, never `0`); `result.overhead_ms`/`decision.decision_ms` as the pipeline
  computed them (§4.4); `protocol.protocol_out: null` (no bytes left the process — a third class beside spec
  §6's two, and §11.4 states the widening); `protocol.translated: false`, `lossy: []`; `errors: []`;
  `decision.provider`/`model` = the resolved route **this request did not call**; `cost` and `usage` exactly the
  shape the existing no-upstream-call class carries (priced nowhere, summed nowhere).
- **Every key component is on the record** (§3.1): `cache.key_digest` + `protocol.protocol_in` +
  `config_digest` + `identity.session` + `transform_mode`.

### 10.2 What the spec says beside it

`docs/spec.md` §6's field-group table gains the group with its vocabulary, §6's `protocol_out` sentence gains the
hit as its third class, and §4.17 is the new config subsection. §11 carries the exact edits this card lands.

### 10.3 The case ids, claimed from a measurement

Measured at this branch's base `bf96207`: `ls tests/conformance/tests/*.rs | wc -l` → **77** files, ids
`01–47, 53–66, 71–78, 80–87`; DESIGN §12.8's own occupancy paragraph (R50-0's, `:1501-1510`) closes with *"the
next free ID is **`CONF-88`**"*, and the register's spent set is `01–47, 52–70, 71–78, 79, 80–87` with `48–51`
reserved. This round therefore takes the **lowest free id above both** the tree's maximum (87) and the
register's own claim (88): **`CONF-88`** and **`CONF-89`**, on
`tests/conformance/tests/conf_88_response_cache_off_by_default.rs` and
`tests/conformance/tests/conf_89_response_cache_hit_is_the_recorded_bytes.rs`. Both land **with the code they
witness** (the R50-1 shape: the case and the change in one commit), the register's heading moves to
`` `CONF-01…CONF-89` ``, a new occupancy paragraph records the spend and names the next free id, and the ids are
spent — not renumbered, not reused.

---

## 11. The documents this contract lands in (spec §1, §4.17, §6)

### 11.1 `docs/spec.md` §4.17 — the config contract (one new key, one default)

A new subsection after §4.16, carrying: the kind (`builtin/response_cache`), the `plugins:` entry's shape, the
single key `config.enabled` and its default **`false`**, the two frozen constants (§3.4) **stated in-band as
constants, not keys**, the key's five components (§3.1), what a hit proves (§3.2's sentence verbatim), what is
never stored (§3.3), and the pointer to §6 for the record's group and to §7 for the label.

### 11.2 `docs/spec.md` §6 — the record's group and the `protocol_out` widening

§6's group table gains the `cache` row and its vocabulary (§10.1) and one paragraph stating the three properties
a reader must not have to derive: present iff a hit; `usage_missing`-class, so summed and priced nowhere; the
reference is inside the session and is the source record's id.

**The `protocol_out` sentence is widened additively** (spec §6's own words today: *"it is `null` only in the
classes where no route was selected"*, `docs/spec.md:1773`): a hit is a **third** class — a route *was* selected
and no bytes left the process — so `null` is a hit's value and the sentence names the class. The field's meaning
does not move: the two existing classes and every record written today read exactly as they did.

### 11.3 `docs/spec.md` §1 — the non-goal row, amended **additively**

The row at `:19` is **replaced** by one that states both halves (it is not deleted, and the *semantic* half is
not weakened):

| Not doing | Reason / later path |
|---|---|
| **Semantic** response cache, context summarization | **Still out**: content-similarity matching and summarization — a large conflict surface with prefix caching, and a measured ledger is needed first (P4). **Now in scope, and off by default**: the **exact-match** response cache (§4.17) — a byte-identical repeat of one session's own request may be served from a response recorded earlier; the saving is `inferred` for ever and **no gate may count it** (AGENTS 4, §7; ADR-042) |

Two facts are part of this edit. First, it is an **amendment for silence, not for error**: the old row was true
of the semantic cache and stayed true (`CLAIM-SOURCES.md:360`), but §1's table must name a capability the config
can turn on, or the contract has a hole of the *documented-and-unreachable* class's mirror image — an
implemented-and-undocumented one. Second, it **drifts R49's claim row W2/C19/C25** (`CLAIM-SOURCES.md:360`,
R49's file, outside this card's write set), which is registered rather than hidden (`R51-0-F1`, §14).

### 11.4 Line-number deltas, and who re-derives them

This card adds §4.17 to a 2439-line spec and two rows + paragraphs to DESIGN §12.8, and inserts §12.22 before
§13: §12.8's rows move nothing above them, `design/DESIGN.md` §13.1 shifts by the length of §12.22, and
`docs/spec.md` §6/§7/§9.2 shift by the length of §4.17. **Those deltas carry every number in this ADR with them,
and the implementation card shifts three code files as well** (`crates/router-proxy/src/forward.rs` +11 at the
cited line, `crates/router-plugins/src/assembly.rs` +40/+44, `crates/router-core/src/trace.rs` +8/+47, and
`book/cost-and-caching.md` +3). The sentence here used to promise the re-derivation to "the round's
implementation card", and that promise was owed rather than kept: R51-1 touched no document, so **R51-3, the
round's landing card, performed the mechanical act** — it re-derived every `path:line` in this ADR at this
branch's HEAD, hunk by hunk, and verified each against the content it claims
(`autowork/harness/r51-3/REANCHOR.md` carries the table, the before/after and the after-check). Every
`path:line` in **this** ADR therefore resolves at this branch's HEAD (the convention in the header) **by
re-derivation rather than trust**, with the two replacement quotes §0 keeps labelled there as the base's.

---

## 12. The invariants, each with its case

### 12.1 The pair's shape

`CONF-88` pins the **default** (an absent capability is off, and a later config edit cannot quietly change that);
`CONF-89` pins the **feature** (a hit returns the recorded bytes and says so in the record). The pair is what
gives the round its red/green contrast: **`CONF-88` is green at the base by construction and must stay green**,
**`CONF-89` is red at the base** (no store exists, so no hit can happen) and green once the code lands.

### 12.2 `CONF-88` — the capability is off by default

| # | limb |
|---|---|
| 1 | **No entry ⇒ off.** With the exemplar root (no `builtin/response_cache` entry), a byte-identical repeat inside one session produces **two upstream calls** and two records, **neither** carrying `cache`. |
| 2 | **An entry with the key absent or `false` ⇒ off.** The same traffic through a root that *does* list the kind with `config: { enabled: false }` (and once more with `config: {}`) behaves **byte-identically** to limb 1: same call count, same outbound bytes, no `cache` anywhere. |
| 3 | **The control that makes limb 2 meaningful**: the same rig with a build whose loader defaults that key to `true` must turn limb 2 **red**. (A sabotage run, recorded as a rig, not as a case — §6.2.) |
| 4 | **The default path is byte-identical to a cache-less build**: the upstream-visible bodies in limb 1 equal the same build's bodies with the kind deleted from the binary's registry — the assertions of `CONF-01`/`CONF-02` are the independent witness for the mutations, and this limb adds the "the cache changed nothing on the default path" half. |
| 5 | **Green at the base**, and green for every root the rest of the suite uses: the check is that no existing case file, fixture or harness root had to change to accommodate it. |

### 12.3 `CONF-89` — a hit is the recorded bytes

| # | limb | how it can fail |
|---|---|---|
| 1 | **Byte equality.** After one miss, a byte-identical repeat inside the same session returns a response **byte-identical** to the recorded one (headers included, per the wire), and the upstream sees **no** second request. | a re-encode, a re-serialise, a header rewrite, or a second call |
| 2 | **The record says so.** The hit's record carries `cache` with `verdict: "inferred"`, `key_digest` equal to the sha256 of the bytes **as sent**, `replayed.request_id` equal to the **miss's** `request_id`, `replayed.session` equal to its own, and `replayed_bytes` equal to the returned length. | a missing group, a `verified` verdict, a wrong reference, a copied usage |
| 3 | **The record refuses to be counted.** `usage_missing: true` on the hit; `usage` and `cost` in the no-upstream-call shape; `result.upstream_status`/`upstream_ms` `null`; `protocol.protocol_out` `null`; `cache` **absent** on the miss and on the ordinary requests of the same run. | a `0` where an absence belongs; a hit that looks like a call |
| 4 | **Ledger reconciliation** (§4.3): the window's `Σ usage` with the hit present equals the same sum with the repeat removed; the reference resolves to a record of the same session; hits are not counted as calls. | a double-count, a cross-session reference |
| 5 | **The key separates what it must**: changing only the session (same bytes) is a **miss**; changing only the mode word is a **miss**; changing one body byte is a **miss**; a different `config_digest` is a **miss**; identical everything is the only **hit**. | a key that drops a component |
| 6 | **Fail-closed**: a non-2xx response is not stored (the repeat is a second call); an incomplete body is not stored; a request with no session is neither looked up nor stored; no partial or oversized body is ever returned. | a replayed error, a truncated stream, a `session: null` bucket |
| 7 | **The store is not a second ledger**: it holds bytes and the source reference only — asserted by the fact that no figure in the window changes when the store's contents are dropped, apart from the call counts the dropped entries cause. | a derived figure kept in the store |
| 8 | **The bounds hold** (§3.4): past `MAX_ENTRIES` or `MAX_STORED_BYTES`, eviction is FIFO by insertion and the counters are the constant, not the traffic. | unbounded growth |
| 9 | **Nothing under `autowork/` is read**, and the hit's record is still the only product → autowork channel: the module's own grep for `autowork` is **0**. | a second observation path |
| 10 | **The gate non-interference check** (a rig, not a limb of the case): the four gates + `CONF-43` + the L1 envelope run against a root that does not mount the capability, and the ledger is the base's own identity. | a gate that begins reading a cache field |

### 12.4 What the pair conserves from the exclusion it replaces

The old row's worry was *"the saving is `inferred` for ever, and the 1 : 1 correspondence goes"*. Limb for
limb: the label is `inferred` on the record and can never be anything else (limb 2, §5 — the old row's first
worry, now a case); the correspondence is **restored in the form the ledger actually needs** — one record per
client request, no usage, no money, one reference, a sum that cannot double-count (limb 3/4, §4 — the old row's
second worry, now a reconciliation check); and the default path is untouched (the whole of `CONF-88`). What
changed is the *verdict* on the capability: it may be built, and the four clauses of §1.1 are the conditions of
building it.

### 12.5 The controls the round's verification cards must run

Minimum set, each recorded under `autowork/harness/r51-*/` (the R50-1 precedent's shape):

1. **A red control proving the feature is really off by default** — a build whose loader defaults `config.enabled`
   to `true`, with `CONF-88` red on exactly that limb and the tree restored by hash afterwards.
2. **A byte-equality control proving a hit returns the recorded bytes verbatim** — the recorded response and the
   replayed response compared byte for byte, with the upstream's request count at **1**, and a positive control
   that the comparison *can* fail (one mutated byte in the store ⇒ red).
3. **A ledger reconciliation control** — the window's sums with and without the repeat, plus the reference
   resolution, on a fixture where the source record is present and one where a cross-session reference is
   synthetically injected (which must fail the check).
4. **A gate-non-interference check** — the four gates + `CONF-43` + the corpus verification on a root with no
   cache entry, and a second run with the capability mounted **on** whose gate verdicts are compared for
   equality of the *counted* figures (the gate reads no cache field).

---

## 13. Alternatives considered, trade-offs, reversibility

| Decision | Alternatives | Gain / sacrifice | Reversible? |
|---|---|---|---|
| **Exact match by digest of the client's bytes** (§3.1) | a semantic/similarity cache; a normalized-content key; a key over the *outbound* bytes | Gain: no tokenizer, no parse, nothing to normalise, and the key is exactly the bytes constraint 1 already protects. Sacrifice: fewer hits (a whitespace change is a miss) — which is the honest direction. | Yes, but a content-equivalence cache is a new ADR (and the `AGENTS`-level question §1.3 names) |
| **Session-scoped key** (§3.1) | content-only key | Gain: a response recorded in one session is never served to another; the record's `session` attribution is true by construction. Sacrifice: the cross-session class is not served — measured at **0** items on the frozen corpora (§7.2), so today the sacrifice costs nothing. | Yes (one component) — and it would need to justify itself against the session attribution |
| **No time-based lifetime** (§2.2) | a TTL; a time bound as a declared deviation | Gain: constraint 2 as written holds, so the feature's own behaviour is replayable and the upstream-prefix promise is about bytes again. Sacrifice: a long-lived process may hold a stale response for ever (which is what a hit *is*, §3.2). | **Yes in code, no as a loop decision** — §2.4 routes it to the owner |
| **Discovery scoped to the medium the request used, with "complete 2xx only"** (§3.3) | buffered-only (simplest); no caching of streaming responses | Gain: the capability is not dead on arrival for the 28 of 34 corpus items that stream, and one rule ("what was recorded, verbatim") covers both media; the medium is already in the key because `stream` is a client byte. Sacrifice: one more call site (the streaming path's twin) and the incomplete-response exclusion to implement. | Yes (narrowing to buffered-only is a one-limb change) |
| **In-process store, FIFO bound** (§3.4) | SQLite/file persistence; an LRU bound; no bound | Gain: no second durable artefact, no migration, no state-dir writer, and a determinism story with no clock and no recursion. Sacrifice: a restart is a cold store; a busy session can evict a quiet one's entries (a miss, never a wrong answer). | Yes (persistence is `R51-0-F4`; an LRU policy is a widening) |
| **The lookup at the last step before the attempt** (§4.4) | before resolution (cheaper); before the guard chain (a policy bypass) | Gain: the record is the ordinary record plus one group, refusals are never softened, and the latency gate's quantity does not move. Sacrifice: a hit pays the decision and transform compute it does not need. | Yes (a cheaper path is a registered widening — and it would change the record's fields) |
| **The `cache` group, present iff a hit** (§10.1) | a new `status` word; a boolean beside `usage_missing`; a `transforms[]`-style entry | Gain: one question, one answer; no new vocabulary; additivity keeps `schema_version` at 2. Sacrifice: a reader must know the rule (stated in spec §6). | Yes, but every alternative spends a word the vocabulary already uses for something else |
| **The label `inferred`, spelled with `forward.rs:457`'s own word** (§5) | a new word (`replayed`, `cached`, `unverified`) | Gain: constraint 4's convention has one vocabulary and one definition. Sacrifice: the word does not by itself say *why* — the record's `replayed` reference does. | Yes (a word is cheap; a second convention is not) |
| **`config.enabled`, default `false`, beside the mechanism's `disabled`** (§6.1) | `disabled` alone (so listing = enabling); a top-level key outside `plugins:` | Gain: the default is a property of the loader, and a config edit cannot silently enable the capability; the mount stays in the mechanism ADR-036 requires. Sacrifice: one key to explain (the ADR explains it). | Yes |
| **The spec row amended additively** (§11.3) | leave §1 silent; delete the row; keep the row and document the capability only in §4.17 | Gain: §1's table stays the complete list of what is in and out of scope, and the semantic exclusion stays written. Sacrifice: a claim row drifts (registered, `R51-0-F1`). | Yes |

---

## 14. Consequences and the register

**Consequences that follow from this contract** (things a reader should not have to discover):

1. A hit's record is a **`usage_missing`**-class record, so `router stats` counts it on its own line and prices
   it nowhere — no new reporting line is spent this round (`R51-0-F2` registers the widening).
2. The **README** gains no claim this round, and gains nothing to *retract* either: R49's own watch item
   (`CLAIM-SOURCES.md:360`) asked that the wording stay true **of the default path**, which it does — the
   capability is off unless asked for, and no README sentence implies otherwise. The README is outside this
   card's write set, and the round's writer card (if the orchestrator cuts one) has exactly one possible edit:
   naming the capability in the Status/feature surface *as off by default*, with the `inferred` label attached.
3. **The default path is provably unchanged** — `CONF-88` limb 4 — which is what makes it legitimate to land an
   off-by-default capability while the four blocking gates keep their meaning.
4. **Two case ids are spent** and §12.8's heading and occupancy paragraph move with them (§10.3).
5. The `cache` group is the **fifth** optional group a reader of the trace must tolerate, in the additive class
   DESIGN §12.6 defines — and the first whose presence means *a call did not happen*.

**The register this card opens** (findings and notes, none blocking; owners named):

| id | item | owner | due |
|---|---|---|---|
| `R51-0-F1` | **the spec §1 row's drift**: amending the row to name the in-scope exact-match cache drifts R49's claim rows W2/C19/C25 (`autowork/harness/r49-0/CLAIM-SOURCES.md:360`, `:204`-class), whose file is outside this card's write set and whose W2 clause reads *"do not widen or delete it"*. The amendment is made **because the R51 card's relay instructs it** and because a silent §1 is the same defect class in the other direction; the row's semantic half is untouched, so no sentence becomes false. | the claim table's next owner (a `book/`/contract-touching card) | with the next claim-table amendment |
| `R51-0-F2` | a `router stats` line for replayed responses (§9.2's own surface; today they fold into `usage missing`) | the next `docs/spec.md` §9.2-touching card | open, with a trigger: the first round that reports the cache's own figures |
| `R51-0-F3` | the store's bounds are frozen constants (§3.4); a configurable capacity is the widening, with its own key, default and case | the next round that owns `config.example.yaml` | open, with a trigger: a deployment where 64 MiB is the wrong bound |
| `R51-0-F4` | the store is in-process only (§2.3(e)); persistence across restarts is the widening, and it would need a store-contract owner (ADR-009/010's writer discipline) | backend-coder + the store contract's owner | open, with a trigger: a measured repeat rate that survives a restart argument |
| `R51-0-F5` | **the upstream-prefix consequence of a hit** (§2.5): a hit does not warm the *upstream's* prefix cache, so a later turn in the same session may pay for it in `cached_tokens`. Reportable as a `cached_tokens` comparison; **not** measurable on the frozen corpus, and **not** a determinism violation. | the round that first measures the cache on live traffic | open |
| `R51-0-F6` | **the number that would justify the feature does not exist in this tree.** §7 measures the class on the frozen corpora (0 of 34, 0.0%) and states why (cumulative resends; the same script in another session differs by 22 bytes inside the session-id spans). A live repeat rate needs real traffic through a cache-enabled root — a paid measurement, hence the owner's. This row exists so the next round does not re-derive the null and mistake it for an argument *against* the feature. | the orchestrator (a live-measurement direction) + the owner (its authorisation) | open |
| `R51-0-F7` | **the design's own residue, named**: the capability is landed **off** and the class it serves is empty on every corpus this repository has, so this round's honest summary is *"a capability with a contract and no measured payoff yet"*. A round that lands it must not present it as a cost lever; §7.4's rule binds every later sentence about it. | every later round that mentions the cache | standing |
| `R51-0-F8` | **a contradiction inside the chapter this round edits**: `book/cost-and-caching.md:3-6`'s status sentence (*"cache fidelity and plan-first routing are served; payload compression is not"*) reads false against the chapter's **own** §"Payload compression: an opt-in mode, off by default" (`:286-329`, which documents the mode as landable today) and against DESIGN §12.12's status (`tier 1 wired`) — measured by reading both sites at this HEAD; **not** edited here (outside this round's subject, and it is another feature's truth) | `book/`'s next owner | open, standing |
| `R51-0-N1` | constraint 2's own words are what force §2.2's shape (`AGENTS.md`: *"never of turn number, wall clock, or RNG"*); a future "add a TTL for hygiene" idea is a **new contract with the owner's signature**, not a convenience edit | — (note) | n/a |
| `R51-0-N2` | cost: this card is docs-only and offline — **`$0.00`**, no provider dialled, no credential read; the measurement in §7 reads committed corpus bytes | — (note) | n/a |

**What is left undecided, and by whom**: the **time-based lifetime** (§2.4) is the owner's, with its trigger;
the four widenings above are rounds' to pick up, each with a trigger; and everything §1.3 lists as *not
licensed* is out of this card's reach by construction.

---

## 15. The `book/` paragraph

One user-facing paragraph, landed this round in `book/cost-and-caching.md` (beside *"Payload compression: an
opt-in mode, off by default"*, the shape it mirrors). It is written for an operator, links to the contract, and
copies no price number (constraint 8):

> **Response caching: exact match only, off unless you turn it on.** The router can keep the response to a
> request and hand the *same* request the *same bytes* again — nothing more. "The same" means the client's
> request bytes are identical, in the same session, on the same protocol, under the same configuration and with
> the same transform mode: change one byte, or ask from another session, and it is an ordinary request. It is
> **off by default** — a `builtin/response_cache` entry with `config.enabled: false` (or no entry at all) does
> nothing, and listing the plugin without flipping that switch still does nothing; turn it on with
> `enabled: true` and the router serves a repeat from what it recorded rather than calling the provider again.
> **What it will and will not claim**: a served repeat is a *replay*, not a prediction — the bytes are the ones
> the provider returned earlier, so the router makes no claim about what it would answer now — and because no
> call happens, nothing is measured: the saving is labelled `inferred` (it is never a measured difference), it
> is excluded from every rate and every sum, and **no gate may count it** (ADR-042; spec §4.17, §6, §7). On the
> corpora this repository measures against, that class is empty today, so treat it as a fidelity-preserving
> convenience, not as a cost lever.

---

## 16. What this ADR does not do — and where the card's ten items are settled

It writes no code: no key, no store, no plugin, no seam, no case file, no config, no `README.md` change. The
implementation is the round's implementation card, its independent verification a verification card, and the
`book/` paragraph a writer's edit; the orchestrator cuts all of them from `autowork/harness/r51-0/PLAN.md`.

| the card's item | settled in |
|---|---|
| 1. the authority chain | §1 (§1.1 verbatim + dated, §1.2 the supersession table, §1.3 what it does not license) |
| 2. the determinism collision | §2 (§2.1 in constraint 2's words, §2.2 the frozen answer, §2.3 the alternatives with costs, **§2.4 the owner decision**, §2.5 the prefix statement) |
| 3. the byte boundary | §3 (§3.1 the key and the no-normalisation rule, §3.2 the honesty sentence, §3.3 the fail-closed list) |
| 4. what replaces 1 : 1 | §4 (§4.1 the record, §4.2 the reference, §4.3 the reconciliation property, §4.4 the seam) |
| 5. the label | §5 (`inferred` always, gate-invisible structurally, `forward.rs:457`'s own word) |
| 6. default off + the case | §6 and §12.2 (`CONF-88`, and its red control) |
| 7. the measured payoff | §7 (0 of 34, 0.0%, the method, why, and what it does not license) + `autowork/harness/r51-0/{repeat-rate.txt,repeat-rate.json,measure-repeat-rate.py,corpus-verify.txt}` |
| 8. the observation boundary | §8 |
| 9. the reopen/close condition | §2.4's trigger column, §2.3's widening rows, and the register's six triggers (§14) |
| 10. the `book/` paragraph | §15 (landed in `book/cost-and-caching.md`) |

**What would falsify this decision later** (the reopen condition, stated so the decision is not open-ended): a
**measured live repeat rate** materially above zero (§7.4 — the number that would justify it, and the only one
that could); a ruling that the lifetime may be time-based (§2.4 — which would change §3.4 and nothing else); or
a serving incident in which a replayed response was demonstrably wrong for the request that received it — which
would be a defect in the key's components (§3.1) and would narrow §3.3 rather than licence a new claim.

---

## 17. Owner rulings (2026-09-28)

Two decisions this ADR **surfaced instead of taking** — the lifetime question of §2.4 and the paid
measurement of §7/§14 (`R51-0-N1`, `R51-0-F6`) — were put to the owner on **2026-09-28**, each with the
orchestrator's recommendation beside it. **The owner ruled on both, the same day.** This section records
those rulings **as the owner's decisions**; it rewrites nothing above it. §2.2's frozen shape **stands as
written**, §2.4's route (a future time bound as a declared deviation) is **not** exercised, and the
paragraphs that raised the two questions stay where they are — they are the record of *why* each was the
owner's to answer and not the loop's.

| id | the question, and where this ADR put it | the owner's ruling (2026-09-28) | what it closes | what it leaves open |
|---|---|---|---|---|
| `R51-0-N1` | may the store's lifetime be **time-based**? — §2.4's row, with its trigger | **No: the lifetime stays clock-free.** No TTL, no `session.ttl` reuse, no timestamp comparison on the lookup path | §2.4's question, and the loop's standing licence to treat **"add a TTL for hygiene"** as a convenience edit; §2.2's shape stands | §2.4's trigger, unchanged: the owner asks for the store to age out, or a serving incident shows unbounded staleness mattering in practice |
| `R51-0-F6` | the **paid live measurement** that would give the capability a real-traffic repeat rate — §7's number, §14's row | **Not funded.** The four frozen corpora's measured **`0.0%`** stands as the only measured rate for this capability | the search for a measured repeat rate inside this repository: nothing is queued behind the corpora's null | the capability's payoff **on real traffic** — stated as **unknown**, explicitly not estimated. A later funding decision, a real deployment's own traffic, or an owner who wants the instrument built would reopen it |

**Ruling 1 — the store's lifetime stays clock-free (`R51-0-N1`).** On 2026-09-28 the owner decided that
the response-cache store's lifetime **stays clock-free**: **no TTL, no `session.ttl` reuse, and no
timestamp comparison on the lookup path.** The implementation already reads no clock — R51's own sweep of
the serving path (re-attacked, and re-derived, by R51-2's verification, attack 7) found **zero** time-API
references in the two cache modules; `autowork/STATE.md` row 22 records that reading — so what the ruling
changes is not the code path but the **authority**. The clock-free contract is now the **owner's**
decision and no longer only this ADR's: a card that proposes a TTL, a `session.ttl` reuse, a "stale after
N seconds" rule or any other expiry by elapsed time is proposing a **contract change that needs new
authority**, and it may not be read — or argued — as an implementer tidying up an oversight. The converse
is the reason the ruling is worth writing down at all: **the absence of a TTL is a decided absence, not
an unexamined default.**

**Ruling 2 — the paid live measurement is not funded (`R51-0-F6`).** On 2026-09-28 the owner decided
**not to fund** the paid live run that would give this capability a real-traffic repeat rate. **What it
closes** is the search for a measured repeat rate inside this tree: the frozen corpora's **`0.0%`**
within-session byte-identical repeat rate (0 of 34 items, 8 sessions; §7 measures it, §7.3 states the
mechanism) **stands as the only measured rate this capability has**, and no further measurement is queued
behind it. **What it leaves open** is everything that number would have decided — and the honest form of
"open" here is **unknown**, not estimated: **the capability's payoff on real traffic is unknown**, and it
stays unknown until an owner funds a measurement or real traffic reports a rate of its own.

The consequences are the ruling's substance, and they bind every later sentence about this capability:

- **No saving may be claimed anywhere on the strength of this ruling or of this capability.** Not in
  `book/`, not in `README.md`, not in the trace vocabulary, not in a round record, not in a config
  comment: the measured `0.0%` licenses **no** saving figure, and §5's always-`inferred` label was never
  a licence to publish one. §7.4's rule and register row `R51-0-F7` already said so; the ruling makes the
  gap between *unmeasured* and *claimed* a **decision** rather than a waiting item.
- **The ruling is fiscal, not technical.** It is a decision about **spending money on a measurement**,
  taken against a corpus that holds no repeat — it is **not** evidence that the response cache never
  pays. The honest reading of §7 is that this repository has never had the traffic that could show it
  either way, and the mechanism the capability would exploit (a client resending a byte-identical
  request in the same session) is a property of the clients, not of this corpus. A later owner may fund
  the measurement; a real deployment may produce the rate for free. **Neither would contradict this
  ruling**, and neither is required to justify the capability's existence — which was authorised on
  constraint grounds (§1.1), not on a saving.
- **What remains true and unchanged:** `CONF-88`/`CONF-89` (off by default; a hit is the recorded
  bytes), the byte boundary (§3), the label (§5), the observation boundary (§8), and §7.2's headline —
  *a documented null*. The capability ships as a fidelity-preserving convenience with a measured `0.0%`
  on every corpus this repository holds, and **nothing above this section is amended.**

**Where the two rulings are registered.** `autowork/STATE.md`'s waiting-on-human table carries them as
rows **22** and **23**; both rows were **decided** on 2026-09-28 by the commit that appends this section
(the rows name it), their original registration text left **standing** — a row in that table is the
record of a decision the loop could not take, and after the ruling the same row becomes the record that
the owner took it. **§14's register rows are not rewritten either** (this ADR is append-only): `R51-0-N1`
is a note whose content this ruling confirms, and `R51-0-F6`'s `open` column is **superseded by this
section** rather than by an edit to §14 — the same spirit as §2.4's unexercised route. A reader who
follows either row's citation to this ADR lands here.
