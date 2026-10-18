# ADR-057 — the step ledger: one line per request in a session, with the cache break named, the money labelled, and nothing invented

- Status: **accepted — implemented.** The owner's ruling of 2026-10-18 adopted the recommendation;
  the prerequisite repair (§2.6) and the surface (§2.1–§2.5) landed on `feat/step-ledger-impl`
  (the §2.6 repair as commit `9731e69`, the reducer with `CONF-111…CONF-117`). One amendment was
  folded into the landing (§2.3 row 3, noted in place): the `gap_ms` null condition is "identical
  `ts` at the record's resolution", not "the two `ts` fall in one second" — the pre-repair clock's
  one-second floor is what that older wording described, and with §2.6 landed it would contradict
  both §2.6 and limb (f).
  *Corrected in place (2026-10-18, before it was ever ruled):* `ADR-058` split the accounting word —
  `measured` for one observation, `verified` for a difference between two worlds — and this draft's counter
  columns were using `verified` for single observations. §1.2, §1.4, §2.2, §2.3, §2.4, §2.5 and §7 now say
  `measured`, and §2.5 states that **no `verified` figure exists on this surface**. No column, value,
  predicate or refusal moved; the correction is a word, and it was made in the body because nothing here had
  been frozen yet (the same reason §2.6's finding is registered rather than repaired).
- Kind: **a new read-out surface with no product byte.** It adds one subcommand that reads the trace
  files, one spec section, and one two-line repair to the clock the trace writes (§2.6) — that repair is
  the only thing here that touches `crates/`, and it is a *prerequisite*, not scope.
- Authority: the owner's ruling of 2026-10-18 (*"确认"* to the recommendation that vadis adopt DeepSeek
  Harness's Trajectory idea as a **turn/step ledger**, not a content ledger, and that the first cut be
  the offline reducer plus its attribution). The evidence is a read of the owner's own deployment
  (§1.3), taken with `vadis serve` running from the trace directory only.
- Amends: `docs/spec.md` (new §9.4; §9.3's not-served list gains one entry), `design/DESIGN.md` (§12.9,
  the round's register line), `book/observability-and-accounting.md` and `book/roadmap.md` (the
  not-served sentence each carries). `crates/vadis-proxy/src/{accounting,auth}.rs` only under §5's
  amendment list, and only when the implementation card lands.
