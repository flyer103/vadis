# ADR-051 — a declared cell is a native cell: v1 serves exactly the cells an entry declares, `wire_api` names
the diagonal and gates nothing, and every commitment point re-enters the family guard

- Status: accepted
- Date: 2026-10-06 (round **R69**'s contract card, `R69-0`)
- Kind: **docs-only in this card.** It adds one ADR, amends `docs/spec.md` (§2, §4, §4.2, §6, §8) and
  `design/DESIGN.md` (§12.8, §12.9, §12.10.9), and parks five conformance cases (`CONF-100…104`), each
  enumerated in §2.7 and each `#[ignore]`d behind `R69-1`. It changes **no product byte** here — `crates/` is
  untouched by `R69-0` — and it adds no gate, moves no corpus, changes no threshold, mints no
  saving/latency/cost figure and touches no price.
- Authority: **the owner's directions of 2026-10-06**, quoted verbatim in §1.1. The allocation of
  `CONF-100…104` is that act, not a loop outcome (AGENTS constraint 9 / ADR-012).
- Related: AGENTS hard constraints **1** (the byte boundary — the vadis may not re-encode the client's body,
  so "translate it" was never an option on the serving path), **2** (content determinism), **5** (no
  fabricated prices — the reason §1.3's roster rule exists), **8** (docs before code), **9** (the measurement
  is not part of the search space: no gate, corpus or existing assertion moves here beyond the two named in
  §5.3); **ADR-004** (passthrough vs safe translation) and **ADR-019** (the content-edit contract) — the two
  ADRs that own what translation would have to be; **ADR-011** (the upstream-error taxonomy and the failover
  action set); **ADR-014** (plan-first routing — whose *session granularity* (item 1), *session-boundary
  probe* (items 2–3) and *guard rule order* (item 9) this ADR preserves untouched); **ADR-015** (the two
  permitted byte mutations); **ADR-020** (`urls`: one complete URL per declared cell, used verbatim);
  **ADR-022** (the candidate walk's per-wire eligibility — **partially superseded here**, scope in §2.3);
  **ADR-023 / ADR-024** (the walk's two refusal conditions, `skipped[]` and its order — unchanged);
  **ADR-040** (the reload); **ADR-049** (plan-first fan-out: the pool, the family list, the drained plan
  tier); spec §2, §3, §4, §4.0, §4.2, §4.6, §4.6.1, §4.8, §4.9, §4.11, §6, §8; DESIGN §12.8, §12.9,
  §12.10.9.
- Cases: `tests/conformance/tests/conf_100_declared_cell_is_native.rs`,
  `conf_101_walk_across_declared_cells.rs`, `conf_102_fallback_reenters_the_family_guard.rs`,
  `conf_103_roster_correction_serves_three_lines.rs`, `conf_104_no_available_route_still_holds.rs`.
  No existing ID is spent, reused or renumbered, and **`CONF-98`/`CONF-99` are not touched**: they are
  promised by the parked branch `round/67-abandoned-attempt`, which this round never reads, switches, merges
  or deletes.
- **Line-number convention.** Every `path:line` below resolves at **this branch's HEAD** (the commit that
  carries this ADR). `R69-1`'s edits shift the numbers in the files it touches; the deltas are stated where
  they matter rather than left to be discovered.

---

## 1. Why

### 1.1 The owner's directions, verbatim and dated

**The owner's directions of 2026-10-06**, verbatim, in the order they were given:

> *「解决 'fallback 跳转不会重新进 plan' 问题」*

> *「如何智能解决上述协议问题？目前 zai / kimi / deepseek 三家都支持 responses 协议，vadis 需要能够根据
> agent 的请求自由切换 /v1/chat/completions、/v1/responses、/v1/messages 协议」*

> *「协议修复深度选择：**A — 只做 v1：声明的 cell 就是原生 cell**」* — with B (`wire_api: native` as a second
> declaration layer) and C (cross-protocol content translation) **explicitly refused**.

> *「roster 注册表更正：**两者都要** —— 补齐已证实的 cell + 把 `zai-cn-plan` 的 `wire_api` 翻成
> `responses` + 新增第二个 policy family `glm-5.3-flash`」*

> *「冻结断言改写：**批准** —— 把 CONF-57 的两条臂改写为「未声明 cell」规则，并改写
> `crates/vadis-proxy/src/forward.rs` 的两个单测；ADR 里逐字引用该授权」* (AGENTS constraint 9 / ADR-012:
> the gate side is the owner's).

### 1.2 The two defects, each at a line

**(A) a fallback jump does not re-enter the plan guard.** The family's routing decision is evaluated **once
per request, on the resolved route only**: `family_policy_for_route` at `crates/vadis-proxy/src/forward.rs:816`
(buffered) and `crates/vadis-proxy/src/stream_forward.rs:344` (streaming), then `plan_guard` at
`forward.rs:834` / `stream_forward.rs:360`. The candidate chain is built immediately afterwards
(`forward.rs:1167-1202`: the resolved route, the plan tier's remaining members, the family's metered tier,
then the global `fallback` list) and walked at `forward.rs:1230-…` — and **no candidate of that chain is ever
passed through either call**. The in-loop test at `forward.rs:1248` asks only whether the candidate's provider
speaks the inbound wire. Consequence: a request that reaches a route **inside a family** by *failover* (a
`fallback` entry that carries the family tag, or a re-decision mid-walk) is attempted on that route whatever
the family's state says. The family's own list — plan tier first, then metered, then global `fallback` — is
honoured for the route the client named and ignored for every route the walk chose.

**(B) the three declared cells are not switchable.** `forward.rs:902-911` and `stream_forward.rs:440-453`
answer `501 not_implemented` for any entry whose `wire_api` differs from the inbound protocol, and the walk
skips such a candidate (`forward.rs:1248`, `stream_forward.rs:752`); `url_for` is even called with the
`wire_api` rather than the inbound protocol (`forward.rs:1316`, `stream_forward.rs:770`). So a vendor that
**documents and serves** three cells (each with its own URL — `urls`, ADR-020) is reachable on exactly one of
them, and the shipped roster already carries the three URLs for eight of its nine entries. The client's
protocol choice — which is the agent's own choice, `codex` speaking responses, `claude code` speaking
anthropic, a plain SDK speaking chat — is therefore not honoured: it is answered `501` for the route the
client named and skipped for every candidate.

The owner's measurement of the vendors' actual surfaces (four live probes, 2026-10-06: `zai-cn-plan` →
`https://open.bigmodel.cn/api/v1/responses` `200`; `kimi-cn` → `https://api.moonshot.cn/anthropic/v1/messages`
`200`; `kimi-cn-plan` → `https://api.kimi.com/coding/v1/responses` `200`; `deepseek` →
`https://api.deepseek.com/anthropic/v1/messages` `200`) is recorded on the round's card as **corroboration**.
It is not, and may not become, the source of a config value: §2.5's cells are backed by vendor pages, per
AGENTS constraint 5.

### 1.3 What this round does not do, and why the refusal is the decision

- **B — a second declaration layer (`wire_api: native`).** Declined. It would answer "which cells exist?"
  twice (once in `supports`, once in a new field) and leave two places to drift, which is the ambiguity
  ADR-020, §4.9 and ADR-049 §4 all declined for the same reason. There is exactly one declaration of a
  cell's existence in v1 and it is `supports`.
- **C — cross-protocol content translation.** Declined *as scope, not as value*. A translator is a lossy,
  determinism-critical content step with its own contract (ADR-004), its own lossy register (spec §2's list)
  and its own conformance cases — six of which (`CONF-04…09`) are `#[ignore]`d and unwitnessed today. The
  byte boundary (AGENTS constraint 1) forbids *pretending* one ran, which is exactly what ADR-022 already
  froze against. v1 serves a cell the vendor documents; it never re-encodes.

---

## 2. Decision

### 2.1 (a) The protocol rule (v1): one binary rule over the declared cell set

**Outbound selection.** For the route a request resolves to, and for every candidate the walk later
considers:

> **inbound protocol ∈ that entry's `supports` ⇒ native passthrough.** The client's own bytes are POSTed to
> `urls[inbound]` — the complete URL ADR-020 forbids composing — and the body carries only §2's two
> byte-level mutations, (a) and (b). Nothing is translated, re-encoded or re-ordered.
>
> **inbound protocol ∉ that entry's `supports` ⇒ the cell does not exist.** For the route the **client named**:
> `400 capability_unsupported` (today's answer, unchanged). For a **candidate the client did not name**: a
> skip in the keyless class, reason `wire_mismatch` (today's behaviour, unchanged) — never a refusal, never a
> displacement, never an `errors[]` member.

In tabular form, over the two questions spec §2 already separates:

| the route in question | declares the inbound cell | does not declare it |
|---|---|---|
| **the route the client named** | native passthrough → `urls[inbound]`, verbatim | `400 capability_unsupported` (spec §8) |
| **a candidate the walk chose** (the family's routes, the `fallback` list) | native passthrough | **skipped**, reason `wire_mismatch` (the keyless class; ADR-022 decision 2, preserved) |

**`wire_api` changes role, and this is the one row of the 3×3 that moves.** `provider.wire_api` is a
**declaration of the entry's native protocol — the diagonal cell** — and is **no longer a gate on
anything**: not on the resolved route, not on a candidate. It keeps exactly one load-time obligation,
`wire_api ∈ supports` (`crates/vadis-core/src/config.rs:2091`, unchanged), so an entry that declares the
three cells must still name one of them as its native form. Everything a reader once derived from
`wire_api == proto_in` is now derived from `supports ∋ proto_in`.

**`501 not_implemented` has no producer reachable from a request in this build, and this is stated rather
than left as a dead row.** The row stays in spec §8's vocabulary (a client parses `error.type`; the enum,
its string, its status and its `kind_for_code` mapping stay — `crates/vadis-core/src/error.rs:22`, `:41`,
`:60`, `crates/vadis-core/src/trace.rs:403`). What disappears is every path that could reach it: the two
resolved-route branches (`forward.rs:902-911`, `stream_forward.rs:440-453`) are deleted by `R69-1`, because
§2.1 makes a client-named cell either natively served or a `400`. One helper remains and answers nothing:
`vadis_proxy::protocol_stub` (`crates/vadis-proxy/src/stubs.rs:7`, re-exported at `lib.rs:28`, built on
`health.rs:482`) is **registered on no route** — `vadis-cli`'s router mounts only the three protocol routes,
`/health` and `/metrics` (`crates/vadis-cli/src/lib.rs:976-994`) — so it is a shipped function with no
caller, and this ADR names it so that "no producer" is a checked statement rather than an impression.

**And it may not be widened into the walk's answer.** "Some candidate you never named cannot take this
protocol" is **ADR-022 decision 3's `no_available_route`** — `502 upstream_error` with
`details.stage` and `details.skipped[]`. Widening the `501` to that question would make one status code mean
two things, which is the alternative ADR-022 itself rejected in the same words. The two sentences stay two
sentences.

**Consequences for the derived fields.** `protocol.protocol_out == protocol.protocol_in` stops being a
coincidence of one field's value and becomes true **by construction**: a route is attempted only if it
declares the inbound cell, so the wire the bytes went out on *is* the inbound protocol. `translated` stays
`false` and `lossy[]` stays `[]` on every record this build writes (spec §6; ADR-022 decision 5, whose
landing is already in place at `crates/vadis-proxy/src/accounting.rs:556-562`).

### 2.2 The spec's 3×3 table, re-labelled

spec §2's table currently reads "Outbound native condition: provider `wire_api: chat`" per row. Under §2.1 it
reads "the inbound protocol is declared in the entry's `supports`", and a new sentence replaces "only the
native diagonal is served": **every declared cell is a native cell**, each with its own complete URL
(ADR-020), and the walk's discriminant is the declared set. The lossy table (§2) is **kept byte for byte and
labelled**: no cell in this build is produced by translation, so its rows describe a mapper this build does
not have (`R69-1` changes no line of it).

### 2.3 (b) The ADR-022 supersede, named clause by clause

ADR-022 is **not edited** (append-only). This ADR supersedes it **only** as follows, and its own
*Reversibility* clause anticipated exactly this ("widening it later — for example when a mapper exists — is
choosing a different eligibility rule for a candidate whose wire differs, at which point this ADR is
**superseded** (append-only: a new ADR states the new rule and cites this one)"):

| ADR-022 | disposition |
|---|---|
| **Decision 1** (candidate eligibility), the clause `its provider's \`wire_api\` equals the inbound protocol` and the sentence "the rule is stated once, as `wire_api == proto_in`, on both paths" | **superseded** → the rule is `supports ∋ proto_in`, stated once, on both paths. Everything else in decision 1 (provider entry exists; a key/transport is held; the walk-order rules) is **preserved verbatim**. |
| **Decision 2** (a wire-incompatible candidate is *skipped* — never refused — and in the keyless class) | **preserved verbatim.** The class, the absence of an intent row / `error.classified` / `failover.triggered` / `failover_from` / `plan_switch` / `errors[]` member, and the reason, all stand. |
| **Decision 3** (the frozen `no_available_route` shape, its message, and the word list `{unknown_provider, keyless, wire_mismatch, demoted}`) | **preserved verbatim**, including `|skipped[]|` == the offered-candidate count and the chain-order rule. `R69-1` writes **no byte** of it. |
| **Decision 4, first half** (`400 capability_unsupported` for the route the client named) | **preserved**; it is §2.1's second row. |
| **Decision 4, second half** (the `501 not_implemented` for a cell the client named whose wire differs) | **kept as contract text and marked unreachable.** Under §2.1 no legal configuration can trigger it: a client-named cell is either declared (native) or undeclared (`400`). The sentence is not deleted, because deleting a vocabulary row is not this round's act; it is annotated in spec §8. |
| **Decision 5** (the trace tells the truth about both fields) | **preserved**, with one clause re-based: "`protocol.out` keeps naming the resolved route's provider wire — which the pre-flight guarantees equals `protocol.in`" is now true *by construction* (§2.1), not by that guarantee. Its `translated`-is-an-event restatement already landed. |
| **Decision 6** (the refusal is a walk predicate, not a load-time rule) | **preserved verbatim.** |
| ADR-022's *Background*, *Alternatives*, *Rationale* | **preserved** as the record of the decision that was right for the tree it was written on. Its "The reading that decides it" paragraph's fifth sentence ("the rule is stated once, as `wire_api == proto_in`") is the superseded clause. |
| ADR-022's *Consequences* (code) | **preserved in shape, restated in content**: the predicate is still one predicate on the buffered walk plus the streaming chain's construction filter plus the same refusal arms; only its *expression* changes to `supports ∋ proto_in`. `R69-1` performs it. |
| ADR-022's *Honest boundaries* ("the wire gate closes the served path's cross-wire reachability. It does not implement translation…") | **preserved verbatim** — it is exactly as true after this ADR, and it is the boundary this ADR keeps. |
| ADR-022's *Reversibility* | **consumed** by this ADR, by the mechanism that clause prescribes. |

**One word keeps its spelling and changes its meaning, and the change is written out.** `wire_mismatch` no
longer means "this entry's `wire_api` is not the inbound protocol"; it means **"this entry's `supports` does
not declare this inbound protocol"**. The contrast a reader needs is the one against its neighbour in the
same array: `wire_mismatch` = *another inbound protocol would serve this candidate* (the cell does not
exist); `keyless` = *this candidate would serve this exact request if it held a credential*. Two different
remedies, two different words, both still in decision 3's frozen list.

**One spec sentence the walk's own text still owes.** spec §4.2's paragraph "A candidate can serve only on
its own wire" carries the phrase "validation already requires `wire_api ∈ supports`, §4, so the wire
condition is the whole of it" — the superseded clause in prose. §2.1's discriminant replaces it, and the
sentence's three conclusions (a skip is not a refusal; the narration predicate *is* the eligibility
predicate; a skip is not the `501` of a route the client named) all stand.

### 2.4 (c) The fallback re-enters the family guard: one rule, evaluated at every commitment point

**The rule, normative.**

> **Before any candidate is attempted, its route passes `family_policy_for_route` + `plan_guard`.** The
> commitment points are: the route the request resolved to (the client's own `model`, an alias, or a guard
> `Downgrade`); each entry of the candidate chain as the walk reaches it — a `fallback` entry, the family's
> own tier members, or a route reached by a re-decision mid-walk. A candidate that lands **on a family** is
> routed by that family's state, in the existing order (the plan tier → the metered tier → the global
> `fallback`), and the displacement is recorded with `reason` chosen by **direction**, through the existing
> single owner `displacement_reason` (`crates/vadis-proxy/src/forward.rs`) — `primary_recovered`,
> `primary_exhausted`, `plan_exhausted`, `primary_cooling_down`, unchanged in spelling and meaning
> (spec §6 / ADR-049 §5.2). A candidate that lands **outside every family** is attempted as today.

**Its boundaries, each stated so an implementation cannot widen it.**

1. **The guard's input is a projection read, and nothing new is introduced.** `plan_state`, the same
   projection `/health` reads and ADR-010 makes rebuildable; **no new state, no new config key, no new
   event kind**. The walk's own in-request exclusions (a provider already attempted in this request; ADR-011's
   cooldown) are layered **on top of** the guard's answer as additional exclusions: the guard decides *which
   route this family wants*; the walk still decides *whether that route may be attempted now*. If the guard's
   answer fails a walk condition, it is skipped in the class that condition already owns (an already-attempted
   provider is not re-attempted; a cooling one is `demoted`, CONF-42's shape) and the walk continues.
2. **ADR-014 item 3 is untouched: a session is never probed mid-flight.** Only a session boundary
   (`turn_index == 1`) can admit a probe, and a session that already holds a sticky binding is served by that
   binding (ADR-049 §6 rule 1's pin). The guard re-evaluates *route*, not *probe admission*.
3. **It is a pure function of (route, family state, stable config)** — AGENTS constraint 2: the same triple
   yields the same answer, with no turn number, wall clock or RNG anywhere in it.
4. **The route the guard settles on is the single source of truth for `protocol.protocol_out` and
   `cost.currency`.** There is no second truth: the wire the bytes go out on and the unit the record is priced
   in are read from the same settled route, so a record can never name one route's wire and another's money.
5. **It is not a per-request re-ranking.** The guard's answer for a family is a function of the family's
   **state**, and the state is only moved by upstream evidence (ADR-014 item 2 / §4.6 rule 3) — so this rule
   adds no re-decision that a healthy request could notice, and it cannot reintroduce the per-request flip
   ADR-014's Background constraint 1 forbids. `overflow_selection`'s ranking stays resolved once and pinned
   to the session (ADR-049 §6 rule 1).

**What this buys, in the owner's words.** The request that today is stranded on whatever route a `fallback`
jump landed it on is returned to the family's own list: if the family is on its plan, a fallback jump onto
that family's metered route is re-pointed at the plan; if the family has spilled, the jump is served by the
metered route the family is actually on. The walk's order becomes what §4.2 and §4.6 always described it as.

### 2.5 (d) The roster correction contract, and the page behind every row

**What the round corrects.** Three edits to the shipped pair's roster entries — two **added cells** and one
**declarative flip** — plus the shipped root's second policy family (§2.5.3). `providers.example.yaml` and
`config.example.yaml` are **outside `R69-0`'s write set**: the tables below are the contract `R69-1`
executes, and each row's authority is the vendor page named in it, read on the date named in it.

#### 2.5.1 Added cells

| entry | cell added | URL (verbatim) | vendor page | read |
|---|---|---|---|---|
| `kimi-cn-plan` | `responses` | `https://api.kimi.com/coding/v1/responses` | `https://www.kimi.com/code/docs/third-party-tools/codex` — *"Kimi Code 服务端原生支持 OpenAI Responses API（流式/非流式、reasoning、function calling 均可用）"*, configured as `base_url = "https://api.kimi.com/coding/v1"` with `wire_api = "responses"` | 2026-10-06 |
| `kimi-plan` | `responses` | `https://api.kimi.ai/coding/v1/responses` | the same page (the base it names for the overseas deployment), plus `https://www.kimi.com/code/docs/` — its 服务地址/平台对比 tables give the Overseas OpenAI-compatible base as `https://api.kimi.ai/coding/v1` | 2026-10-06 |

Both entries' existing `chat` and `anthropic` cells are **not** re-read by this round and are **not**
touched: they already carry their own `urls` rows with their own citation (`providers.example.yaml`,
`kimi-cn-plan`'s and `kimi-plan`'s `urls` comments, read 2026-09-21), and §4.9's `set(urls) == set(supports)`
obligation is what makes the two new rows mandatory once the cells are declared.

#### 2.5.2 The declarative flip

| entry | change | authority |
|---|---|---|
| `zai-cn-plan` | `wire_api: anthropic` → `wire_api: responses` | `https://docs.bigmodel.cn/cn/coding-plan/quick-start`, its 接入端点说明 table: Anthropic Message 协议 `https://open.bigmodel.cn/api/anthropic`; OpenAI Chat Completion 协议 `https://open.bigmodel.cn/api/coding/paas/v4`; **OpenAI Response 协议 `https://open.bigmodel.cn/api/v1`**. Read 2026-10-06 (the same table the entry's own `urls` comment already cites at 2026-09-21). |

The entry declares all three cells either way, so this is a change of **which cell it names as native**, not
of reachability: under §2.1 every declared cell is a native cell, and `wire_api ∈ supports` still holds
(`responses` ∈ `[anthropic, chat, responses]`). The flip is the operator's own preference, expressed where
§2.1 says it belongs.

#### 2.5.3 The second policy family

The shipped root's `plan_policy:` becomes `plan_policies:` with **two** families — `glm-5.3`
(`primary: zai-plan/glm-5.3`, `overflow: zai/glm-5.3`) and the new `glm-5.3-flash`
(`primary: zai-plan/glm-5.3-flash`, `overflow: zai/glm-5.3-flash`) — because §4.6.1 gives the second family
its own key and the two keys are exactly-one-of (`config.example.yaml`, its `plan_policies:` spelling
described in the comments at `:218-221`). Both families are single-currency (USD, intl), so neither is
refused by §4.6.1's ranking rule; the fleet-wide default stays `overflow_selection: declared`. `R69-1` makes
this edit with the parser it needs — the shipped pair must keep parsing with the shipped parser
(`deny_unknown_fields`), so it lands in the commit that teaches the parser the key.

#### 2.5.4 What is deliberately **not** added

- **`zai` (the metered intl entry) gains no `responses` cell.** No page publishes a Responses endpoint for a
  **metered** z.ai key: the `https://api.z.ai/api/v1` Responses base is published on the **Coding Plan**
  pages (`https://docs.z.ai/devpack/quick-start`, its Endpoint Guide, read 2026-10-06 — "OpenAI Responses
  `https://api.z.ai/api/v1`"), and the metered API reference documents Chat Completions. A cell found only by
  a probe is not added: §4.0's rule is **better missing than guessed**, and an undeclared cell is a `400`, not
  a wrong request. Registered as an open item in §8.
- **No cell is added on the strength of the owner's live probes.** Of the four probes, three hit cells the
  roster already declares (`zai-cn-plan` responses, `kimi-cn` anthropic, `deepseek` anthropic — all present at
  `providers.example.yaml`, declared 2026-09-21/2026-09-22) and the fourth is the `kimi-cn-plan` row of
  §2.5.1, which is added because its **page** says so, not because the probe answered.
- **No `wire_api` is flipped on any other entry.** `zai-plan` and `zai-cn-plan` already declare all three
  cells; `deepseek`, `kimi`, `kimi-cn`, `zai-cn` likewise.

### 2.6 (e) The honest boundary this round must not overstate

`providers.example.yaml`'s `zai-cn-plan` section already records the fact, with its source and date: the
plan's own documentation says it is usable only 「在官方支持的指定工具与产品环境中使用」 and that calls from
self-built applications **do not consume** the allowance — naming ZCode / Claude Code / Codex / Cline /
OpenCode / Roo Code / Kilo Code / Cursor as the supported tools (read 2026-09-21; `providers.example.yaml:830-836`).

> **What this round's declaration states is that a request reached the plan account on a protocol the vendor
> documents — nothing more.** It does **not** state, and must never be read as stating, that the plan's
> allowance is *consumed* in the way an operator expects. Whether a gateway's traffic counts against a
> coding-plan allowance is a **vendor policy** question, and no assertion in this repository can answer it:
> the vadis cannot observe the vendor's meter. Registered as a gap in §8 rather than presented as covered.

This is why §2.5's rows are corrected at all: a cell the vendor documents is a fact about the vendor's
surface, which this repository may record with a citation; "the plan will behave as you hope" is not.

### 2.7 The five parked cases

Allocated by the owner's act of §1.1. Each lands `#[ignore = "CONF-NN: depends on R69-1"]`, compiles, and
carries its rig and assertions in its own file header (CONF-20…25 / CONF-91…97's precedent). None of them
depends on anything this contract does not name — an event kind, a trace field, a `/health` member or a
refusal body.

| ID | Subject | The assertion, in one line |
|---|---|---|
| `CONF-100` | §2.1·a declared cell is a native cell, and an undeclared one is a `400` | one entry declaring `[chat, responses, anthropic]` with `wire_api: responses`, hit by a **chat** request: the upstream sees the client's own chat bytes (modulo the two mutations), `protocol.protocol_out == "chat"`, `translated == false`, and the mock's **path** is that entry's `urls.chat`. Reverse arm: an entry declaring `[chat]` hit by a responses request ⇒ `400 capability_unsupported`. |
| `CONF-101` | §2.1/§2.3·the walk's discriminant is the declared set, not the wire | a chat request whose resolved route is keyless; `fallback` entry #1 declares `[responses]` ⇒ skipped, reason `wire_mismatch` (its meaning: `supports` does not declare this protocol); entry #2 declares `[chat, responses]` ⇒ served, and the mock receives `urls.chat`. |
| `CONF-102` | §2.4·a fallback jump re-enters the family guard | the resolved route is outside every family and answers a retryable failure; the `fallback` entry carries a family tag whose family is on its **plan** tier: the request is served by the **plan** route, `plan_switch.reason == "primary_recovered"` (the direction word the existing owner picks), the family's **metered** mock receives **0** requests, and `cost.currency` is the settled route's. |
| `CONF-103` | §2.5·the corrected roster serves three lines | the three corrected entries are each served on each line they declare (path asserted at the mock); `wire_api ∈ supports` still holds after the flip; no model id repeats within one provider. |
| `CONF-104` | §2.3 decision 3 (ADR-022)·the frozen refusal still holds under the new discriminant | a chain in which **no** candidate declares the inbound protocol ⇒ `502 upstream_error`, `details.stage == "no_available_route"`, one `skipped[]` entry per offered candidate each with a reason from the frozen list, **no** `upstream.submitted` row, `usage_missing: true`, nothing charged. |

---

## 3. Alternatives considered

- **Implement translation (option C) so any cell is reachable from any client.** Rejected as scope: a mapper
  is a lossy, determinism-critical content step (ADR-004, ADR-019) with its own contract, its own lossy
  register and six `#[ignore]`d unwitnessed cells already allocated to it. It also contradicts the position
  the tree is already in: ADR-022 exists precisely to stop the vadis *pretending* one ran.
- **Keep `wire_api` as the gate and ask vendors to declare one cell.** Rejected: it deletes a true
  declaration (the vendors publish and serve all three) to protect a limitation of the software — a config
  that describes vadis instead of the vendor, which ADR-020 rejected in the same words for URLs.
- **Add a second declaration layer (`wire_api: native`) so "the gate" and "the diagonal" can be named
  separately (option B).** Rejected: two declarations of one fact. §2.1's single sentence is checkable;
  a pair of fields that may disagree is the defect class this repository keeps removing.
- **Answer `501` for a candidate a client never named.** Rejected — it is ADR-022's own rejected alternative
  and it is already `no_available_route` (`502`, `stage`, `skipped[]`). One status, one meaning.
- **Let the guard re-decide on *every* request until the family's answer is stable** (i.e. re-evaluate the
  state rather than read it). Rejected: it re-introduces the per-request flip ADR-014's Background
  constraint 1 forbids — the guard reads the state the upstream moved, it does not re-rank per request.
- **Evaluate the guard only on chain construction, not per commitment point.** Rejected: the mid-walk
  re-decision is one of the two ways a request ends up inside a family, and the defect (§1.2 A) is exactly a
  route chosen by the walk. One rule, every commitment point.

## 4. Rationale

- **The config file already contained the answer.** Eight of the nine shipped entries carried three URLs and
  a `supports` list naming them (`providers.example.yaml`, ADR-020's landing); the software served one. The
  change makes the code agree with the file, which is the direction ADR-020 chose when it made `urls` the
  entry's own fact.
- **A declared cell is checkable by a human; a translated one is not.** "This vendor documents this protocol
  at this URL" is a citation (AGENTS constraint 5). "The vadis maps your chat body onto their responses
  endpoint" is a claim about bytes nobody has reviewed — the class ADR-022 was written after measuring once.
- **The walk's order was already the contract.** §4.2 and §4.6 say the family's routes precede the global
  `fallback` and that the family's state decides which one is attempted. §2.4 makes the walk obey the
  sentence it was already written under, and adds no state to do it: the guard is a projection read.
- **Nothing here invents a saving.** No transform, no token delta, no figure: D3 stays unmet and constraint 4
  is untouched.

## 5. Consequences

### 5.1 Docs (this card)

- `docs/spec.md` §2: the selection rule becomes §2.1's declared-cell rule; the two-questions paragraph keeps
  its shape with the discriminant changed to `supports`; the lossy table is kept and labelled as describing a
  mapper this build does not have.
- `docs/spec.md` §4 / §4.2 / §6 / §8: the `supports` comment reads "the cells this provider serves
  **natively**"; the `wire_api` comment reads "declares this entry's native protocol (the diagonal); it gates
  nothing"; §4.2's wire sentence takes the new discriminant; §6's `protocol` derivation is restated as
  *by construction*; §8's `not_implemented` row is annotated "no producer reachable in this build" and the
  `upstream_error` row's wording moves from "not native for the inbound protocol" to "does not declare the
  inbound protocol".
- `docs/spec.md` §4.11: **checked and reported, not changed.** `wire_api` and `supports` are **not** in the
  wizard's askable key set — the `providers` section writes `api_key_env`, or displays a pool
  (`docs/spec.md:1132`) — and no locator rule (§12.14 rule 6: a value inside a flow collection is not
  settable) would reach them: `wire_api` is a scalar on an entry that *is* reachable, and `supports` is a flow
  sequence and is not. Adding either as a row would be a **§4.11 contract change requiring its own witness**,
  which is outside this card's write set; it is registered in §8 instead.
- `design/DESIGN.md` §12.8: the heading moves to `CONF-01…CONF-104`, the five rows land, and the occupancy
  paragraph is repaired in the same commit.
- `design/DESIGN.md` §12.10.9: a paragraph stating the discriminant change and that **both** forwarding paths
  (`forward.rs` buffered, `stream_forward.rs` streaming) must move together, with the sites named.
- `design/DESIGN.md` §12.9: this round's *deliberately not done* items are registered.

### 5.2 Code (`R69-1`'s write set, not this card's)

- the walk's eligibility predicate: `forward.rs:1248` → `supports ∋ proto_in`; the narration predicate
  `forward.rs:2258` the same; the streaming chain's construction filter `stream_forward.rs:752` the same;
- `url_for`'s argument: the inbound protocol, not the entry's `wire_api` (`forward.rs:1316`,
  `stream_forward.rs:770`), and the two `protocol_out` producers (`forward.rs:963`, `stream_forward.rs:505`)
  name the inbound protocol — which the settled route guarantees is a declared cell;
- the two resolved-route `501` branches **deleted** (`forward.rs:902-911`, `stream_forward.rs:440-453`);
- §2.4's guard at every commitment point (the walk's candidate loop, both paths), reusing
  `family_policy_for_route` and `plan_guard` unchanged;
- the roster and the root's `plan_policies:` (§2.5), which land in the commit that teaches the parser the
  keys it writes.

### 5.3 The existing assertions this round authorises to move — the complete set

The owner's direction of §1.1 approves **exactly two** places, and this ADR records the set it checked so
that "exactly" is a measurement rather than a claim (a repo-wide grep of `wire_mismatch` and of `wire_api`
in test code):

| # | assertion | why it moves |
|---|---|---|
| 1 | `tests/conformance/tests/conf_57_wire_compatible_candidates_only.rs`, its two `wire_mismatch` arms (the file's rig declares `supports: [responses]` for the foreign entry — `:122-123`) | rewritten to witness the **undeclared-cell** rule: the discriminating condition becomes `supports ∌ proto_in`, so the rig must let `wire_api` and `supports` **disagree** on at least one entry. Behaviour on the current fixture is unchanged, which is why a rewrite (not a re-numbering) is what makes the case non-vacuous. |
| 2 | `crates/vadis-proxy/src/forward.rs`'s two unit tests, `wire_matching_candidate_still_serves_after_the_gate_skips_foreign_ones` (`:2730`) and `wire_mismatch_candidate_never_serves_and_reports_the_reason` (`:2792`) | same reason: their `provider(name, wire)` fixture sets `supports: [wire]`, so they would pass by coincidence under the new rule. Each gains an arm where the entry declares the inbound cell while naming another `wire_api`. |

**And the assertions this ADR checked that do *not* move**, each because its rig already omits the inbound
protocol from the candidate's `supports`:

- `conf_58_walk_refusal_two_conditions.rs` (its foreign entry declares `[responses]`, `:156`),
  `conf_59_skipped_completeness_per_candidate.rs` (`:96`), `conf_65_skipped_is_the_chains_order.rs`
  (`:95`) — all three assert `wire_mismatch` for an entry that declares neither the inbound protocol nor
  anything else in it, so their reason word, its position in `skipped[]` and the array's order are
  **unchanged**;
- `conf_42_primary_cooling_down.rs`, `conf_44_switch_reason_by_direction.rs`, `conf_71-78`, `conf_82`,
  `conf_91-97` — they read the family's state and the displacement fields, none of which §2.4 alters;
- every case that reads `protocol.protocol_out` — it is `== protocol_in` before and after, by different
  arguments.

Nothing else in `tests/conformance/` or in `crates/`'s test modules may move. If `R69-1` finds a third
site, the finding **escalates** (a new owner decision), it does not silently widen this table.

### 5.4 The register (non-blocking)

| id | item | owner | due |
|---|---|---|---|
| `R69-0-F1` | `zai` (metered intl) has no `responses` cell because no page documents one for a metered key (§2.5.4). If a page appears, adding it is one `urls` row. | owner | open |
| `R69-0-F2` | `providers.yaml` (the operator's **live** roster, outside this tree) carries whichever cells the operator declared; this round's correction lands in the shipped **example** pair that a live file is copied from. Whether the operator's own file needs the same two rows is not observable from here. | owner | open |
| `R69-0-F3` | PLAN-TOS: what a coding-plan allowance does with gateway traffic is vendor policy (§2.6). Registered, not solved. | owner | open |
| `R69-0-F4` | `wire_api`/`supports` are absent from §4.11's askable key set and reachable by no locator rule (§5.1). Adding them is a §4.11 contract change with its own witness. | owner (if wanted) | open |
| `R69-0-N1` | Cost: this card is docs-only, offline, `$0.00` — no provider dialled, no credential read. The vendor pages of §2.5 were read over the public web on 2026-10-06. | — (note) | n/a |

## 6. Honest boundaries and verification owed

- The two defects of §1.2 were localised by reading the tree at this card's HEAD; they were **not** re-measured
  by this card (it runs no request, dials no provider and reads no key). Their fix is witnessed by the five
  parked cases, not by a live client.
- The cells of §2.5.1–§2.5.2 are backed by the vendor pages named there and by nothing else. The owner's four
  probes are corroboration (§1.2). No added cell rests on a probe.
- **One claim in this ADR is a falsifiable assertion and is offered as such:** that `501 not_implemented` has
  no producer reachable from a request after `R69-1`. It is checked today at four sites (`forward.rs:902-911`
  and `stream_forward.rs:440-453`, both to be deleted; `stubs.rs:7` + `health.rs:482`, mounted on no route;
  the enum and its mappings, which are vocabulary). A fifth site would falsify the claim, and `R69-0b` is
  chartered to look for one.
- Likewise falsifiable: that `plan_guard` is evaluated on the resolved route only (§1.2 A). The call sites a
  reader must check are `forward.rs:816`/`:834` and `stream_forward.rs:344`/`:360` — four, and no others.
- Nothing in this ADR is a saving statement, and none may be derived from it: no transform runs, no token
  delta is produced, and the round mints no figure.

## 7. Reversibility

Reversible in both directions, and cheaply for the protocol rule: restoring the old behaviour is re-adding
the `wire_api == proto_in` predicate and the two `501` branches, and re-narrowing `supports` to one cell per
entry (a roster edit). What is **not** reversible is a served cross-wire request — once the client's bytes
were posted to a URL the roster declared, they exist — which is why the rule is stated as a rule and not as a
tolerance. §2.4's guard is reversible by deleting the guard call from the walk's commitment points; restoring
the old behaviour costs nothing that was not already lost (the walk's order is the only thing it changes).
The roster corrections of §2.5 are single-line edits in both directions. §2.1's `wire_api` role is reversible
by re-reading the field as a gate — at which point this ADR is superseded by a new one citing it, exactly as
ADR-022's Reversibility clause provides.
