# ADR-041 — `GET /metrics` as a served surface: the authority, the frozen metric contract, the single-owner rule, and the one vocabulary word a refusal spends

- Status: accepted
- Date: 2026-09-27
- Kind: **contract**. This card changes **no product byte**: `crates/**`, `tests/conformance/tests/**`
  and `config.example.yaml` are outside its write set. What lands here is the authority record, the frozen
  metric contract (spec §4.16), the §9.3 move, the DESIGN landing (§12.21 + §12.8's row/paragraph) and the
  README sites the round's writer card must change. **The amended assertion and the code land together in
  one card** (§2.5), so the tree is never red in between.
- Authority: the **owner's direction of 2026-09-27, item 2 of four**, relayed to the loop through this
  round's card `t_87e4b60b`; quoted verbatim in §1.1.
- Supersedes (in the narrow sense of §1.2): the W1 dossier's *dropped* item (`w1-dossier.md`, the
  "Candidates considered and dropped" entry) and the competitiveness plan's *park* (`:134`). It supersedes
  **nothing else** — §1.3 enumerates what it does **not** license.
- Related: `AGENTS.md` constraints **1** (the byte boundary — the client's bytes and exactly two permitted
  mutations), **2** (content determinism), **3** (the observation boundary), **4** (no unverified savings
  presented as measured), **5** (no fabricated prices), **8** (docs before code), **9** (**the measurement
  is not part of the search space**); ADR-005 (the trace is the only product → autowork channel);
  ADR-006 (integer nano accounting); ADR-012 (the never-mutable paths); ADR-015 (the byte mutations);
  ADR-022/023 (the walk's eligibility and its refusals); ADR-040 D2/D5 (the capture-once rule; the keys a
  reload refuses); spec §4.1 (`:370`), §4.7 (`:675`), §4.8 (`:734`), §4.13 (`:1338`), §4.15 (`:1482`),
  §6 (`:1694`), §7, §8 (`:2029`), §9.1 (`:2156`), §9.2 (`:2285`), §9.3 (`:2414`); DESIGN §12.6 (`:730`),
  §12.8 (`:950`), §12.11 (`:2699`), §12.21 (**new**), §13.1 (`:3898`).
- Cases: `CONF-46` (**amended**, `tests/conformance/tests/conf_46_metrics_is_served.rs`, renamed from
  `conf_46_metrics_is_bare_404.rs`) and `CONF-87` (**new**,
  `tests/conformance/tests/conf_87_metrics_single_owner.rs`). The id is claimed from a measurement, not
  from memory: §2.3.
- Numbering note (observation, not a decision): the ADR register's newest landed entry is `ADR-040`;
  **`ADR-041` and `ADR-042` are unused** (`design/decisions/ADR-043-ingress-three-verdicts.md:22-24`
  records the same observation, and `autowork/harness/r47-0b/EVIDENCE.md:44` the 40-ADR count behind it).
  This ADR carries the number the round's cards fixed for it.
- **Line-number convention (so every `path:line` below can be checked).** Every reference resolves at **this
  branch's HEAD** — the commit that carries this ADR — and where a number is a *measurement of the base*
  commit `ac44181` the sentence says so (the case-directory occupancy in §2.3, and §9.3's removed lines in
  §1.2/§2.1). The two documents this card edits shift under it, so the deltas are stated rather than left to
  be discovered: in `docs/spec.md`, §4.16's insertion moves everything below it by **+112** (§6's heading
  `1582 → 1694`, §9.2's `2173 → 2285`) and §9.3's own rewrite adds **+5** more below §9.3 (`:2316-2322 →
  :2433-2439`); in `design/DESIGN.md`, §12.8's two new rows and the `CONF-87` allocation paragraph move
  §12.11 by **+28** (`2671 → 2699`) and §12.21's insertion moves §13.1 by **+126** (`3772 → 3898`). The
  remap was applied mechanically and each pair's replacement count is in
  `autowork/harness/r50-0/remap-refs.py`'s own output, with every resolved line printed in
  `autowork/harness/r50-0/anchors.txt` beside the claim it supports.

---

## 1. The authority for serving a surface the record said not to serve

### 1.1 The owner's authorisation, verbatim and dated

**The owner's direction of 2026-09-27 (item 2 of four)**, relayed to the loop by this round's card
`t_87e4b60b` (card body, "Round R50", first paragraph), verbatim:

> *"authorised: amend `CONF-46` and `spec` §9.3, and implement `/metrics` to contract"*

Three facts about that sentence are part of the record, and none of them may be inferred away:

1. **It is an owner act, not a loop outcome.** The three artifacts it moves — a conformance assertion, a
   spec section, and the surface's shape — are each in the never-mutable set of `AGENTS.md` constraint 9 /
   ADR-012 ("the gate definitions, the fixed corpus, the conformance assertions and the L1 envelope are
   outside the loop's mutable scope"). No round may take this decision; a round may only carry it, which is
   what §2 does.
2. **It is dated and scoped**: *this* surface (`GET /metrics`), and *these* three artifacts. §1.3 lists
   everything it does not reach.
3. **It authorises the amendment, not a shape.** The metric names, labels, units and exposure rules are
   **not** in the owner's sentence; they are the implementing change's ruling (spec §9.3's own rule at
   `docs/spec.md:2433-2434`: *"a surface's shape is frozen by the change that implements it"*). This card is
   that change's contract, and §3 freezes the shape — which is also what keeps the sentence from being read
   as a licence to invent one.

### 1.2 What the ruling supersedes — and what the prior artifacts actually said

Every claim in this table was re-read at this card's HEAD (`ac44181`) or at the path named; the commands and
their raw output are in `autowork/harness/r50-0/anchors.txt`.

| Artifact | What it says, verbatim or closely | Superseded? |
|---|---|---|
| `$HERMES_HOME/kanban/boards/router/attachments/t_fc8fd11a/w1-dossier.md` (**the W1 reconciled dossier**, an attachment outside the repo), "Candidates considered and dropped (named, with the reason)": *"**Serve `router replay` / `GET /metrics` (gap probe G2) — DROPPED from this wave's pool.** ADR-sized (simulation seam + plugin-config surface), the axis (A5 observability) is crowded and sold by everyone …, and cheap-to-prove fails: the one number it would mint is obtainable via the existing harness for ≤ $0.01 (Candidate 2) without the ADR. It stays the right answer to *"who owns the product-side verified path"* — **an owner direction decision, registered as open human row, not a D-pool item this wave can justify.**" | **Yes — and the dossier said so itself.** The entry drops the surface *from one wave's pool* on **budget** grounds (ADR-sized, crowded axis, no number to mint), and in the same breath refers it to the owner. The owner's ruling is that referral being taken. Nothing in the entry measured the surface's *value*; nothing here is contradicted. |
| `$HERMES_HOME/plans/2026-09-25_130500-router-competitiveness-plan.md:134` | *"Semantic cache · MCP / A2A / gRPC ingress · dashboards / `/metrics` before v0.2 · SDKs · …"* — under the heading at `:132`, **"## Not doing (write it down so the loop does not drift into it)"** | **Yes, for the `/metrics` clause only.** It is a *filing* — a "so the loop does not drift" list, not a measurement and not a contract. The other entries in that line stay filed and untouched (MCP/A2A/gRPC were separately declined by ADR-043; the semantic cache is R51's question; SDKs and provider breadth are not this ADR's business). |
| the cut-spec card `t_1d86acb2`'s reopening rule (card body), verbatim: *"不重开被否决项（档案 §3 末尾四条…）；要重开必须给出档案里没有的**测量**"* — *do not reopen a rejected item; a reopening must supply a **measurement** the dossier does not have* | The rule is scoped to **the dossier's own four dropped items** (`ADR-043:29-31` names them: "`router replay`/`/metrics`", cache-breakpoint auto-injection, multi-tenancy/per-key identity, "Be Rust"/benchmark-chasing) | **The rule *does* govern this item — `/metrics` is one of the four — and the owner's ruling supersedes it for this item.** The rule is the owner's own standing instruction to the *loop*; an owner act is not bound by it. This is the honest reading and it is written down here so no reader later mistakes the event for a loop reopening a rejected item: **no round reopened anything**; the owner did, and this round carries it. Note also what the rule's *purpose* was — do not spend a round on a filing with no measurement behind it — which is exactly why the act had to be the owner's. |
| `docs/spec.md:2304-2314` at the base `ac44181` (§9.3's "not served" entry, the clause **this round removes**; at this HEAD the entry no longer exists and §9.3's heading is `:2414`) | *"…and `GET /metrics` (Prometheus) are **planned, not served**"*, with the sub-bullet at `:2310-2314` (*"not registered: the route answers a bare `404` … its metric names, labels and units are frozen by the change that implements it"*) | **Yes, the list entry moves** — that is half the authorisation. The sub-bullet's *expectation* ("frozen by the change that implements it") is precisely what §3 discharges. |
| `docs/spec.md:2433-2434` (§9.3's rule) | *"a surface's shape is frozen by the change that implements it, and a documented-but-unreachable surface is a defect"* | **No.** The ruling *is* that rule being obeyed: the surface stops being documented-but-unreachable because a change implements it and freezes its shape in the same round. |
| `tests/conformance/tests/conf_46_metrics_is_bare_404.rs` (its assertion, `:104-122`) | a bare `404`, no §8 body, no `X-Router-Request-Id`, with a `/health` 200 liveness control | **Yes, the assertion is replaced** (§2.2) — a frozen assertion, hence an owner act, hence this ADR. |
| `autowork/STATE.md:910` (R6-G2's row) | spec §9.3's two claims were *"witnessed by hand but **not** in the suite"*: `GET /metrics` → **404** with an empty body and no `content-type`; the parser's refusals → rc=2 | **Historic.** R6-G2 turned the hand-witness into `CONF-46`/`CONF-47`; this ADR re-points one of the two at its new truth. `CONF-47` (the parser's refusals) is **not touched** — `router replay` / `router trace tail` still do not exist. |

**One more delta the ruling does not close, stated here because it was registered as "the next round's
decision"**: `autowork/progress/2026-09-27_12-14-42_R49-readme.md:231` (`R49-2-F6`) records that the bare
universal *"one DecisionRecord per request"* (`docs/spec.md:1694`, §6's own heading; the same sentence at
`:2436`) is over-general — an abandoned-mid-stream request leaves no record at all (README's own carve-out at
`:341-344`, DESIGN §12.10.3 R5). That finding's standing instruction is *"no card may edit the contract for
this; it is the next contract round's decision"*, and its carriers are ≥ 4 (spec §6's heading, spec §9.3's
closing sentence, `autowork/harness/r49-0/CLAIM-SOURCES.md` rows C3/C40 — another card's artifact, outside
every R50 write set — and DESIGN's repeated phrase). **This card does not edit it**: a partial narrowing is
the same class of defect as the one being closed (R49's own completeness lesson,
`progress/2026-09-27_12-14-42_R49-readme.md:150-160`), and the atomic edit needs a card that owns all four
carriers. Registered in §8 as `R50-0-F1` with the recommended home.

### 1.3 What the authorisation does **not** license

The sentence authorises **one surface** and the two documents that describe it. By its own terms it does not
reach:

- **any other route.** Not `/mcp` (STATE.md row 20's question, still the owner's), not a dashboard, not
  `router replay`, not `router trace tail`, not a second metrics path (`/metrics/v1`, a query-parameterised
  variant, a per-window endpoint). What is registered today is exactly what `serve` assembles at
  `crates/router-cli/src/lib.rs:891-910` — `health_router`'s `/health` (`:584-591`) plus the three
  `.merge(guarded_protocol_route(…))` sites (`:892`, `:898`, `:904`) — i.e. four path literals, which one
  grep counts: ``grep -c '"/health"\|"/v1/chat/completions"\|"/v1/responses"\|"/v1/messages"' crates/router-cli/src/lib.rs``
  → **4** at HEAD (measured; the same file through `grep -c 'route('` reads **8** and measures nothing —
  two `fn` definitions, two `.route(` builders and `route_layer`). R50-1 adds `/metrics`' own literal, so
  the pattern's count reads **5** after it: one new path, not a category.
- **the byte boundary.** No client byte is read, mutated, or echoed by this surface; the two permitted
  mutations of `AGENTS.md` constraint 1 stay exactly two (ADR-015 item 1/5).
- **any translation cell or protocol semantics** (ADR-022/023, ADR-043's (a) verdict).
- **the trace schema.** `TRACE_SCHEMA_VERSION` stays **2**; no field is added, removed or retyped; the one
  vocabulary widening is the *value* word `"metrics"` in an existing string field (§3.8), and it is
  enumerated there as a consequence rather than left to be discovered.
- **any config key.** No key is added to `config.example.yaml`, the loader or the parser — which is also why
  the window is a constant (§3.3) and why "a configurable window" is registered as a widening with a trigger
  rather than smuggled in.
- **anything about §9.1 or §9.2's figures.** `/health`'s members, `router stats`' lines, the provenance
  table and the omission rule are the surface's *input contract*, not its output; none of them changes here.
- **the measurement**: no gate definition, no threshold, no corpus, no L1 envelope, and no *other* case's
  assertion. `CONF-43`'s docs ↔ CLI relation is untouched (this surface is a route, not a subcommand).

---

## 2. What changes on the frozen surface, enumerated

### 2.1 The §9.3 move (`docs/spec.md`)

§9.3's list entry for `GET /metrics` leaves the "not served" list — the opening enumeration and the
sub-bullet that carried it are **gone** (base `ac44181`: `:2304`, `:2310-2314`; at this HEAD §9.3's heading is
`:2414`) — and becomes a **served** statement pointing at the new §4.16. `router replay --trace … --config …`
and `router trace tail`
**stay** in the list, and the section's closing paragraph (`:2433-2439`) stays — its rule (*"a surface's
shape is frozen by the change that implements it"*) is the sentence this whole round is obeying, and its
*"until they land, the trace record of §6 **is** the interface"* remains true of the two subcommands that
have not landed. So exactly one clause moved: the two remaining members and every other sentence of the
section are byte-identical, and what replaced the clause is a pointer at §4.16 plus the three sentences the
old entry had **promised** (integer nano money with the currency as a label, §9.2's own figures, nothing
new computed) — the promise kept rather than re-stated as an intention.

### 2.2 `CONF-46`: its assertion is **replaced**, and its file is renamed

`tests/conformance/tests/conf_46_metrics_is_bare_404.rs` (126 lines) asserts a bare `404` and the absence of
§8's body and header. After this round the route answers a served surface, so **every limb of that
assertion is false** — the case cannot be "extended", it is replaced (the id is a contract and stays `46`;
the file *name* is a description, and a file called `…_is_bare_404.rs` asserting a served surface is a lie).
The frozen obligations the old case carried are **conserved limb for limb** — see §5, which is the answer to
"does the amended case still protect what it protected?"

- New file: `tests/conformance/tests/conf_46_metrics_is_served.rs`; old file deleted in the same commit.
- The liveness control (`/health` → 200 on the same run) is kept.
- The `404`/`501` exclusion becomes the **status-set** assertion: the admitted arm is `200` and *nothing
  else*; the refused arm is `401` and *nothing else*; a `501` or any other status is a defect.
- The "no §8 error body, no `X-Router-Request-Id`" assertion is kept **on the admitted arm** and is
  *sharpened*: the 401 arm **does** carry §8's body and header — because it is §4.7's guard refusing, which
  is §8's contract, not this surface's answer (spec §4.7's refusal table, `docs/spec.md:726-727`).
- It **lands with the implementation**, in one card (R50-1). At *this* card the old assertion is still true
  of the tree and `CONF-46` is still **GREEN** (the code answers a bare 404 at `ac44181`); the two files
  (case and code) change together, so **the tree is never red in between** — neither on `main` nor on this
  branch. That is why this card may not write the amended assertion "ahead of" its implementation.

### 2.3 The new case: the id is claimed from a measurement

| Step | Measurement at this card's HEAD (`ac44181`) |
|---|---|
| the tree's case files | `ls tests/conformance/tests/*.rs \| wc -l` → **76**; the ids present are `01–47, 53–66, 71–78, 80–86` |
| the register's occupancy (DESIGN §12.8's allocation paragraphs) | the last of them (**R43/CONF-85**) closes with *"the spent ID set is `01–47, 53–70, 71–85`; `48–51` stay reserved exactly as the paragraphs above leave them; the next free ID is **`CONF-86`**"* (`design/DESIGN.md:1466-1468`; `:1448-1465` at the base `ac44181`). Its `53–70` is a **stale slip the paragraphs above contradict** — `CONF-52` **is** spent, with its witness deliberately a `compile_fail` doctest in `router-core` rather than a case file (`design/DESIGN.md:1142`, `:1208`) — so the register's own accumulated spent set at this HEAD is `01–47`, `52–70` (67–70 file-less, R22), `71–85`; reserved `48–51` |
| the tree's maximum | **86** — a file for it exists (`conf_86_setup_check_roster_fact.rs`) although §12.8 has no row for it (**`R46-4-F2`**, `progress/2026-09-26_19-57-19_R46-overdue-register.md:204`; open, "next docs round") |
| therefore the lowest free id | **`CONF-87`** — the first id above both the tree's maximum and the register's own (stale) next-free claim. Taken by this card, on the file `tests/conformance/tests/conf_87_metrics_single_owner.rs`. |

The id is **spent**: not renumbered, not reused (§12.8's closing rule, `design/DESIGN.md:1496-1498`;
`:1472-1474` at the base `ac44181`). Its row
and its allocation paragraph are §4's DESIGN change. §4 also closes the registry drift the measurement
exposed — the missing `CONF-86` row and the stale heading range — as a **registry repair** (`R46-4-F2`), with
no case file and no assertion touched.

### 2.4 The DESIGN landing

- **§12.8** (the conformance case table): a new row for `CONF-87`; a new allocation paragraph; the amended
  `CONF-46` row rewritten to the served surface (the row is the register, and a row that describes a bare
  `404` after this round is the drift the section forbids); the registry repair of §2.3; the heading range.
- **§12.21** (new, appended after §12.20): the landing — the route's **home**, the guard seam, the
  single-owner rule, the invariants and their cases.
- **§12.11** (the auth landing): one sentence recording that the guard's refusal record takes the endpoint's
  own protocol word as a string, so a non-protocol guarded route can name itself (§3.7).
- **§12.6** is **not** changed: `ProtocolRec.protocol_in` is a `String` in the landed type
  (`crates/router-core/src/trace.rs:87`); the *value* vocabulary gains one word (§3.8).

### 2.5 The amended assertion + the code land in one card — stated explicitly

The card that lands the implementation (R50-1) is the card that writes the new `conf_46` and the new
`conf_87`, in the same commit as the code. Consequences, each of which is a thing that could be got wrong:

- At `R50-0` (this card) and at `main`: 4 case files' worth of *nothing* changes; the ledger identity is
  reproduced unchanged (§7).
- `CONF-46` is **GREEN** at this card's HEAD by construction (the code still answers the bare `404` its
  *current* file asserts).
- `CONF-87` cannot be green before the surface exists: it is **not** parked `#[ignore]`d across a card
  boundary (the CONF-27/41/42/45/57 parking rule exists for cases written ahead of their *implementation*,
  and this case is not written ahead of one — it lands *with* it). If R50-1's schema turns out to need a
  parked interval, the parking rule applies unchanged and the round's record must say so.

---

## 3. The metric contract, frozen here (spec §4.16)

### 3.1 The surface

`GET /metrics`, served by the same axum assembly that serves `/health` and the three protocol routes
(`crates/router-cli/src/lib.rs:891-910`), from the **same listener** and the same process. It is a
**guarded** route: §3.7 decides that, and `docs/spec.md:718-720` is why it cannot be otherwise (§4.7's
exemption is `/health`'s alone — *"Nothing else is exempt"*).

### 3.2 Format and media type

| Property | Value |
|---|---|
| success status | `200` |
| `content-type` | `text/plain; version=0.0.4; charset=utf-8` — the Prometheus text exposition format (`# HELP`, `# TYPE`, `name{labels} value`); **not** OpenMetrics (its `# EOF`/exemplar surface buys nothing here and every consumer accepts 0.0.4) |
| metric type | **`gauge` for every series** (§3.6) |
| numbers | counts and token quantities: unsigned integers; **money: integer nano of the series' own `currency`** (§4.8 — never a decimal); ratios: the report's own displayed precision, **exactly four decimal places** (spec §9.2 prints `0.9898`); p99: integer milliseconds |
| time | **no timestamp, no process uptime, no `# generated` line** — the scrape's own instant is the scraper's (`timestamp()`), and a value derived from the wall clock would make two scrapes disagree for no reason (§5 limb 9) |
| body size | O(series): under **8 KiB** for the frozen set, independent of the record count (§5 limb 7) |

### 3.3 The window is a frozen constant, stated in-band

The exposition reports **the last 900 seconds** (15 minutes) of the configured `trace.dir`. The value is a
**process constant** — `router_cli::metrics::WINDOW_MS: i64 = 900_000` — not a config key, not a query
parameter, not a header:

- **Not a query parameter**: an unauthenticated-adjacent knob that widens a read is exactly the unbounded
  surface `CONF-46` has always excluded; a scraper (Prometheus, VictoriaMetrics, a curl in a cron) cannot be
  relied on to send one, and a per-scrape-varying window makes two scrapes incomparable.
- **Not a config key**: the owner's sentence authorises *this surface*, and adding a key means the loader,
  `config.example.yaml`, the parser, the reload's refused-set question (ADR-040 D5 — is a window change
  live?) and a conformance case — a configuration-contract change, not a metrics one.
- **Stated in-band, always**: `router_metrics_window_seconds` is a series of every response, and it is the
  metrics surface's answer to spec §9.2's rule that a windowed report must say which window it covers
  (`docs/spec.md:2297-2299`: *"A default would let a report be printed without stating the window it
  covers, which is exactly what §7's reporting requirement forbids"*). A reader never has to guess the
  scope, and it changes only when the constant
  does.
- Cost of the choice: the window is not tunable without a release. Registered as `R50-0-F3` (§8) with the
  trigger: *a round that wants a configurable window owns the key, its read site, the reload's answer for
  it, and the case.*

### 3.4 The series — names, labels, units, sources

Every name is `router_<quantity>[_<unit>]`; `provenance` carries **spec §9.2's own label column** as a
Prometheus label, so the §7 labelling rule travels machine-readably (a consumer may filter
`provenance="verified"` and cannot accidentally average an inferred figure into a measured one).

| # | metric | labels | unit | the figure it renders | §9.2 label |
|---|---|---|---|---|---|
| 1 | `router_metrics_window_seconds` | — | seconds | the surface's own constant (§3.3) | — |
| 2 | `router_metrics_omitted_figures` | — | count | **the response's own bookkeeping**: how many of §3.5's omission arms fired (never a figure over records) | — |
| 3 | `router_trace_files_read` | — | count | `Report.files_read` — the rollover files this read opened | count |
| 4 | `router_requests` | — | count | `TraceFigures.requests` (`requests`) | count |
| 5 | `router_requests_succeeded` | — | count | `.succeeded` (`succeeded`) | count |
| 6 | `router_requests_failed` | — | count | `.failed` (`failed`) | count |
| 7 | `router_failures_by_kind` | `kind` | count | `.failure_kinds[kind]` — the report's `(upstream_error 2, upstream_timeout 1)` split; `kind` ∈ §8's closed vocabulary (`docs/spec.md:2039-2053`, incl. `unauthorized` and `request_too_large`) | count |
| 8 | `router_requests_usage_missing` | — | count | `.usage_missing` | count |
| 9 | `router_cost_nano` | `tier`, `currency`, `provenance` | integer nano **of `currency`** | `.input_miss_nano[c]` / `.input_hit_nano[c]` / `.cache_write_nano[c]` / `.output_nano[c]`, `tier` ∈ `input_miss`\|`input_hit`\|`cache_write`\|`output` | **verified** |
| 10 | `router_cache_input_cached_tokens` | `provenance` | tokens | `.input_cached_tokens` — the hit rate's numerator, as a measurement | **verified** |
| 11 | `router_cache_input_tokens` | `provenance` | tokens | `.input_total_tokens` — its denominator | **verified** |
| 12 | `router_cache_hit_rate` | `provenance` | ratio (0..1) | §9.2's `hit rate` (`cache_hit_rate(figures)`) | **verified** |
| 13 | `router_prefix_continuity_p50` | `provenance` | ratio | §9.2's `continuity p50` (`median(figures.continuity)`) | **inferred** |
| 14 | `router_transform_savings_tokens` | `provenance` | tokens | `.verified_savings_tokens` / `.inferred_savings_tokens` — one series per label present | verified / **inferred** |
| 15 | `router_plan_switches` | `family` | count | `.switches` | count |
| 16 | `router_plan_switch_cost_nano` | `family`, `currency`, `provenance` | integer nano | `.switch_cost_verified_nano[c]` | **verified** |
| 17 | `router_plan_switch_reprefill_tokens` | `family`, `provenance` | tokens | `.reprefill_tokens` | **inferred** |
| 18 | `router_plan_switch_reprefill_cost_nano` | `family`, `currency`, `provenance` | integer nano | `.reprefill_cost_nano[c]` | **inferred** |
| 19 | `router_plan_switches_without_usage` | `family` | count | `.switches_without_usage` | count |
| 20 | `router_stateful_inbound_rate` | `provenance` | ratio | §9.2's `stateful inbound rate` (`stateful_inbound_rate(figures)`) | count |
| 21 | `router_overhead_ms_p99` | `provenance` | milliseconds | §9.2's `overhead p99` (`p99(figures.overhead_ms)`) | **measured** |

`family` is the loaded `plan_policy.family` verbatim (`docs/spec.md:2260`); series 15–19 exist **only when
the loaded config declares a `plan_policy`** — §9.1's no-fabricated-plan-section rule (`docs/spec.md:2259`)
applied to this surface, so a policy-less process exposes no plan series at all.

**Four emission rules, each one a place a wrong surface would lie:**

1. **Money is per currency and never summed across them** (§4.8). A money series exists **only for a currency
   the window actually holds** — never a zero series for an absent currency, mirroring §9.2's omission rule
   (`docs/spec.md:2392-2397`). The `tier` label is exhaustive and there is **no total series**: a
   `tier="total"` would double-count under a PromQL `sum()`, and the total §9.2 prints is one `sum()` away.
   The `# HELP` line says both sentences.
2. **A figure that cannot be computed is omitted with a named reason — never a 0** (§9.2's own rule,
   `docs/spec.md:2410-2412`; AGENTS constraints 4/5). Zero is a *read* ("this window had none"); the absence
   is a *hole*, and every hole writes a comment line (§3.5).
3. **The failed-by-kind split and the transform savings series are emitted only for labels the window
   produced** — never a synthetic zero for a kind or a verdict nothing carried (a `provenance="inferred"`
   savings series appears only when inferred savings exist).
4. **Nothing else is ever emitted.** No series whose value is a config value other than `family` and the
   window, no series keyed by a request, and no series whose label value comes from traffic (§3.9).

### 3.5 The omission arms, and the comments that name them

The exposition has no stderr, so §9.2's "with a one-line note" becomes an **in-band `#` comment line**
(Prometheus ignores comment lines, so the comment is free and cannot confuse a parser). One frozen family,
and the surface's own bookkeeping series counts what fired:

| arm | when | comment (frozen wording) | `router_metrics_omitted_figures` |
|---|---|---|---|
| `unknown_outcome_requests` | **always** (§3.10) | `# router: unknown_outcome_requests omitted — the figure lives in the event log and this surface does not scan it` — **this string is normative in spec §4.16 and is repeated here verbatim**; a case asserts the substring `unknown_outcome_requests omitted` | 1 |
| the trace read | `trace.dir` cannot be read/listed (a live dir removed under a running process) | `# router: trace unreadable — <the reason the read gave>; every trace-derived figure is omitted` | 1 + the arms below that therefore fire |
| `router_cache_hit_rate` | the window holds no input tokens (`input_total_tokens == 0`) | `# router: cache_hit_rate omitted — no input tokens in the window` | +1 |
| `router_prefix_continuity_p50` | no record in the window carries a non-null `prefix.continuity` | `# router: prefix_continuity_p50 omitted — no continuity sample in the window` | +1 |
| `router_overhead_ms_p99` | no record in the window carries `result.upstream_ms` | `# router: overhead_ms_p99 omitted — no upstream-measured record in the window` | +1 |
| `router_stateful_inbound_rate` | the window holds no records (`requests == 0`) | `# router: stateful_inbound_rate omitted — the window holds no records` | +1 |
| the money series | the window holds no currency | `# router: no currency in the window — no money series` | +1 |

An empty window is **not** a failure: `200`, the counts present as `0`, the money/ratio/quantile series
absent by the rules above, the window series present. A **partially** readable window is never presented as
a whole one (§9.2's *"a partial report is never presented as a complete one"*).

### 3.6 Gauges — and why not counters

Every series is a **`gauge`**, and no name ends in `_total`. Reasons, in order of weight:

1. **The figures are window-scoped and may go down.** A record ages out of a 900s window; a
   `_total` suffix is a promise of monotonicity, and Prometheus's own tooling (`rate()`, `increase()`,
   `resets()`) acts on that promise. Naming a windowed count `…_total` would produce silently wrong rates —
   the class of number `AGENTS.md` constraints 4/5 exist to forbid.
2. **A true counter needs a second derivation.** Process-lifetime accumulators would have to be maintained
   somewhere in the serving path — a stateful second source of truth for figures the ledger already owns,
   i.e. exactly the defect §4's single-owner rule prevents (and ADR-010's event-log-as-truth is the
   repository's answer to "where does a cumulative number live"; this surface does not get to invent a
   second).
3. Reversibility: adding a *derived* `rate()`-friendly surface later is a new contract with its own ADR; a
   gauge set can be kept beside it. Nothing here blocks that.

### 3.7 The route sits **behind the token guard** — decided here

**Decision: `/metrics` is registered in the guarded set** (the same guard layer as the three protocol
routes, `crates/router-cli/src/lib.rs:785-794`, `:818-880`), not outside it like `/health`.

- **Spec §4.7 forbids the alternative in words.** `docs/spec.md:718-720`: *"**Nothing else is exempt**, and
  the exemption is **structural** — the guard is applied to the three protocol endpoints' routes only, never
  to `/health`"*, with the reason the exemption exists at all: a liveness probe *"cannot be used by whatever
  supervises the process"* if it needs a credential (`:717-718`). **A metrics scrape can carry a
  credential** — every scraper's config has a header field — so §4.7's own rationale does not extend to this
  surface; leaving it exempt would make a written sentence of the spec false, the same class of defect as a
  documented-but-unreachable surface.
- **The surface discloses figures, `/health` discloses configuration.** `/health` answers without a token
  because it reports *what was loaded* and names no secret (`docs/spec.md:2208-2224`). `/metrics` reports
  what the operator's traffic **spent** and **did** — spend, volumes, cache behaviour — and if the operator
  has turned auth on, that is exactly the data the token exists to protect.
- **The guard's own invariants** (`crates/router-cli/src/lib.rs:576-583`): the guard layer is *always*
  installed because the gate lives on the **revision** — a reload may introduce, rename or drop
  `server.auth_token_env` (ADR-040 D5) — and *"a route with no layer could never engage it"*, while *"a
  fourth route added later cannot silently inherit a wrong one"*. A `/metrics` outside the guarded set would
  have to be *carved out* by a path comparison, which is the one shape that sentence names as the thing not
  to do.
- **The consequence for the reader is one line of scraper config** (`authorization: Bearer …` or
  `x-api-key: …` — §4.7's two accepted forms, `docs/spec.md:697-705`), measured by `CONF-46`'s two-header
  limb.
- **Alternatives rejected**: (i) unguarded, `/health`-style — violates §4.7's written "nothing else is
  exempt" and the confidentiality argument above; (ii) guarded-but-secret (`?token=` query parameter) — a
  credential in a URL, logged by every intermediary, and a *second* admission path inside the process (the
  guard's single-owner reasoning); (iii) bound to loopback only, unguarded — the listener is
  `server.addr` (configurable, §4), so this is a second address rule, not a guard.
- **Reversible**: yes, in the direction that matters (`unguarded` → `guarded` is the change that would need
  the owner; `guarded` → exempt again is the same edit plus a spec sentence).

### 3.8 What a refusal spends: one word in an existing string field

A refused `/metrics` scrape is **§4.7's ordinary refusal**: `401`, §8's unified body
`error.type = "unauthorized"`, `details.header` naming the header the guard read, `X-Router-Request-Id`
present (`docs/spec.md:726-727`), **no** store row (`:730`), and **one trace record** — §6's pre-pipeline
class, whose `protocol.protocol_in` is `"metrics"`: *the endpoint's own protocol*, which is literally what
`docs/spec.md:1869` prescribes for that field.

The landed type already permits it: `ProtocolRec.protocol_in` is a `String`
(`crates/router-core/src/trace.rs:87`), and **no consumer branches on its value** — measured: a grep of
`crates/**` and `tests/conformance/tests/**` for `protocol_in` finds writes and per-case literal assertions,
never a match on the three wire words (`autowork/harness/r50-0/anchors.txt`, section "protocol_in
consumers"). So the vocabulary gains one word and no code path gains an arm. Consequences, stated:

- `router stats` counts a refused scrape in `requests`/`failed` and splits it out as `unauthorized`
  (§9.2's provenance rows already say a refusal lands there — `docs/spec.md:2371` is the `succeeded`/`failed`
  row, *"split by `errors[].kind`"*, and `:2370` the `requests` row); a metrics scan is
  therefore visible in the report beside a protocol-endpoint scan. That is deliberate: §4.7's own table says
  the trace is how *"am I being scanned?"* is answered (`docs/spec.md:731`), and a scan of the metrics port
  is the scan an operator most wants to see.
- What the **guard seam** must be, exactly: `router_proxy::refused_record`'s protocol parameter
  (`crates/router-proxy/src/auth.rs:126-133`) takes the endpoint's word as a **`&str`**, and
  `GuardState.proto_in` (`crates/router-cli/src/lib.rs:802-806`) with it; the three protocol routes pass
  `WireApi::Chat|Responses|Anthropic.as_str()` — **byte-identical on the wire, so `CONF-45` and every
  pre-pipeline case are unaffected** — and the metrics route passes `"metrics"`.
- **Rejected alternative 1 — a fourth `WireApi` variant** (`crates/router-core/src/config.rs:526-529`):
  `WireApi` is the *wire* enum; it is what a provider declares in `supports:`, what the 3×3 matrix indexes
  and what `urls` is keyed by. A `Metrics` variant would let a roster entry write `supports: [metrics]` — a
  configuration-contract contamination for a trace-vocabulary need.
- **Rejected alternative 2 — no record for a metrics refusal.** Cheaper, and it is the one rejected
  alternative worth naming as *defensible*: it narrows §4.7's "**one record**" row (`docs/spec.md:731`) from
  a universal to "a protocol request", and it makes an unauthenticated scan of this port invisible in the
  trace — an observability hole in a repository that treats those as defects (R53-1-F2's class). It also
  costs the same seam change, so it buys nothing.

### 3.9 What may **not** be exported

The list is part of the contract, and it is the answer to "what stops this surface from becoming an
observation channel?":

- **No client bytes and no message content**: not the request body, not a message, not a tool schema, not a
  prompt, not a `model` string, not a hash of any of them. The formatter receives no request input at all
  (§4), and the case proves the negative empirically by sending a canary (§5 limb 3).
- **No key material**: never a token value, never an `api_key` value, never even the *name* of the variable
  holding one (`server.auth_token_env`) — §9.1's `auth` member owns that single fact
  (`docs/spec.md:2208-2224`), and one fact lives on one surface.
- **Nothing per-request and nothing keyed by traffic**: no `request_id`, no `session`, no
  `prompt_cache_key`, no `thread_id`, no `turn_index`, no `provider`, no `model`, no `requested_model`, no
  `client`, no upstream URL. The `kind` label is **not** in this class: its value is drawn from §8's closed
  vocabulary (`docs/spec.md:2039-2053`), bounded by the code, never by traffic.
- **Session identity, in particular**: §6 documents `session` on the trace, and the trace is where it stays.
  A per-session series would be unbounded cardinality *and* would ship a conversation identifier to whatever
  scrapes this port — the reverse of ADR-005's boundary.
- **The per-provider / per-model cost split — a rejected widening, named so nobody adds it "helpfully".**
  It is the most tempting figure this surface could carry and it is forbidden for a hard reason: §9.2's
  derivation does **not** produce it (`crates/router-cli/src/stats.rs:34-37` group by currency only), so
  exposing it would be **a second, parallel derivation** — the defect §4 exists to prevent. Trigger for
  revisiting: *a round that first puts the split into §9.2's provenance table* (one owner, two readers),
  which is a reporting-contract change with its own case.
- **No config echo**: no paths (`root_path`, `roster_path`, `trace_dir`), no digests, no prices, no quotas,
  no plugin list — §9.1 is the surface that reports the loaded configuration.
- **No inferred figure presented as measured**: every series that carries a §7 label carries it as the
  `provenance` label, machine-readable (§3.4).

### 3.10 The one figure that is **not** here, and why

§9.2's `unknown outcome requests` (the only figure that lives in the **event log** rather than in the trace)
is **not exposed by this surface** — **no series of any name carries it**, which is why no metric name is
proposed for it here (the comment line in §3.5 names the *figure*, not a series) — and the omission is a
frozen constant of the response (§3.5's first row), not a runtime arm:

- `Query::AllEvents` is documented in the store contract as **bounded use**: *"The full event log in
  `event_id` order (bounded use: conformance and rebuild; **the serving path never scans the log**)"*
  (`crates/router-core/src/store.rs:303-305`). A per-scrape full-log read from inside the serving process is
  exactly what that sentence forbids, and at a 15s scrape interval it would be the most expensive thing the
  process does.
- Adding a windowed event query (a new `Query` variant + an index) is a **store-contract change** the
  owner's sentence does not authorise, and it would put a log scan on the serving path.
- So the figure stays where §9.2 puts it: `router stats` (an offline command, which may scan the log) and
  the trace. The exposition names the omission in-band every time, so a consumer cannot read its absence as
  a zero.
- Trigger for revisiting: *a round that owns the store contract and can show a windowed read that does not
  scan the log* (or an operator asking for it after a `book/` note exists). Registered as `R50-0-F2` (§8).

---

## 4. The single-owner rule

**Every value this surface renders is a value the existing reporting derivation already produced. The
surface may format; it may not compute a figure.**

| layer | the owner | at HEAD |
|---|---|---|
| the window's records | `router_cli::stats::read_window_records(dir, start_ms, now_ms) -> Result<(Vec<Value>, usize), String>` | `crates/router-cli/src/stats.rs:409-454` |
| the figures | `router_cli::stats::aggregate(records: &[Value]) -> TraceFigures` | `crates/router-cli/src/stats.rs:77` |
| the quantiles | `stats::median` (p50) and `stats::p99` | `crates/router-cli/src/stats.rs:488-509` |
| the ratios | `stats::cache_hit_rate(&TraceFigures) -> Option<f64>` and `stats::stateful_inbound_rate(&TraceFigures) -> Option<f64>` — **hoisted** from the two inline copies in `print_text` and `report_json` | inline today: `crates/router-cli/src/stats.rs:756-758` (`--json`), and the printer's own division in `print_text` (`:545`) |
| the rendering of the figures for §9.2's readers | `stats::print_text` (stdout) and `stats::report_json` (`--json`, exposed for `CONF-56`) | `crates/router-cli/src/stats.rs:545`, `:726` |

**The required extraction list, closed** (this is the whole of the refactor the implementation card owes;
each item is mechanical and changes no value):

1. `read_window_records` → `pub(crate)`.
2. `cache_hit_rate` and `stateful_inbound_rate` → two named `pub(crate)` functions over `&TraceFigures`,
   called by `print_text`, `report_json` **and** the metrics formatter, so the same ratio is divided in one
   place (today it is divided in two, and the metrics path would have made it three).
3. That is all. Nothing in `aggregate`, in the cost grouping, in the quantiles or in the omission logic
   moves.

**The rule is enforced structurally, not by review.** The formatter's signature is frozen here and admits
**no records**:

```rust
// crates/router-cli/src/metrics.rs
pub const WINDOW_MS: i64 = 900_000;                       // §3.3
pub fn exposition(
    figures: &crate::stats::TraceFigures,                 // the derivation's output, nothing else
    files_read: usize,                                    // the read's own count (§3.4 #3)
    plan_family: Option<&str>,                            // the loaded policy's family, or none
    read_error: Option<&str>,                             // Some(reason) when the read itself failed
) -> String;
```

A formatter that cannot see a record cannot derive a second value from one; the case asserts the pure
rendering directly (`CONF-41`/`CONF-56`'s exposed-builder precedent) *and* the served bytes
(`CONF-87`). Two further requirements ride with it: **the metrics path may not read the store** (it holds
no `Query`, and §3.10 removes the only figure that would need one — `AppState.store` is not touched), and
**it may not read anything under `autowork/`** (§6).

**A shared-read change the live surface forces — the torn tail.** `read_window_records` today **errors** on
any line that is not a parseable trace record (`crates/router-cli/src/stats.rs:439-440`, *"not a trace
record"*). That is correct for `router stats` (an offline reader) and **wrong for this surface**: the
serving process appends to the file the scrape is reading (`crates/router-store/src/trace_sink.rs:73-106`
writes one `write_all` + `flush` under a mutex; a large record, or a reader between the write and the
flush, can be observed mid-line), so a scrape would intermittently find the whole window "unreadable" —
the one behaviour that would make this surface useless.

- **The rule**: the window read **tolerates a torn tail** — the *final* line of the *newest* file, when it
  is not valid JSON, is skipped and never fabricated; every other malformed line stays an error, exactly as
  today.
- **It lives in the shared read**, not in the metrics path — one owner for a window, both callers — so
  `router stats` gains the same tolerance for the same input class.
- **The change of behaviour is named**: for exactly one input class (a torn tail) `router stats` goes from
  "error, exit 2" to "skip the partial record". No conformance case asserts the old behaviour (measured:
  the string `not a trace record` appears in `crates/` only, in `stats.rs:440`; no case fixture in
  `tests/conformance/tests/**` feeds a malformed line to the reader — `anchors.txt`, section
  "torn-tail sweep"), and §9.2's exit-code list (`docs/spec.md:2309-2311`) names no malformed-line case.
- **A named behaviour, kept in one place**: skipping a partial record is not "an absent measurement read as
  0" — nothing is counted for it, and the *record* does not exist yet. `router_metrics_omitted_figures`
  does **not** count it (it is not a figure arm); the next scrape sees the completed line.

---

## 5. The invariants, each with its case

| # | invariant | case | the limb |
|---|---|---|---|
| 1 | **The answer is never §8's error body.** The admitted arm's media type is the exposition's, its body carries no JSON `error` member, and it carries **no** `X-Router-Request-Id` (the §8 formatter's always-on header, `docs/spec.md:2055-2057`). | `CONF-46` | the same two negative layers the old case carried, re-pointed at `200` |
| 2 | **The status set is closed**: admitted → `200`; refused → `401` (§8's body, header present per §4.7); **anything else — `404`, `501`, `500`, `503` — is a defect**. The old case's exclusion of `501` becomes this. | `CONF-46` | status assertions + the `error.type == "unauthorized"` body read |
| 3 | **No request byte is read and nothing of the request is carried.** A `GET /metrics` carrying a canary body answers **byte-identically** to the body-less one, and the canary appears in no response byte, no trace file and no store row. | `CONF-46` | the canary limb (CONF-70's precedent) + the byte-equality limb |
| 4 | **An admitted scrape writes nothing.** The trace dir's bytes and the store's event count are unchanged by the admitted arms; the *refused* arms each add exactly **one** record (the guard's, `protocol_in: "metrics"`, `errors[].kind: "unauthorized"`). | `CONF-46` | byte-compare of the trace dir before/after + a record count/kind read |
| 5 | **The guard applies.** With `server.auth_token_env` set: no token → `401`; `Authorization: Bearer` → `200`; `x-api-key` → `200` **byte-equal** to the Bearer arm. With no key configured: `200` with no token (§4.7's key-absent control, `CONF-45` ⑤'s shape). | `CONF-46` | four arms, one rig + one keyless rig |
| 6 | **The figures are the derivation's.** Every value in the served exposition equals (a) the case's **own** independently computed sum/count/quantile/ratio over its own fixture records (the `CONF-41` method) and (b) the value `stats::report_json` renders for the same window (the `CONF-56` method). No series exists that the derivation does not produce. | **`CONF-87`** | the equality table, figure by figure |
| 7 | **The series set is a function of the config, not of traffic.** Two rigs, N and 10N records in the same window: identical metric-name+label sets and identical line counts (only digits differ). The body stays under 8 KiB. | `CONF-87` | the N/10N pair |
| 8 | **The read is bounded and sourced correctly.** `router_trace_files_read` equals the number of §4.1 rollover files the window actually intersects (`≤ 2`), and a **decoy** trace dir holding records outside the config's `trace.dir` contributes nothing. | `CONF-87` | the file count + the decoy dir |
| 9 | **Determinism.** Two admitted scrapes with no intervening traffic are **byte-identical** (no timestamp, no uptime, no counter — §3.2). | `CONF-87` | two sequential GETs, `cmp` |
| 10 | **Zero vs absent, and the partial-report rule.** A window with records but no input tokens: the token series are present as `0`, the ratio series is **absent**, and its comment names why. A window whose dir is removed after boot: `200`, every trace-derived figure absent, each with its comment, `router_metrics_omitted_figures ≥ 1`, and **never** a §8 body. | `CONF-87` | the two shapes + the comment substrings |
| 11 | **The formatter cannot see a record** — the single-owner rule's structural half: `metrics::exposition` is called directly with a hand-built `TraceFigures` and renders exactly the series the values imply (so a figure computed from records is not expressible). | `CONF-87` | the direct call, `CONF-41`/`CONF-56`'s precedent |
| 12 | **The `provenance` label is §9.2's own**, per series, and never re-labelled: an `inferred` figure is not emitted as `verified` (AGENTS constraint 4). | `CONF-87` | the label assertion per series |
| 13 | **`CONF-46`'s control survives**: the `/health` 200 liveness control on the same run, unchanged from the old case. | `CONF-46` | kept verbatim |

**How the amended `CONF-46` still protects what it protected** (the question its auditor will ask): the old
case's subject was *"a route that a client can reach, that the process does not serve, and that must not be
mistaken for a served one"*. Limb for limb: the surface is **not unbounded** (limb 7: the series set is a
config function; limb 8: ≤ 2 files, < 8 KiB), it is **not unauthenticated** (limb 5 — the opposite of the
old exclusion, and §3.7's decision), it is **not error-bodied** (limbs 1 and 10), and it is **not a
request-shaped path** (limbs 3 and 4: it reads no byte, writes no record). What changed is the *verdict* on
one of the four: the surface is now served, and the owner said so.

---

## 6. The observation boundary (AGENTS constraint 3)

- **What the surface reads**: `serve`'s resolved `trace.dir` — the directory `TraceSink::open` took at
  startup (`crates/router-cli/src/lib.rs:401-408`, exit `4` when it cannot be opened) and which
  `AppState.trace_dir` carries (`crates/router-proxy/src/health.rs:20-22`). `trace.dir` is in ADR-040 D5's
  **refused set** (a reload may not move it, spec §4.15's `:1507-1512`), so the surface needs no revision
  coupling and cannot be pointed at a second directory by a config change. **Nothing else is read**: not the
  store (§3.10), not the config beyond the plan family, not any path under `autowork/`.
- **The direction that must not be opened**: the exposition is a **derived view for operators**. It is not a
  product → autowork channel (ADR-005: the trace JSONL is that channel and only that), it may not be read by
  the harness as evidence, and no gate, corpus or acceptance column may cite it — the gates are the frozen
  measurement (constraint 9 / ADR-012) and this surface is *downstream* of the same records the gates read.
  A future round that wants to consume it must say which measurement it replaces, not merely that it is
  available.
- **The reverse direction is equally closed**: the serving path reads no `autowork/` file, and this surface
  adds no exception to that. A `grep` of the metrics path for `autowork` must stay 0 (its read site is the
  config's own directory); the DESIGN landing states it (§12.21) so the next reader sees the rule where the
  code lives.

---

## 7. Alternatives considered, trade-offs, reversibility

| Decision | Alternatives | Gain / sacrifice | Reversible? |
|---|---|---|---|
| **Guarded route** (§3.7) | unguarded `/health`-style; a `?token=` path; loopback-only | Gain: §4.7's written "nothing else is exempt" stays true, and spend data inherits the operator's access control. Sacrifice: one line of scraper config. | Yes (both ways), the unguarded direction being the one that would need the owner |
| **The refusal spends the word `"metrics"`** (§3.8) | a `WireApi` variant; no record; `"other"`/a NULL | Gain: the refusal trail stays complete and the record's own field keeps naming *the endpoint's protocol* (§6's `:1869`). Sacrifice: one vocabulary word and one `&str` seam. | Yes, and the seam is one signature |
| **All gauges, no `_total`** (§3.6) | process-lifetime counters (a second derivation); `_total` names over window values | Gain: no silent rate() lie, no second source of truth. Sacrifice: a consumer cannot `rate()` these; window arithmetic is the consumer's. | Yes (adding a real counter later is a new ADR) |
| **A frozen 900s window** (§3.3) | a config key; a query parameter; a longer/shorter constant | Gain: no config-contract change, no unbounded knob, a scrapable statement of scope. Sacrifice: not tunable per deployment. | Yes (the widening is registered with its trigger) |
| **§9.2's label becomes the `provenance` label** (§3.4) | no label (HELP text only); a `verdict` label name; nothing | Gain: constraint 4 is machine-enforceable and a gate-grade filter exists in PromQL. Sacrifice: one extra label per series (cardinality +4). | Yes |
| **No `tier="total"` series** (§3.4) | a total series beside the four tiers | Gain: `sum()` cannot double-count. Sacrifice: one `sum()` in a query, and a reader comparing against §9.2's total must add. | Yes (but it would be a footgun to add) |
| **The unknown-outcome figure is omitted, always** (§3.10) | a windowed store query (store-contract change + a log scan on the serving path); exposing it from a cached value (a second derivation) | Gain: the surface obeys `store.rs:303-305`'s own rule and stays cheap. Sacrifice: one §9.2 figure is not on this surface (named in-band, never a 0). | Yes, with a store-contract change |
| **Text exposition 0.0.4** (§3.2) | OpenMetrics 1.0 | Gain: universal consumer support. Sacrifice: no exemplars/`# EOF`. | Yes (a media-type change, both formats are scraper-side negotiable only by the server) |
| **The surface's home: `router-cli::metrics`** (§4) | `router-proxy` beside `health_json` | Gain: the derivation it must call lives in `router-cli` (`stats.rs`), and the dependency direction (`router-cli` → `router-proxy`, DESIGN §2 `:31-53`) forbids the reverse. Sacrifice: one more module in the crate that is already the assembly's crate. | Yes |
| **The id: `CONF-87`** (§2.3) | `CONF-86` (taken by a file, no row); `48–51` (reserved with no obligation) | Gain: the id is above the tree's maximum and above the register's own (stale) claim. Sacrifice: none. | No — ids are spent, never reused |

---

## 8. Consequences, register, and what is still open

**Consequences that follow from this contract** (each one a thing a reader should not have to discover):

1. `router stats` gains a **torn-tail tolerance** (§4) — one input class, a behaviour *widening*, no
   assertion touched.
2. A refused metrics scrape is **counted** by `router stats` (`failed`, split as `unauthorized`), so a scan
   of the metrics port is visible in the operator's own report.
3. `conf_46` is **renamed**, and its id now describes a served surface; the id is unchanged.
4. `CONF-87` is spent, and `DESIGN` §12.8's heading range moves to `CONF-01…CONF-87` because a row for 87
   exists and the heading otherwise lies about its own table (the R10 precedent for that header).
5. `/metrics` becomes the **fifth** route — the first one that is neither a protocol endpoint nor a liveness
   probe, which is why §3.7 and §3.8 are the two decisions this ADR spends the most words on.

**The register this card opens** (findings and notes, none blocking; owners named):

| id | item | owner | due |
|---|---|---|---|
| `R50-0-F1` | the `R49-2-F1`/`R49-2-F6` class: the over-general universal *"one decision record per request"* still stands at `docs/spec.md:1694` (§6's heading) and `:2436` (§9.3's close), and `autowork/harness/r49-0/CLAIM-SOURCES.md` rows C3/C40 carry its wording — **not repaired here** (§1.2), because a partial narrowing is the same defect class and the atomic edit needs one card that owns all four carriers (the row file is outside every R50 write set). Recommended home: one contract-wording card. | architect (the contract's next owner) | open |
| `R50-0-F2` | the unknown-outcome figure is absent from `/metrics` by design (§3.10); the widening (a windowed, non-scanning store read) needs a store-contract owner | backend-coder + the store contract's owner | open — with a trigger |
| `R50-0-F3` | the metrics window is a constant (§3.3); a configurable window is the registered widening, with its own key, read site, reload answer and case | the next round that owns `config.example.yaml` | open — with a trigger |
| `R50-0-F4` | the `conf_46` **rename** moves a case file name. Re-measured at HEAD with `git grep --untracked -n`, because the sentence this row first carried (`grep -rn 'conf_46_metrics_is_bare_404' .`, "finds `DESIGN §12.8`'s row only") is **false and was not a measurement**: a recursive filesystem grep over this repo also walks `target/`, and what it returns is not the tree. The real set is **74 files / 85 lines outside this card's own evidence** (the command in `measurements.txt` carries the `':!autowork/harness/r50-0'` exclusion, because the receipt that prints this number is a file in that directory and a count taken while it is being written is a self-reference rather than a measurement; the excluded half is measured on its own below). It splits: **must move** — `design/DESIGN.md:1005`, §12.8's `CONF-46` row, which §2.4 rewrites to carry both names, and *nothing else* (R50-1's rename owes no other document an edit). **Must not move**, each because it quotes or seals an event in which the file really carried that name: `.github/workflows/ci.yml:65` — a **live** file: the free-disk comment quotes the targets run 36299293578 died on, and rewriting the quotation would make the workflow assert an event that did not happen (`ci.yml` is R53's file, outside every R50 write set); this ADR's own lines (a record that authorises a rename must name both names); R49's fourteen, including the two audit rigs that `grep` the case **by path** (`r49-0/anchors.sh:67,250,252`, `r49-0b/my-anchors.sh:67,250,252`) and `r49-0/CLAIM-SOURCES.md:160,359`; three round records; fifty-four sealed `gates*.log`/`*.out` receipts under `autowork/harness/r2*–r53*`; and this card's **9** evidence files, which the same command reports separately (`git grep --untracked -l … -- autowork/harness/r50-0` → `9`) because they seal the pre-rename state and must outlive the rename as they are. What is **not** in the set, and is the part that would have bitten: no `book/` page, no manifest (`tests/conformance/Cargo.toml` names no case) and no live harness entry point | R50-1 — nothing owed beyond the rename itself (one re-scoped verdict, no edit) | due R50-1 |
| `R50-0-F5` | **`DESIGN §12.11` contradicts `router-cli/src/lib.rs:579-583` on the key-absent case** — the section still reads *"When `server.auth_token_env` is absent, **no layer is installed** and the assembled router is byte-for-byte the assembly v0.1 had before this key existed"*, while the landed wiring (R47-2) installs the layer **always** and gives the key-absent revision a gate that admits everything, precisely so the two revisions have one code path. `R50-0` noticed it while adding this round's bullet directly above that sentence and **did not edit it** (the sentence is not this card's subject and the fix is a sentence in a file another round owns); the evidence is `crates/router-cli/src/lib.rs:579-583` against `design/DESIGN.md` §12.11's bullet list | the next round that owns `design/DESIGN.md` §12.11 (the auth landing; `R47-2`/`R48`'s truth-repair class) | next §12.11-touching card |
| `R50-0-N1` | `AGENTS.md` constraint 2's discipline is what forbids the timestamp/uptime in the body (§3.2); a future "add `router_uptime_seconds`" idea must be recorded as a *new* contract, not a convenience | — (note) | n/a |
| `R50-0-N2` | cost: this card is docs-only, offline, `$0.00` — no provider dialled, no credential read | — (note) | n/a |

**What is left undecided, and by whom**: nothing in this surface's contract is left to a loop's judgement;
what remains outside it is (a) the four items above, and (b) everything §1.3 lists as *not licensed* —
each of which is the owner's, on its own card, with its own ADR.

---

## 9. The README sites the round's writer card changes

Written down here, **not edited** by this card (`README.md` is the writer's, and this card's write set
excludes it). The writer's touch, four edits, all in `README.md`:

1. **`README.md:279-287`'s `## API` table** — a served endpoint must appear as a row beside `/health`:
   `| GET | `/metrics` | the trailing-window figures `router stats` reports, in the Prometheus text format
   (spec §4.16) |`. Anchors: `docs/spec.md` §4.16 (new), §9.2 (`:2285`, the figures' provenance), and the
   guard row below.
2. **`README.md:286`** — the sentence *"`GET /metrics` (Prometheus) is not registered in v0.1: the route
   answers a bare `404`."* is **false** after R50-1 and must go. Its replacement states the three facts a
   reader needs: it **is** served; it answers **§9.2's own figures** over a frozen 900-second window; and it
   sits **behind the same token guard** as the protocol endpoints (the `/health` exemption is `/health`'s
   alone).
3. **`R49-2-F1` — `README.md:272`, *"one decision record per request"*** (`progress/2026-09-27_12-14-42_R49-readme.md:226`).
   Class: an over-general universal whose domain is "the requests the gateway served", while the file
   carves out the abandoned-mid-stream class at `:341-344`. Proposed clause (the register's own):
   *"one decision record per **completed** request"*. **Coupling the writer must know**: that sentence is
   claim row **C40**'s own wording (`autowork/harness/r49-0/CLAIM-SOURCES.md:204`, anchored at
   `docs/spec.md:2436`), so narrowing it in the README alone **drifts a claim row** — the register's own
   "two owners" caveat. The row and the spec sentence are outside every R50 write set (§1.2/`R50-0-F1`), so
   this card's ruled form is: **write the narrowed clause and register the drift**, or leave the sentence and
   say so — the writer may not silently diverge from a claim-row sentence.
4. **`R49-2-F2` — `README.md:395`, *"The trace is the authoritative record of what each request cost"*** —
   same class; no claim row carries it, so the fix drifts nothing. Proposed: *"…of what each request **was
   measured to** cost"* (the register's own wording).

Two further README facts the writer should not have to re-derive: `README.md:5`'s headline was fixed by
`R49-1c` (`d2641ad`) and is **already** the narrowed form (*"…for every request it **completes**"*), and
`README.md:395`'s §Ops bullet (*"`docs/spec.md` §9.3 lists the reporting surfaces that are not served
yet"* — the bullet that also carries `R49-2-F2`'s sentence, one line below the §Ops heading at `:389`)
stays **true** — `router replay` and `router trace tail` are still not served, so only its wording,
not its truth, is at issue.

---

## 10. What this ADR does not do

It does not write the handler, the route registration, the seam widening, the extraction or either case
file; it does not touch `README.md`, `book/`, `AGENTS.md`, `crates/**`, `tests/**` or
`config.example.yaml`; it changes no price, no key, no gate, no corpus and no other case's assertion. The
implementation of everything frozen here is `R50-1` (backend-coder), its independent verification `R50-1b`
(the `CONF-46` red → green contrast is its control), the README `R50-1w`, and the audit of this contract
`R50-0b`.

---

## 11. Post-audit repairs (R50-0b, 2026-09-27) — pointers, not decisions

`R50-0b` audited this contract against `8b0fd21` and returned **changes requested**: every substantive check
it ran passed (the four gates and `CONF-46` reproduced independently; the 21 series identical in spec §4.16,
this ADR and DESIGN §12.21; the single-owner rule structurally enforceable; the amended `CONF-46` conserving
each protection it had; the guarded-route decision sound; the authority chain quoted correctly) and **four
defects sat in the text's own pointers**. All four are repaired here, in the card that owns the text:

| R50-0b | the defect | the repair |
|---|---|---|
| 1 | `design/DESIGN.md:2798`'s §12.11 bullet cited `docs/spec.md:1757` for *the endpoint's own protocol*; the phrase occurs at `:1869`. The pair was stale in one carrier — this card's remap fixed it in this ADR and missed the DESIGN bullet | `1757` → `1869` in DESIGN. Asserted by tier B of `citecheck.py` |
| 2 | §9's second note anchored the §Ops bullet at `README.md:389-393`; the sentence is at `README.md:395` (the bullet `R49-2-F2` also lives in). The writer card would have hunted three lines early | anchored at `:395`, with the §Ops heading's own line (`:389`) named so the reader can see both |
| 3 | §1.3's *any other route* bullet named `grep -c 'route('` as the measurement of "4 routes at HEAD"; that command reads **8** — it counts `fn` definitions, `.route(` builders and `route_layer`, not routes. The number was right, the command was not a measurement of it | the bullet now anchors the assembly (`crates/router-cli/src/lib.rs:891-910`, `/health` `:584-591`, three `.merge(guarded_protocol_route(…))` sites `:892`/`:898`/`:904`) and names a pattern that reads **4**, with the old pattern's **8** stated beside it |
| 4 | §8's `R50-0-F4` asserted that `grep -rn 'conf_46_metrics_is_bare_404' .` "finds `DESIGN §12.8`'s row only". The command never terminates over `target/`, and the real carrier set is **74 files / 85 lines outside this card's own evidence** — including one **live** file the sentence did not mention, `.github/workflows/ci.yml:65`, which quotes the failing targets of CI run 36299293578 | the row is re-measured with `git grep --untracked -n` and split into what must move (one row) and what must **not** (a quotation of an observed run, sealed receipts, a past round's tables), with the failed command named as failed |

**Two more of the same class were found by the new assertion rig**, not by the audit, and are repaired here
too. §3.3 attributed the rule *"a report must state the window it covers"* to `docs/spec.md:2297-2299`, where
the spec's words are not that quote — a compression in quotation marks rather than a citation, so the section
now carries the spec's own sentence verbatim. And §3.8 named `docs/spec.md:2372` as the `succeeded`/`failed`
row when it is `:2371`; `:2372` is the `usage missing` row.

**Why an assertion rig, and not a better sentence.** `anchors.sh` — this card's original evidence, the tool
R50-0b used — *prints* the line a claim points at. A wrong number therefore prints happily, which is exactly
how finding 1 survived a full audit inside a bullet that was itself printed. `citecheck.py` inverts the
direction: it reads the documents' text and **fails** when the file, the range or the phrase bound to a
citation is not there. Its red control is at the pre-fix commit — 4 failures, the two anchor-class ones plus
these two — and it is green here (`citecheck.txt`: 110 tier-A, 7 tier-B, 0 failures). What it **cannot** do
is stated with it, because a checker's blind spots are part of its evidence: a range that exists but is drawn
too *wide* passes tier A and is caught only where a phrase is bound to it (tier B, 7 of 110 citations here);
a claim shaped like a command whose command does not reproduce it is not a citation at all and is
`measurements.sh`'s job; and a phrase split across a line break is bound as it reads in the joined paragraph,
so a claim that depends on the break is a human's to read. Nothing in this section changes a decision, moves a
section, or touches the frozen surface: the assertions in §2 stay what they are, `CONF-46` stays green, and
R50-1's work is unchanged.