- Related: `AGENTS.md` constraints **3** (the observation boundary — §2.7), **4** (no unverified savings
  — §2.5), **2** (determinism — the reducer takes no RNG, no clock), **8** (docs before code — §6),
  **9** (the measurement is not part of the search space — this draft touches no gate); **ADR-005** (the
  trace is the only channel), **ADR-010** (the event log is state truth — the one thing this surface
  must *not* read, §2.1), **ADR-042** (the response-cache group it prints as a flag), **ADR-056 §2.4**
  (the three-bucket predicate it reuses verbatim and does not redefine), **ADR-055** (the last surface
  added to §9), **GAP-Q14** (block tokens are inferred), **GAP-Q17** (a plan's allowance is not a token
  count — §2.5's `plan` label exists because of it).

---

## 1. Why

### 1.1 The question this answers, and the shape it borrows

DeepSeek Harness's **Trajectory** answers one question well: *what happened inside that run?* It does so
by keeping an append-only session log of everything the model saw — system prompt, reasoning, tool calls
and results, subagent scheduling, context injections, compaction — and rendering it as a ledger grouped
by turn and step, one row per event, each row openable for tokens, duration, input, output and timing.

Two of its properties are the ones worth stealing, and neither is the UI: **the ledger's unit is a step,
not a request**, and **every row is attributable** (a token count, a duration, a cause). The third — the
content — is the one vadis must refuse (§2.7).

vadis is a different instrument in one decisive way: **it sits on the wire, so it sees the bytes but not
the agent's intent, and it sees the agent's *account* but not its *reasoning*.** That asymmetry decides
the design. A vadis ledger cannot show you a tool call's payload; it can show you, per step, what the
prompt grew by, what the provider's own cache said, which account paid, what that cost, and — when the
prefix broke — **which block broke it**. Harness knows tokens and not prices. vadis knows prices and not
tokens-of-content. The ledger below is the second shape, and it is not a lesser one: it is the only shape
in which "which step cost me the money" is a question with an answer.

### 1.2 The relation that makes a step visible without reading content

Both shipped clients are stateless (AGENTS "Environment gotchas"): codex sends `store: false` and
resends the whole `input` every turn, hermes does the same. vadis therefore receives, on every request,
the conversation so far — and for every request it already records the prefix as a **list of structural
units**, each with a kind, an index, a token estimate and a hash
(`crates/vadis-core/src/trace.rs:143-161`; the extraction is `crates/vadis-core/src/prefix.rs:82`).

That list is the prompt's skeleton, and **the difference between two consecutive lists in one session is
the step**:

| Relation between step *n*'s block list and step *n−1*'s | What the wire says happened |
|---|---|
| the earlier list is a **prefix** of the later one (the later has more blocks) | the client appended: one more exchange, or more tool output at the tail |
| the later list is a **strict prefix** of the earlier one | the client dropped the tail: a compaction, a trim, a `--continue`-less rewind |
| the two lists share a prefix and then **differ**, and the earlier list's tail reappears later | the client changed something **early**: an edited turn, a different first message, a reordered tool set |
| the lists are **identical**, block for block | the same bytes were sent twice: a retry |

Every one of those four statements is a statement about **hash equality**, which is a `measured` fact about
the bytes vadis forwarded (spec §7, ADR-058). What it is *not* is a statement about why the client did it — §2.2 makes that
boundary explicit, because the temptation to print "the agent compacted" is exactly the place a gateway's
ledger starts lying about a client it cannot see.

**What this relation cannot produce, and the ledger therefore does not print:** tool names, tool
arguments, tool results, reasoning text, the user's message, or the model's answer. ADR-005 makes the
trace the only channel, and the trace carries hashes and counts, never content
(`crates/vadis-store/src/lib.rs`'s `body_hash`: *"never the body"*). Content capture is a different
product with a different contract (ADR-003's `tee`, out of scope for v0.1); §7 returns to it.

### 1.3 What the corpus says today, before any code

Read on 2026-10-18 with `vadis serve` live, from `~/.config/vadis/state/traces/*.jsonl` **only** (the
store is EXCLUSIVE-locked while serve runs; the trace is not). Window
**2026-10-04T05:17:54Z → 2026-10-18T03:36:47Z** (13.9 days), **181** hourly rollover files, **8,260**
decision records (every row whose `ts` is at or before the window's last row, so the snapshot is a
readable one: a live gateway keeps appending while the corpus is read). The count moves between reads because the corpus is being written while it is read —
the *shares* below are the claim, not the totals.

| Step class | Rows | Metered input-miss money | Notes |
|---|---|---|---|
| `append` | 7,290 | **$1.745** (`deepseek`) | 4,490 of the rows are on the metered route; 2,800 on the plan route (marginal 0) |
| `rewrite` | 308 | **$0.386** (`deepseek`, 200 rows) | **16.1 %** of that account's input-miss money from **4.1 %** of its rows |
| `shorten` | 354 | $0.0002 | 340 of them on the **plan** route: the compaction class is unmetered in this corpus |
| `first` | 196 | $0.259 | one per session key; 66 sessions are a single request |
| `repeat` | 112 | $0.002 | byte-identical resends — a client retry, invisible to `vadis stats` |

Two shapes in that table are the whole argument for the surface:

1. **A `rewrite` step re-prices ~4.97× the prefix tokens an `append` step does** (2,572,613 miss tokens
   over 200 rows ≈ 12.9 k per row, against 11,631,003 over 4,490 ≈ 2.6 k per row). It is 4.1 % of the
   metered rows and 16.1 % of the metered input-miss money. "Which step broke my prefix, and what did the
   break cost" is not a curiosity — on this corpus it is one sixth of the metered bill's cache line.
2. **The expensive-looking class is the unmetered one.** `shorten` carries 2.57 M miss tokens (**7.7 %** of
   the window's miss-token volume) and **$0.0002 of money**, because 340 of its 354 rows sit on a
   `coding_plan` route whose marginal price is 0 by §4.6 rule 4 — and plan traffic in total carries
   **52.4 %** of the window's miss-token volume at a zero marginal price. A ledger that printed `$0.0000`
   there would be telling its reader *"compaction is free"*. It is not: it consumes allowance headroom
   that vadis cannot price at all (GAP-Q17 — a CN coding plan publishes credits, not tokens). Hence
   §2.5's rule: **a row on a plan route prints `plan`, never a zero.**
3. **A mid-prefix break is a structural fact first and a money fact second.** **563** steps sit strictly
   mid-prefix (`0 < lcp < len(L(n−1))`), and the kept prefix they re-price at miss is **17,630,252** tokens —
   **52.7 %** of the window's miss-token volume — yet those rows carry only **$0.0648** of input-miss money
   (**2.7 %**), because most of them are plan rows. Both halves are the ledger's content: the block index
   that broke is worth knowing regardless of who paid, and a surface that printed only the money would
   report the break as nearly free.

And two facts about the corpus's own limits, stated because they bound every number above:

- **32 rows carry no session key** (`identity.session: null`) — a client that sent neither
  `prompt_cache_key` nor a session header. A per-session surface cannot show them (§2.7).
- **152 rows carry no usage** (`usage_missing: true`) — the ledger must print an absence, never a zero
  (§2.5), and it does not compute a ratio over them.

### 1.4 Why this is a contract rather than a script

The reducer is ~300 lines of pure function over JSONL, and the temptation is to ship it as a script in
someone's dotfiles. Three things make that wrong here:

- **The labels are the product.** `measured` / `inferred` / unresolvable, the currency rule, the `plan`
  label, the ratio's denominator — each is a §7 convention that already exists, and a second
  implementation of them is a second reading of the accounting contract. ADR-041 made the same argument
  for `/metrics` and `vadis stats` sharing one definition of a figure.
- **The step classes are a vocabulary.** A row word (`append`, `rewrite`, …) is read by humans and by a
  future `--json` consumer; two tools naming the same relation differently is the drift this repository
  hunts.
- **The surface must be absent from `/metrics` on purpose.** A per-session ledger is not a scrape target
  (§2.1), and saying so in the contract is what stops a later round from "reusing" the class words as
  Prometheus labels — a per-session label set on a shared process is the cardinality explosion that gets
  a metrics endpoint turned off.

---

## 2. The ruling

### 2.1 The surface: `vadis steps --session <key> --config <file>`

One subcommand, one input medium, one output.

| Property | Value |
|---|---|
| Command | `vadis steps --session <key> --config <file> [--last <n>] [--json]` |
| Reads | the trace JSONL under the config's `trace.dir` (§4.1) — **the trace only** |
| Does **not** read | the store (`state/vadis.db`), `/state/events`, `/health`, `/metrics`, the tee store, any provider |
| Requires `--config` | **yes**, exactly as `vadis stats` does (`crates/vadis-cli/src/stats.rs:415`): the money column needs the price table that priced the row, and the `plan` label needs the roster's `account:` (§2.5) |
| Requires `--session` | **yes**. An unkeyed request belongs to no session and the surface cannot invent one (§1.3) |
| `--last <n>` | print the last *n* steps; the header then says `showing the last n of N`. No default truncation |
| `--json` | the same fields, the same names, one array plus the summary object (§2.4) — a field never appears in one form and not the other (the §9.2 rule) |
| Exit code | `0` on a printed report; `2` on a load/parse refusal, an absent session, or a bad flag — the same convention `stats` already uses for a bad config (`stats.rs:418-427`) |
| Live gateway | no fallback path exists and none is needed: the trace is readable while `serve` runs, so unlike `stats`' one store-derived figure (§9.2) this surface has **no `(gateway)` mode** |

**Why not a `stats` sub-report.** `stats` answers a window-wide question with figures that must sum; the
step ledger answers a session-shaped question whose *order* is the information. Folding it in would force
`stats` to carry a second output shape whose rows do not aggregate — and it would make the one surface
that runs with the gateway live depend on a subcommand that a scripted report already parses.

**Why not `/metrics`.** Per-session and per-step labels on a process-wide scrape surface are unbounded
cardinality. The ledger is a read-out, not a series; §2.4's summary is print-only, and this draft adds no
metric (a future round that wants a *windowed* class histogram is a different surface with its own ADR).

**Why `steps` and not `turns`.** A **turn** is a user instruction through to the agent's stop, delimited
by the agent loop. vadis does not see the loop: it sees requests. It can *guess* a turn boundary from the
block relation (a `rewrite-early` where the changed block is the first user message), but a guess printed
as a fact is exactly the class of error §2.2 and §2.5 exist to prevent. The unit this surface can stand
behind is the **step** — one client request, one trace row — and the name says so.

### 2.2 The step relation, and the five words it gets

For step *n* > 1, let `L(n)` be the ordered list of block hashes `prefix.blocks[i].hash`
(`trace.rs:153-161`) and let `lcp` be the length of the longest common prefix of `L(n−1)` and `L(n)`.

| `class` | Predicate (pure, over hashes) | What it is **not** |
|---|---|---|
| `first` | *n* = 1 in this session | not "a new conversation" — a session key can be reused after days (§2.4) |
| `append` | `lcp == len(L(n−1))` and `len(L(n)) > lcp` | not "a new tool result" — an appended block can be a message the *client* appended |
| `repeat` | `L(n−1) == L(n)` | not "a provider retry" — the client resending the same bytes is indistinguishable from it, and that ambiguity is the row's honest content |
| `shorten` | `lcp == len(L(n))` and `len(L(n)) < len(L(n−1))` | not "a compaction" — a trim and a rewind look the same here |
| `rewrite` | `0 ≤ lcp < min(len(L(n−1)), len(L(n)))` — the empty common prefix (`lcp == 0`) is this row, and the row prints `kept 0` with `at #0:<kind>` | not "an edited message" — only "the first differing block is at index `lcp`" |

**Amendment 2026-10-18 (owner ruling; prose only — the implementation does not move).** As drafted, the
`rewrite` row read `0 < lcp < min(len(L(n−1)), len(L(n)))`, a predicate that left a divergence with an
**empty** common prefix in no row of the table at all. The implementation never had that hole: it
classified that case `rewrite` and printed the relation beside the word (`kept 0`, `at #0:<kind>`,
`crates/vadis-cli/src/steps.rs:328-336`), disclosed the classification in its own comment and pinned it
with the reducer's unit test — measured before the ruling at **103 of 8,388 non-first steps (1.2 %), in
28 of 198 sessions** over the 182-file corpus (`t_82f1b093`'s scan, and the L review's finding F-QA1;
re-measured for this amendment on a newer pinned window — 185 files, 94,565,313 bytes, 8,725 records,
201 sessions — at **106 of 8,490 non-first steps (1.25 %), in 28 of 201 sessions**, so the rate is stable
and the count grows with the corpus).
The owner ruled on **2026-10-18** that the row **covers** `lcp == 0`: the predicate above is widened and
the reasoning below is untouched, because none of it was wrong — what was wrong was a table that did not
describe the row the command prints. §2.3 needs nothing: column 10 prints the measured `lcp` (a `0`, not
an absent figure) and column 11's `at` is null only for `append`/`first`/`repeat`, so `#0:<kind>` was
already inside its contract. No code, no class, no case, no `CONF` id and no other class word moves
(AGENTS constraint 9: this ruling *is* the human decision that constraint asks for — a case pins behaviour,
and no behaviour changed).

**The rule that keeps these words honest: the class is a *name over a `measured` relation*, and the record
it is printed from carries the relation, not the name.** Hence §2.3's `kept`, `at` and `blocks` columns
sit *beside* `class`, and the header of the text output says so in one line. A reader who needs the fact
reads `lcp` and the index; a reader who needs the shape reads the word. Nothing in this surface claims
the client's intent, because intent is not on the wire.

**Two more boundaries the relation forces:**

- **A block index is vadis's own enumeration order, not the provider's template order.** codex serialises
  `input` before `tools`; the provider's template puts `tools` first (`prefix.rs:77`, `:323`). So "the
  block at index 4 changed" is true of the bytes vadis forwarded and is *not* a statement about the
  provider's prompt position — and that is precisely why the cache column is the provider's own
  `usage.input_cached` and not a block-derived ratio (§2.5, and the metric-versus-provider-counter pitfall
  this repository already paid for once).
- **The relation is computed across files, not within one.** The rollover is hourly
  (`state/traces/YYYY-MM-DDTHH.jsonl`); a session that crosses an hour boundary must still be diffed
  against its previous step. The reducer's unit is the **session**, and the file set is only where the
  rows live.

### 2.3 The field contract — one row per step

Column names are the `--json` keys verbatim (snake_case). The text output prints them in this order.

| # | Column | Type | Source in the record | Label | Printed when the source is absent |
|---|---|---|---|---|---|
| 1 | `step` | u32 | — (assigned by the reducer, 1-based within the session) | derived | never |
| 2 | `start` | RFC3339 | `ts` — the request's **start** (§2.6) | measured | never |
| 3 | `gap_ms` | i64 \| `—` | `ts(n) − ts(n−1) − result.overhead_ms(n−1)`; **negative values are printed as measured**, never clamped | derived | `—` when the two `ts` are **identical** (equal at the record's millisecond resolution — *amended at the landing*: the draft said "when the two `ts` share a second", which described the pre-§2.6 clock's floor and contradicted §2.6 and limb (f)), or when n = 1 |
| 4 | `route` | string | `decision.provider` + `/` + `decision.model` | measured | `—` when both are empty (a refusal before any route resolved) |
| 5 | `in` | u64 \| `—` | `usage.input_total` | **measured** (the provider's own count) | `—` when `usage_missing` |
| 6 | `cached` | u64 \| `—` | `usage.input_cached` | **measured** | same |
| 7 | `out` | u64 \| `—` | `usage.output` | **measured** | same |
| 8 | `money` | amount + `currency`, or `plan` | `cost.total`, `cost.currency`; `plan` when the `decision.provider` entry declares `account: coding_plan` | measured / config-derived label | `—` when `usage_missing` (the cost was never computed, §7) |
| 9 | `blocks` | signed count | new blocks: `len(L(n)) − lcp` (`+k`), dropped: `lcp − len(L(n))` (`−k`), `0` | **measured** (hash equality) | never |
| 10 | `kept` | u64 | `lcp` | **measured** | never |
| 11 | `at` | `#<index>:<kind>` | the first block where the lists diverge, its index and its `kind` (`system`/`message`/`tool`/`input_item`) | **measured** | `—` for `append`, `first`, `repeat` |
| 12 | `new_tok` | u64 | `Σ prefix.blocks[i≥lcp].tokens` | **inferred** (GAP-Q14: a proportional attribution of the measured total) | never (`0` on `repeat`) |
| 13 | `class` | enum(5) | §2.2 | derived name over a measured relation | never |
| 14 | `wall_ms` | u32 | `result.overhead_ms` | measured — **and it includes the upstream's generation time on a streamed row** (§2.7) | never |
| 15 | `head_ms` | u32 \| `—` | `result.upstream_ms` | measured (head arrival on the streaming path) | `—` when null |
| 16 | `status` | u16 (`→` u16) | `result.status`, plus `→` `result.upstream_status` when the two differ | measured | never |
| 17 | `flags` | space-joined vocabulary | presence checks, each on a named field (§2.3.1) | derived from measured facts | empty string |

**No row of this table carries `verified`.** A ledger row is one observation and the ledger holds no
counterfactual, so the label its counters take is §7's `measured` — a difference between two worlds is what
`verified` names, and no such difference exists on this surface (ADR-058). `inferred` stays where it was
(`new_tok`, the GAP-Q14 attribution), and `—` stays the mark of a figure the record cannot carry.

**§2.3.1 The flag vocabulary is closed.** Each flag is one predicate over one field, never an
interpretation:

| Flag | Predicate |
|---|---|
| `trunc` | `errors[]` holds an entry with `details.error_class == "stream_truncated"` (ADR-056 §2.3) |
| `no_usage` | `usage_missing == true` |
| `cachereplay` | the record carries the `cache` group (ADR-042 — the bytes came from the recorded store) |
| `xform` | `transforms[]` is non-empty |
| `failover=<route>` | `result.failover_from` is present; `<route>` is its value verbatim |
| `plan=<from>→<to>` | `result.plan_switch` is present (ADR-014); the two routes verbatim |
| `no_route` | `decision.provider` is empty — a refusal after the guard, before any attempt |
| `key=<i>` | `decision.key_index` is present (ADR-049 §3 — the index, never the value) |

`retry` is deliberately **not** a flag: it would duplicate `class == repeat`, and two names for one fact
is how a vocabulary drifts.

### 2.4 The summary block

Printed after the rows, and present as the `summary` object under `--json`. Every figure here is a
restatement of rows already printed — this surface computes nothing the rows do not carry.

| Line | Content | Rule |
|---|---|---|
| `session` | the key, verbatim | never truncated, never normalised |
| `span` | `first ts .. last ts` and the number of trace files read | this **is** the window: a per-session surface states its window by naming the session's own ends, and the file count is the reader's check that no hour was missed |
| `steps` | `N` | equals the number of rows printed (before `--last`) |
| `succeeded / failed / stream_incomplete` | three counts | **ADR-056 §2.4's predicate, reused verbatim** — this surface defines no bucket. The sum equals `steps` |
| `usage missing` | `M` | orthogonal to the three buckets (ADR-056 §2.4's second partition); such rows are never priced and never enter a ratio's denominator |
| `money` | one line per currency, plus `plan rows: P (marginal 0)` | **sums never cross a currency** (§4.8: a family may span two); the plan rows are counted, not added to any currency line |
| `by class` | the five counts, each with `new_tok` (inferred) and money (same currency/`plan` rule) | the row-level labels carry through; no class figure is a new measurement |
| `top steps` | the five largest `new_tok`, as `#step (class, +k blocks)` | the "where did this session's prompt grow" answer |
| `cache` | `Σcached / Σin` over rows that carry usage | **measured**, the provider's own counters; `—` when no row carries usage. It is a ratio of measured figures, never presented as a saving (§7) |

**A reused session key is not a new session.** Clients generate the key; nothing stops a client from
reusing one after a day, and the store's own session TTL is what normally bounds the gap. The summary
therefore does not split a key's rows into "sessions": it prints the span, and a multi-hour `gap_ms` in
the middle is the reader's evidence. Inventing a boundary would be a fabricated fact.

### 2.5 The three labels, and the two accounting rules that decide the money columns

The labels are §7's, unchanged; this surface adds no fourth and uses three of them:

| Label | On this surface | Example |
|---|---|---|
| **measured** | a figure the provider itself counted, or a fact of the bytes (`usage.*`, block hashes, `lcp`) — **one observation, no counterfactual** | `cached 167,552` |
| **inferred** | a figure derived locally with no control (§7), and every block-token arithmetic (GAP-Q14) | `new_tok 5,844` |
| **unresolvable** | a figure the record cannot carry, printed `—` — **never `0`, never a guess** | `gap_ms` within one second; `in · out · money` on a `usage_missing` row |

**There is no `verified` figure on this surface, on purpose.** `verified` is §7's word for a difference
between two worlds (a control turn, the replay ladder), and a ledger row is one observation of one request:
the column that would tempt a reader into the forbidden inference — *"this step saved N tokens, `cached` is
high"* — is `measured`, and the surface's refusals (§2.7) keep it there. This is the ADR-058 correction:
the draft of this table labelled those counters `verified`, which is exactly the inference AGENTS
constraint 4 forbids.

And the two rules that the money column exists to keep:

1. **A plan row prints `plan`, not `0`.** §4.6 rule 4 makes an in-plan request's marginal price 0 in
   every cost bucket; `stats` states that as "the difference between the plan's marginal price and the
   metered account's real price shows up as the plan section's two lines". A per-step ledger has no such
   section, so the label moves into the cell. Without it, `$0.0000` reads as *free*, and §1.3's second
   finding is exactly the case where that misreading is expensive: 52.4 % of the window's miss-token volume
   is plan traffic whose cost is allowance headroom, not money.
2. **A ratio is computed over measured rows only, and says so when it is not.** `Σcached/Σin` excludes
   `usage_missing` rows and prints the excluded count beside it. A denominator that silently includes
   zeros is the same defect as an invented saving.

### 2.6 The prerequisite: `ts` must carry the millisecond it is documented to carry

This surface's third and fourth columns are arithmetic over `ts`, and today `ts` is truncated to the
second **by a step that reads as a millisecond formatter**. Three facts, each verified:

| # | Fact | Site |
|---|---|---|
| a | The contract says millisecond precision: *"RFC3339 UTC with millisecond precision, e.g. `2026-09-19T07:41:02.123Z`"* | `crates/vadis-core/src/trace.rs:35` |
| b | The writer's formatter is named `rfc3339_millis` but takes **epoch seconds** and multiplies by 1000, so the fraction is always `000`: `let ms_total = epoch_s * 1000` | `crates/vadis-proxy/src/accounting.rs:724-730`, called with `ctx.now_epoch_s` at `:535` |
| c | A **second** function of the same name, in the same crate, takes **microseconds** and does the arithmetic correctly (`(us % 1_000_000) / 1_000`) | `crates/vadis-proxy/src/health.rs:461-475` |

So the precise clock already exists one layer up and is thrown away at the write: the request facts
carry `now_us` — *"the same clock read as `now_epoch_s`, untruncated (µs)"* (`forward.rs:599-601`) — and
`now_epoch_s` is derived from it, not read again (`forward.rs:685`). Measured on the corpus of §1.3:
**8,260 of 8,260 rows end `.000Z`** — not one record in 14 days carries a fraction.

**The repair is wider than one call site, and narrower than a refactor.** Four record writers stamp `ts`
through the seconds-taking formatter, and each takes the second as an argument rather than a context:

| Writer | Site | Production callers |
|---|---|---|
| the decision record | `accounting.rs:535` (`rfc3339_millis(ctx.now_epoch_s)`) | every served and unserved attempt (the context is built at `forward.rs:726-741` and its streaming twin) |
| `mode_refused_record` | `forward.rs:134` | `crates/vadis-cli/src/lib.rs:506` |
| `too_large_record` | `body_limit.rs:71` | `crates/vadis-cli/src/lib.rs:726` |
| `refused_record` | `auth.rs:149` | `crates/vadis-cli/src/lib.rs:1052`, `crates/vadis-cli/src/reload.rs:1682` |

**Ruling (one repair, no new field, no `schema_version` move):** the four writers take the microsecond
instant and render it through **one** function whose argument unit is microseconds — the `health.rs`
arithmetic becomes the only implementation of the civil conversion (that file keeps only its `Option`
adapter, which encodes "a family that never switched", not a second arithmetic) — and `AccountCtx` gains
`now_us` filled from the *already-read* fact (`facts.now_us`), so no second clock read is introduced
(AGENTS constraint 2). The three refusal writers' `now_epoch_s` parameters become the same `now_us` at
their call sites, and `now_epoch_s` stays wherever a window, TTL or quota comparison needs seconds.
A record whose fraction is `000` remains valid: it says *that vintage truncated to the second*, exactly
the additive-vintage rule `config_digest` uses — never "the request started exactly on the second".

**Why this is a prerequisite and not an improvement.** Two of the ledger's columns are differences of
`ts`. With a 1 s floor, a 300 ms step prints `gap 0` (the truth is unknown, not zero, §2.5), two steps in
one second cannot be ordered, and `ts + overhead_ms` — the arithmetic that lines a client's failure
instant up with a trace row — is off by up to a second. A surface that ships that column is a surface
that is wrong 100 % of the time it is small. The repair is four writers, five call sites and two
in-crate unit tests (§5, §6.1).

**And the ordering rule that survives the repair.** `ts` is the request's **start**; a row is written
when the request **ends**. So rows sorted by `ts` are sorted by start, and any tie is broken by the order
the rows appear in the trace files — which is completion order. The reducer states this in the header and
orders `(ts, file order)`; it never claims a same-millisecond pair's order.

### 2.7 What the surface refuses to do

| Refusal | Why |
|---|---|
| print message, tool, reasoning or response **content** | ADR-005: the trace is the interface, and it carries hashes; content capture is ADR-003's `tee`, out of scope for v0.1 (constraint 3's observation boundary) |
| print a **live** duration for an in-flight request | the trace has no row until the request ends. Harness's own ledger leaves the in-flight cell blank rather than inventing a span; so does this one — the row simply does not exist yet |
| call `wall_ms` "vadis's overhead" | on a streamed row `overhead_ms` spans the whole request while `upstream_ms` is head arrival, so the difference is the model's generation time (the registered finding this repository already carries). The column is named `wall`, the header states the caveat, and no figure is summed across rows as "gateway overhead" |
| print a **saving**, a "tokens saved", or a counterfactual | a saving is a difference between two worlds and the trace holds one (§7). `new_tok` is a cost, not a saving; `cache` is a measured ratio, not a claim |
| derive the cache ratio from block hashes | the block order is vadis's, the provider's template order is not (§2.2), and the two readings were measured to disagree by a factor of four on real codex traffic. The provider's own `input_cached` is the column |
| reimplement the success/failure partition | ADR-056 §2.4 owns it; this surface renders it |
| extrapolate a window, a rate or a per-month estimate | §9.2's report is the window surface; this one is a session |
| fork or replay a step | resume/fork is the harness's, because it owns the loop. vadis's equivalent is `vadis replay` (same-code-path recomputation, §9.3, planned) and it replays a *request*, not an agent's branch |

---

## 3. Blast radius, in both directions

**Gained.** The one question `stats` cannot answer — *which step in this session cost me the money, and
which block broke my prefix* — becomes a command. On the corpus of §1.3 that question has a 16.1 %-of-
metered-money answer (`rewrite` steps), and it costs nothing in the serving path: the surface is a reader.
Also gained: a measured cache ratio per step from the provider's own counter, which is the number no
inference can fake; and a classification that makes a client retry (`repeat`) and a compaction
(`shorten`) visible as *different* rows, which no existing surface does.

**Paid.** One prerequisite repair to `ts` (§2.6) — four record writers, five call sites and two
in-crate unit tests, and a strictly more precise value in every future record. Two behaviour-free consequences: a table column gains three digits,
and a v2 record with `.000` must be read as a vintage, not as an instant. And the book gains one sentence
naming a surface that does not exist yet (§6).

**Unchanged, deliberately.** No forwarded byte; no trace field added or renamed; no `schema_version`
move; no store read or write; no `/metrics` series; no `/health` member; no gate, corpus, threshold or
envelope; no config key; no `vadis setup` row; `stats`' own figures and its `(gateway)` fallback.

---

## 4. The cases, and what each limb observes

Ids are **not spent by this draft** (§8): the card that lands the reducer takes the next free numbers from
`DESIGN` §12.8's heading, which at this HEAD reads `CONF-01…CONF-110`. What follows is the executable
outline so the case author is not inventing the accepts.

**A note on the rig, because it decides the shape.** Every existing conformance case drives a real
`serve` over a loopback mock and reads the trace back; a reader-only surface does not need the mock — it
needs a **fixture trace**, and writing one by hand is the one thing this repository does not do (a
hand-written fixture asserts the author's idea of the writer, `R74-0e`'s lesson). So the case is authored
in two halves: **generate** the trace by driving a real `serve` through a mock whose answers are canned
(two appends, one identical resend, one trimmed resend, one mid-prefix change, one usage-less answer, one
plan-route answer), then **assert the reducer's output over that trace**. The generating half is the
existing testkit, unchanged.

| Limb | What it pins | What turns it red |
|---|---|---|
| (a) the five classes | over the canned session, `class` is `first, append, append, repeat, shorten, rewrite` in that order, with `blocks`/`kept`/`at` values matching the diff the mock produced | a reducer that classifies by `turn_index` (which reports `2` for every row after the first, §9 F2); one that treats a shorten as a rewrite |
| (b) an absent measurement is absent | the usage-less answer's row prints `—` in `in`/`cached`/`out`/`money`, `no_usage` in `flags`, and the summary's `usage missing` count is 1 while the three buckets are untouched | a reducer that prints `0` (the "absent is not zero" class, §7) |
| (c) a plan row is not a zero | the plan-route row prints `plan` in `money` and the summary counts it in `plan rows`, adding it to no currency line | a reducer that prints `0.0000`, i.e. the §1.3 second finding's misreading |
| (d) the cache column is the provider's | the printed `cached` equals the mock's own `usage.input_cached` verbatim, **including** on a row whose block diff would imply a different hit rate | a reducer that derives the cache figure from `prefix.blocks` |
| (e) the partition is ADR-056's | a row carrying `errors[].details.error_class == "stream_truncated"` lands in `stream_incomplete`, not `succeeded`, and the three counts sum to `steps` | a reducer that counts by `result.status` alone |
| (f) the ts prerequisite | on a session whose steps are issued in quick succession, `start` carries a non-zero millisecond fraction and `gap_ms` is resolvable (not `—`); a fixture record with `.000` still parses and still prints a resolved gap when it can be | the base tree, where every `ts` ends `.000Z` and two same-second steps are unordered |
| (g) refuses to invent | `--session` for a key with no rows exits `2` with a message naming the trace dir; `--config` absent is a parse refusal, not a default | a reducer that prints an empty table and exits 0, or one that reads the XDG default config behind the operator's back |

Limb (f) is the one that must be authored **after** §2.6's repair lands, not before: it is the case that
would be red on every other limb's rig for a reason unrelated to the reducer.

---

## 5. The amendment list (frozen the moment this draft is accepted)

Two in-crate unit tests pin the **seconds convention** §2.6 removes. Neither is a conformance case, so
neither is an owner decision — they are the implementing change's to move, and they are listed here so
that change does not "fix" a red by loosening an assertion:

| # | File | Edit | Why |
|---|---|---|---|
| **A1** | `crates/vadis-proxy/src/accounting.rs:760-766` (`rfc3339_known_instant`) | the three anchors' arguments become microsecond instants (`0`, `1_789_256_462_000_000`, `951_827_696_000_000`) and the expected strings keep their `.000Z`; **one** new anchor is added with a non-zero fraction (e.g. `…_000_500` → `.500Z`) | without the new anchor nothing pins that the formatter carries the fraction, which is the whole repair |
| **A2** | `crates/vadis-proxy/src/auth.rs:370-380` (`refused_record_matches_the_spec6_table`) | the call's clock argument becomes `1_789_256_462_000_000`; the assertion's expected string is unchanged | it feeds a **second** to the same formatter; untouched, it would render a 1970 instant and read as a regression |

**No conformance case is amended**, and that was checked rather than assumed: the cases that write a
`ts` build their own fixtures with their own helpers (`conf_84:114` a hand-written `….000Z`,
`conf_87:754/1024` its own `rfc3339(now_ms())`), so the writer's precision does not reach them; the cases
that *read* `ts` parse RFC3339 and sort, which is unchanged.

---

## 6. Landing plan

### 6.1 The code (two changes, in this order)

| Change | Files | Content |
|---|---|---|
| **P (prerequisite, ~60 lines across 8 files)** | `crates/vadis-proxy/src/accounting.rs` (the formatter takes µs — the `health.rs` arithmetic becomes its body; `AccountCtx.now_us`; the write at `:535`; its unit test's anchors), `crates/vadis-proxy/src/health.rs` (the twin's arithmetic goes, the `Option` adapter stays and delegates), `crates/vadis-proxy/src/{forward,stream_forward,auth,body_limit}.rs` (the four writers take `now_us`; `AccountCtx` initializers pass `facts.now_us`), `crates/vadis-cli/src/lib.rs` + `reload.rs` (four call sites), `crates/vadis-core/src/trace.rs:35` (doc only: the field is the request's **start**) | §2.6. No new field, no version move, no second clock read |
| **L (the surface, one crate)** | `crates/vadis-cli/src/steps.rs` (new), `src/main.rs` (one subcommand arm beside `stats` at `:46`), `src/lib.rs` (module + re-export), `docs/spec.md` §9.4's status line → *served* | §2.1-§2.5. Pure function over the trace files; the only I/O is `read_to_string` and the config load `stats` already performs |

**The reducer's own shape, stated as a constraint** (so a later reader does not "improve" it): it is a
function `[DecisionRecord] → Report`, it takes **no clock** (every time value comes from a record), it
sorts by `(ts, file order)` and nothing else, and it holds the whole session in memory (the largest
session in §1.3 is 441 rows; a session that needs streaming is a session nobody has).

**What `crates/vadis-core` does *not* gain.** The classification is a reader's convention (§2.2); putting
it in core would invite a future `DecisionRecord` field, which is a schema change with a version
question. It lives in the CLI, beside the only consumer, and `--json` is the extension point.

### 6.2 The discoverability half

A capability that lands only in `crates/` and `book/` is invisible on the three paths a user enters by
(AGENTS' own rule for routing capabilities): the example pair, `vadis setup`, and `README.md`.

- **`config.example.yaml` / `setup`**: **nothing**. This surface adds no key; `--config` is an existing
  argument, and `setup`'s askable set is contract (§4.11) — adding a row here would be a §4.11 change
  for no key.
- **`README.md`**: one row in the feature table, when it is *served*, naming the command and the one
  question it answers.
- **`book/observability-and-accounting.md`**: the sentence that today names `vadis replay` and
  `vadis trace tail` as planned gains a third name; when the surface lands, a short section under
  "Reading the reports" with one worked example (a session's summary and two rows), no price numbers
  (single-source price policy), and the three labels restated in the reader's words.

---

## 7. Alternatives considered

| Alternative | Why rejected |
|---|---|
| a **content** ledger (Trajectory's own shape: tool payloads, reasoning, diffs) | it stops being a gateway: content capture is ADR-003's `tee` plus a privacy and retention contract, and the trace's own rule is *hashes, never the body*. Refusing it is what keeps this surface cheap enough to be default-on |
| fold it into `vadis stats` | §2.1: a window of aggregates and an ordered session have different output shapes, and `stats` is the one surface with a live-gateway fallback |
| expose it as `/metrics` series | unbounded label cardinality on a process-wide scrape surface (§2.1) |
| read the **store** for session state (bindings, cooldowns, plan rows) | the store is EXCLUSIVE-locked while `serve` runs, so the surface would be unusable exactly when it is most wanted. The trace carries what this ledger needs; the event log is state truth for a different question (ADR-010) |
| derive the cache column from block continuity | measured to disagree with the provider's own counter by a factor of four on real codex traffic (block order vs template order, §2.2) |
| name the unit a **turn** | vadis sees requests, not the agent loop's turn boundary (§2.1) |
| print the step classes without the underlying counts | a word without its relation is an interpretation; §2.2 puts `kept`/`blocks`/`at` beside it |
| classify by `identity.turn_index` | the field is not an ordinal in production (every row after a session's first reports `2` — §9 F2) |
| make the surface stream a running session live (`--follow`) | that needs the in-flight state the trace deliberately does not carry (§2.7), and a wrong duration is worse than a blank one |

---

## 8. Register

### 8.1 IDs spent

| ID | What it is |
|---|---|
| **`ADR-057`** | this file |
| **spec `§9.4`** | the surface's contract (`§9.1`/`§9.2` are the served read-outs, `§9.3` the not-served list; numbering is by landing time, as `§4.20` followed `§4.19`) |
| **`CONF-111…CONF-117`** | spent by the implementing card (the seven limbs of §4, one case each), allocated from `DESIGN` §12.8's `CONF-01…CONF-110` heading — the draft itself allocated none, as promised |
| **`CONF-118`** | spent by the review-repair card that followed the L review's REQUEST CHANGES (2026-10-18): the `--json` form read where the bytes are made (`report_json`/`render_text`), pinning `--last`'s tail slice, column 16's integer typing, §9.4.5's five counts in both forms and §2.2's header caveat. No §2 ruling moved — the fixes implement §2.1–§2.5 as written; §2.4's `showing: {last, of}` summary member is the one shape addition (§9.4.1's `--last` row states it) |

### 8.2 What this draft adds to the documents

- `docs/spec.md`: **§9.4** (the whole contract), **§9.3** (one more name in the not-served list, with
  the one sentence saying what it will answer).
- `design/DESIGN.md`: **§12.9** (one register paragraph — this draft's, with the three findings §9).
- `book/observability-and-accounting.md`, `book/roadmap.md`: the not-served sentence each already
  carries gains the third surface.
- **Nothing else.** `crates/`, `tests/`, `config.example.yaml`, `README.md`, `rules/` are untouched.

---

## 9. Findings this draft registers

Three mismatches surfaced while designing the columns. Two of them the ledger simply refuses to depend
on, which is the point of writing them down here: **a surface must not be built on a field that does not
say what its own contract says.**

| # | Class | Site | Effect | Owner | Due |
|---|---|---|---|---|---|
| **F1** | unreachable-vocabulary | `identity.client`'s contract names four values — `codex` / `hermes` / `claude-code` / `other` (`crates/vadis-core/src/trace.rs:84-86`) — and **every** writer passes the literal `"other"` (`forward.rs:141`, `accounting.rs:548`, `auth.rs:157`, `body_limit.rs:78`, `trace_sink.rs:178`; the two test fixtures at `trace.rs:547` and `store/lib.rs:1139` copy it). Measured: **8,260 of 8,260 records** carry `other` | a documented vocabulary member no code can produce — the same defect class as a documented `plan_switch` reason (DESIGN §12.8's registered finding). **The ledger prints no client column in v1**; the column arrives with the change that fills the field, with its own witness (the inbound headers exist at the pipeline's entry, so the value is computable once and threaded like `session`) | the round that wires UA normalisation | not this draft |
| **F2** | wrong-semantics | `identity.turn_index` is written as `requests_seen + 1` (`forward.rs:377-389`), and `requests_seen` increments **only on a binding write** — a sticky hit writes nothing (`crates/vadis-store/src/lib.rs:416-445`, whose own comment says *"requests_seen == the session's `session.bound` count"*). Measured: **7,036 of 8,260 records report `turn_index: 2`**; only 227 report `1` | the field's serving consumer is **unaffected** (ADR-014's probe boundary asks `turn_index == 1`, which is still exactly a session's first request — a *repair* here must not move that gate, and moving it would be an ADR-014 reversal). What is wrong is the *record's* meaning: an analysis surface reading it as an ordinal is off by the number of sticky hits. `conf_15_prefix_continuity.rs:149` asserts the ordinal meaning and cannot see the drift, because its rig is exactly **two** requests — the number at which the drift starts. **The ledger assigns its own `step`** and never reads this field; the fix is a projection-write change with its own ADR review | the round that owns the session projection | not this draft |
| **F3** | contract-contradicted | `ts`'s documented millisecond precision (`trace.rs:35`) against a formatter that takes **seconds** (`accounting.rs:724-730`), called with a second (`:535`), while a **correct µs twin** lives in the same crate (`health.rs:461-475`). Measured: **8,260 of 8,260 records end `.000Z`** | two of the ledger's columns are differences of `ts`; §2.6 rules the repair and §5 freezes the two unit tests it moves. **Repaired by the prerequisite change (§6.1 P)**, because the surface cannot ship without it | this draft's implementation card | with the surface |

---

## 10. What a future reader must not conclude

- **Not** *"vadis now records content."* It records hashes, counts and timings; the step's *relation* is
  what makes it readable. Content capture is still ADR-003's `tee`, still out of scope.
- **Not** *"the class word is what the client did."* `shorten` is "the new block list is a strict prefix
  of the old one". It may be a compaction, a trim, or a rewind; the relation is the fact, the word is its
  name (§2.2).
- **Not** *"a plan row cost nothing."* It cost allowance headroom that this product cannot price
  (GAP-Q17); the cell says `plan` for that reason (§2.5).
- **Not** *"`new_tok` is a saving."* It is a cost, and it is **inferred** (GAP-Q14) — a proportional
  attribution of a measured total.
- **Not** *"the cache ratio came from the prefix analysis."* It came from the provider's own counters;
  the prefix analysis is deliberately not used for it (§2.2, §2.7).
- **Not** *"`wall_ms` is vadis's overhead."* On a streamed row it includes the model's generation time;
  the column is named for what it measures (§2.7).
- **Not** *"a missing figure is zero."* `—` means the record cannot carry it; the three labels are the
  whole accounting discipline of this product (§2.5).
- **Not** *"`vadis steps` is a Trajectory clone."* It is the same *shape* — a step ledger, every row
  attributable — applied to the only unit a gateway owns. Harness can show you what the model read;
  vadis can show you what it cost and who paid. The two are complementary, and neither is a subset.
- **Not** *"`identity.turn_index` and `identity.client` were fine before this draft."* They were
  unreachable and mis-specified respectively; this draft registers both and builds on neither.
