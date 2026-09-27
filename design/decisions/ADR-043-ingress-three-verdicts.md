# ADR-043 — MCP / A2A / gRPC ingress: three separate verdicts — the semantic facades declined, the h2c transport adopted behind an experiment, the external-adapter story adopted as documentation

- Status: accepted
- Date: 2026-09-27
- Kind: **evaluation**. Nothing is implemented, adopted or added by this ADR: no code, no dependency,
  no config key, no `Cargo.toml` byte, no spec/DESIGN clause and no conformance id lands with it. The
  deliverable is **one verdict per question**, the constraint verdict per direction, the cheapest
  falsifiable experiment each adopted direction would need, and the decisions this round may **not**
  take.
- Decided against: `autowork/survey/2026-09-27_ingress-mcp-a2a-grpc.md` at **`786eb40`** (the survey),
  and its independent citation audit `autowork/harness/r52-0b/REPORT.md` at **`8bff7ca`** (verdict
  `PASS-with-findings`; 43 citations re-walked), both on `round/52-ingress-evaluation`.
- Related: `AGENTS.md` constraints 1 (byte boundary — the client's original bytes and **exactly two**
  permitted mutations), 2 (content determinism), 3 (the observation boundary), 5 (no fabricated
  prices), 7 (English only), 8 (docs before code), 9 (**the measurement is not part of the search
  space**); ADR-004 (native passthrough first; deterministic translation second); ADR-012 (the
  never-mutable paths); ADR-015 (**item 1**: the two mutations; **item 5**: a third is a change to
  `AGENTS.md` constraint 1, "which no round may make on its own"); ADR-017 (the shape this ADR
  follows for an evaluation); spec §1 (`:6`, the v0.1 non-goals), §2 (`:24`, the protocol contract),
  §9.3 (`:2302` and its rule at `:2316-2317`); `book/protocols.md:54-61`; `book/roadmap.md:22`;
  `autowork/STATE.md` §Waiting on human adjudication (rows 1, 17, 18).
- Numbering note (observation, not a decision): the newest landed ADR is `ADR-040` and **`ADR-041`
  and `ADR-042` are unused** (`autowork/harness/r47-0b/EVIDENCE.md:44` records the register as holding
  40 ADRs with `ADR-041` free). This ADR carries the number the round's cards fixed for it; the gap is
  flagged here so no reader infers that two ADRs exist that do not.

## 1. The authority for reopening — stated first

The owner's direction **of 2026-09-27 (item 4 of four)** asks this question explicitly: *should the
router speak MCP, A2A or gRPC?* That direction — relayed to the loop through this round's cards
(`t_1ffcfde6` for the survey, `t_ab048a10` for this ADR) — is the **authority for reopening** the item,
and it is the only authority this ADR relies on for the act of asking.

What the prior artifacts actually said, verified by this card at `8bff7ca` with its own commands (raw
output in `autowork/harness/r52-1/REPORT.md` §5):

| Artifact | What it says about this axis | Does the owner's ruling supersede it? |
|---|---|---|
| `$HERMES_HOME/plans/2026-09-25_130500-router-competitiveness-plan.md:134` | "Semantic cache · **MCP / A2A / gRPC ingress** · dashboards / `/metrics` before v0.2 · SDKs · …", under the heading at `:132` "## Not doing (write it down so the loop does not drift into it)" | **Yes, for the act of evaluating it.** The entry is a *filing*, not a measurement — nothing behind it measured this axis. The owner's ruling reopens the item for evaluation. It does **not** adopt anything, and it supersedes no other entry in that list. |
| `$HERMES_HOME/kanban/boards/router/attachments/t_fc8fd11a/w1-dossier.md` (the W1 reconciled dossier) | **Silent on this axis**: `grep -c -iE 'mcp\|a2a\|grpc\|ingress'` → **0**; `grep -c -i reopen` → **0** | **Nothing to supersede.** The dossier never considered MCP/A2A/gRPC. It *does* drop four neighbouring items ("`router replay`/`/metrics`", "cache-breakpoint auto-injection", "multi-tenancy / per-key identity", "'Be Rust' / benchmark-chasing" — `:180-198`), each with a reason. |
| the cut-spec card `t_1d86acb2` (card body) | carries the reopening rule the loop operates under, verbatim: "不重开被否决项（档案 §3 末尾四条…）；要重开必须给出档案里没有的**测量**" — *do not reopen a rejected item; a reopening must supply a **measurement** the dossier does not have* | **No — the rule stands, and by its own terms it is not the rule that governs this item.** It is scoped to the dossier's own four dropped items, and MCP/A2A/gRPC is not among them. |

**The two consequences, stated plainly, because the survey's citation was wrong here and this ADR must
not repeat it (audit finding **F1**, severe).** The survey attributed to the W1 dossier both a
*rejection* of this item and the *reopening rule*; neither is in the dossier (the rule lives in a card
body, the filing in a plan file). So:

1. **The dossier's reopening rule does not apply to this item.** It never applied: you cannot reopen
   something a document never rejected. What the dossier is, on this axis, is **silent** — and silence
   is a *weaker* authority for a "no" than a rejection is. A reader deserves to know which one this is,
   and it is silence.
2. **An evaluation answer is not a measurement of a competitor.** Neither the survey nor this ADR
   produces a measured comparison against any other gateway — there is **no comparable measurement**
   of this router against any other gateway, and the owner **declined the shared benchmark**. Where the
   honest sentence is "no measurement exists", that is what is written; a competitor's capability list
   is a capability list, never a measurement of this product (constraint 5).

The evidence bar this ADR therefore uses is the repository's ordinary one: **a claim about the outside
world carries a source the audit resolved; a claim about this codebase carries a `path:line` at HEAD; a
number nobody measured is not written.** Where the bar cannot be met, the verdict says so and names the
measurement that would lift it (§4, §5).

## 2. The three questions, and the three verdicts

The question is answered **separately, three times** — a single verdict over a collapsed "add MCP
support" is itself the defect this round exists to avoid (the survey was required to keep the three
apart, and the audit's §4 recorded the separation as **HELD**). It is legitimate — and here it is the
case — for one direction to be adopted while another is declined.

| # | The question | Verdict | One-line reason |
|---|---|---|---|
| **(a)** | **serving on another protocol's semantics** — an MCP tool-call facade, an A2A agent card + task surface, a gRPC/Connect service | **DECLINE** all three | Each is a *translation surface*: the inbound bytes are no longer the client's LLM request and the reply is synthesized, not forwarded. That is a **third** permitted mutation / a **seventh** declared translation cell — `AGENTS.md` constraint 1's subject, and the owner's decision, not a round's (ADR-015 item 5). v0.1 already refuses the same class of surface six times with `501 not_implemented` (`book/protocols.md:54-61`) |
| **(a′)** | **an MCP management-plane facade** — the router's *own* facts (`/health`, usage, routes) exposed as read-only MCP tools | **NOT ADOPTED in v0.1** — registered as the owner's decision | It has **no** constraint-1 conflict (it never enters the LLM request path; it reads product state exactly as `/health` does, `crates/router-cli/src/lib.rs:584-593`). But it is a **new protocol surface** and a **new documented surface**, which spec §9.3 (`:2302`, rule at `:2316-2317`) makes a *contract* decision, and the plan's own rule for a new contract is "ADR + owner signature". It is a different question from (a) and is decided separately — see §7 **O2** |
| **(b)** | **another transport only** — HTTP/2 / h2c framing of the *same* request bytes | **ADOPT, conditionally** — the one actionable direction | The change is **one feature on an existing dependency**: `http2` added to `axum = "0.8"` at `Cargo.toml:40`. axum 0.8.9's default feature set does **not** include `http2` (verified by the audit against the published `.crate`, `E31`), so the process today accepts HTTP/1.x inbound only, while `reqwest` already carries `http2` outbound (`Cargo.toml:56`). No semantic change, no constraint touched (§3). Condition: the §4 experiment passes **and** a trigger exists; otherwise stay on HTTP/1 and keep this evaluation |
| **(c)** | **consuming outward** — the router reached *through* the MCP/A2A ecosystem from outside its serving path | **ADOPT** the external-adapter story (docs only); **DECLINE** MCP client-side consumption | (c1) Documenting how to front the existing OpenAI-compatible endpoints with an external MCP gateway costs **zero product bytes**, exposes no constraint, and is what the surveyed ecosystem actually does with gateways that are not gateways-for-MCP (below). (c2) MCP client-side tool aggregation is fatal on the serving path under constraint 1 (`tools` are part of the prompt, and re-formatting tool schemas invalidates the prefix cache for the whole conversation — `AGENTS.md` environment gotchas); as pure proxying of MCP traffic it is **a second product**, i.e. a roadmap decision for the owner, not an ingress feature of this one |

**(b) is named exactly, so a later card can act on it.** The direction is *carry the same bytes over
h2c*: enable `http2` on the existing `axum` dependency (`Cargo.toml:40`) so the same
`tokio::net::TcpListener` (`crates/router-cli/src/lib.rs:912`) and the same `axum::serve(listener, app)`
(`:971`) also negotiate HTTP/2. Nothing about the request path changes: the three guarded protocol
routes (`:892-907`) and the byte-level mutations are untouched. What is **not** established by any
source is the negotiation behaviour itself: the cited page says only that `axum::serve` "supports both
HTTP/1 as well as HTTP/2" (`E30`), and the survey's prior-knowledge-h2c sentence exceeded that text
(audit finding **F11**). The experiment (§4) verifies it; this ADR does not assert it.

**The prior-art negative, in the bounded form the audit requires (finding F2, severe).** Raw form:
*no LLM gateway ships LLM chat as its production MCP ingress* — **this is false as an unbounded claim,
and the counterexample was found in one search**: Lemonade (AMD's local inference server) ships chat
completion as a documented MCP tool (`lemonade_chat`) at `POST /mcp`, on a regular API route that
honours the same API key as its OpenAI-compatible endpoints (evidence:
`autowork/harness/r52-0b/evidence/falsification-lemonade-mcp.md`). The counterexample carries a
distinction — Lemonade is a **single-backend local model server**, not a multi-provider routing
gateway — but the distinction does not rescue the unbounded sentence. The form this ADR uses, and the
only form that survives the audit: **of the eleven gateways the survey sampled, none exposes LLM chat
completions as its production MCP ingress; the one counterexample found elsewhere is a local model
server, and no demand measurement for this product exists in either direction.** The **(a)** decline
does not rest on this negative at all — it rests on the constraint-1 conflict above — which is why the
false phrasing changes no verdict (audit §8 F2).

## 3. The constraint analysis, named

Which `AGENTS.md` constraint each direction touches, and whether it would need a **third** permitted
mutation or a new **documented surface** — i.e. something this round may not grant (constraint 9 /
ADR-012: a constraint amendment or a frozen-surface change is a human decision, never a loop outcome).

| Direction | constraint 1 — byte boundary (the client's bytes, exactly two mutations) | constraint 2 — content determinism | constraint 3 — observation boundary | needs a 3rd mutation or a new surface? | a frozen surface / gate touched? | verdict |
|---|---|---|---|---|---|---|
| **(a1)** MCP chat-completions-as-tool | **named conflict.** Inbound bytes are MCP JSON-RPC, not the client's LLM request; the reply is a synthesized MCP content block. The proxy cannot forward these bytes and has nothing upstream that speaks MCP | would bite if the mapping were not a pure function — a new mapping must be a translation cell with its own determinism obligation | not engaged (product surface), but the mapping is a serving-path object | **yes — a seventh declared translation cell / a third mutation** | yes, in effect: constraint 1 text | **DECLINED** → §7 **O1** |
| **(a2)** A2A agent card + task surface | **named conflict**, same shape: task messages in, a synthesized chat completion, task artifacts out — translation, not forwarding | as (a1) | not engaged | **yes** | constraint 1 text | **DECLINED** → §7 **O1** |
| **(a3)** gRPC / Connect service | **named conflict.** A proto schema *is* a new semantic contract; requests arrive in a shape the proxy must translate. There is also no existing proto to implement: no public LLM provider documents chat over gRPC except Vertex (`E35`, against `E37`'s REST-only references), so the work would be *authoring* a new API | as (a1) | not engaged | **yes** | constraint 1 text | **DECLINED** → §7 **O1** |
| **(a′)** MCP management-plane facade | **none.** MCP requests never enter the LLM serving path; the facade reads product state as `/health` does (`lib.rs:584-593`) | none (no transform) | **none** (product surface, no autowork involvement) | **no third mutation**; but **yes, a new documented surface** | **yes — spec §9.3's documented-surface rule (`:2316-2317`)** and the plan's new-contract rule | **NOT ADOPTED in v0.1** → §7 **O2** |
| **(b)** h2c / HTTP-2 ingress, same bytes | **none.** Constraint 1 governs application bytes; HTTP/2 changes **framing** only. The byte-boundary assertions compare **body** bytes and are framing-blind — *to be verified by the experiment, not assumed* | **none** (no transform is involved) | none | **no** | no constraint and no gate definition; the implementing card owes a **framing clause in spec §2/DESIGN §12.10 first** (constraint 8) | **ADOPT, conditionally** (§4.1) |
| **(c1)** external adapter, book-only | **none by construction** — the adapter sits outside the process; the router sees ordinary OpenAI-compatible bytes | none | **none** — no autowork involvement | **no** | no; a `book/` page is the write set | **ADOPT** (docs; §4.2) |
| **(c2)** MCP client-side consumption | **fatal on the serving path**: injecting merged tool definitions mutates request bytes, and `tools` are part of the prompt (schema order/formatting changes destroy the prefix cache for the whole conversation). As pure forwarding of MCP traffic it does not conflict — but then it is a second product | bites wherever the tool list is merged deterministically-or-not | not engaged | **yes, if it is to be useful on the serving path** | constraint 1 text | **DECLINED** (serving-path half); the "second product" half is the owner's roadmap call |

**What this table therefore says, in one sentence:** the only directions that need **no** amendment are
**(b)** (a transport flag) and **(c1)** (a page of documentation); everything under **(a)** — including
the constraint-free **(a′)** facade, whose blocker is a *surface* rather than a mutation — is a
decision this round may not take, and is written in §7 as the owner's.

## 4. The cheapest falsifiable experiment each adopted direction would need

Shape follows the repository's evaluation rounds (a question, a cheap experiment, a named kill result).

### 4.1 (b) h2c — the conformance-identity experiment (scratch worktree, hours, `$0.00`)

- **Question:** does the same request over h2c produce the same upstream bytes, and does the process
  actually negotiate HTTP/2 on the existing listener?
- **Experiment:** on a scratch worktree, add `http2` to `axum`'s features (`Cargo.toml:40`), re-run the
  **full** conformance suite — the 3×3 protocol matrix, the byte-equality and prefix-hash assertions —
  and then point one real client (codex / hermes) at the h2c listener, with
  `curl --http2-prior-knowledge` as the framing control. No new dependency, no new route, no config key.
- **Kill criteria:** (i) **any** conformance identity changes — the byte-boundary assertions must be
  framing-blind, and that is *verified*, never assumed; (ii) the experiment passes but **no client asks
  for it and no measurement shows connection overhead that matters** — then revert the flag and record
  the negative. There is **no comparable measurement** of an h2c benefit on this router today, and this
  ADR mints none.
- **What it must not do:** mint a latency claim. Any statement of an h2c latency effect is a claim
  about the L1 quantity, and `autowork/STATE.md`'s waiting-on-human **row 1** is that the latency
  envelope has no contract home: the loop writes *declared + measured*, invents no number, and derives
  no timeout from it.

### 4.2 (c1) the external-adapter page — write the page (a `book/` card, `$0.00`)

- **Question:** is the sentence *"you can put this router behind an MCP-speaking front door today"*
  actually true, end to end?
- **Experiment:** write the `book/` page with one worked example fronting `/v1/chat/completions` with a
  maintained, permissively-licensed REST→MCP wrapper (`IBM/mcp-context-forge` — Apache-2.0 — and Kong
  AI Gateway's "Map a RESTful API to MCP tools" are the two the survey sourced; `E36`, `E13a`).
- **Kill criterion:** the worked example cannot be made to run end to end against a real MCP client
  **while writing it**. That would falsify the "users can already do this" claim, and the page must then
  say so instead of shipping.

### 4.3 The experiments that would be needed to *reopen* what was declined

Not run by this round; named so a future round can run them without re-litigating the reasoning:

- **(a)/(a′)** — the reopening instrument for (a) is the measurement the evidence bar of §1 asks for
  (a claim without one is not written): **a real
  user (or the owner) demonstrating an MCP-only or A2A-only client workflow that the OpenAI-compatible
  endpoints plus the (c1) adapter story cannot serve.** For (a′), the cheap instrument the survey named:
  a one-day scratch spike mounting `rmcp`'s Streamable HTTP service at `/mcp` exposing one read-only
  tool over `/health`'s JSON, with kill criteria (i) the mount cannot satisfy the existing auth guard's
  invariants without special-casing (`lib.rs:576-583`), and (ii) after a `book/` note exists, zero user
  asks in a full round.
- **(c2)** — no experiment is owed. If the owner wants an MCP gateway in this repository, that is a
  roadmap decision for a second product, taken on its own evidence (Envoy AI Gateway's and LiteLLM's
  feature surfaces are the prior art: `E18`, `E6`).

## 5. What would change my mind, per verdict

| Verdict | This would change it |
|---|---|
| **(a) decline** | The owner amends constraint 1 (or grants a declared translation cell) **and** the reopen instrument of §4.3 materializes. The amendment is theirs alone (ADR-015 item 5); the workflow demonstration is the measurement. Absent both, no amount of ecosystem adoption changes it: another product shipping a surface does not make this proxy's byte boundary movable |
| **(a′) not adopted** | A user asks for the router's own facts over MCP **and** the surface's shape is decided as a contract (spec §9 + §9.3 discipline: names, auth, and what a tool may read). I would then support it: it is the only serving-side MCP candidate with no constraint-1 conflict, and the Rust prior art is real (`rmcp`; TensorZero shipped exactly this shape at `/mcp` in 2026.4.0 — `E15`, with `E17` a secondary source, flagged) |
| **(b) conditional adopt** | A conformance-identity failure over h2c (kill (i)) — that would make the flag unsafe, and it would become a *wrong* answer, not a deferred one. Symmetrically: a real client needing multiplexing, or a measurement showing connection overhead that matters, would promote it from "evaluated, ready" to "adopt now" |
| **(c1) adopt** | The worked example failing to run (§4.2). The direction would then become "tell the user honestly that the adapter story is unproven, and name what it needs" |
| **(c2) decline** | Nothing inside this repository's constraints. It is a second product's question; the thing that would change the answer is a roadmap decision by the owner, not evidence about ingress |

## 6. The evidence, and the limits the audit put on it

**Cited by commit hash, on `round/52-ingress-evaluation`:**

- the survey — `autowork/survey/2026-09-27_ingress-mcp-a2a-grpc.md` at **`786eb40`** (plus its sources
  file `…sources.txt`); every external claim below is one of its `[E#]` ids.
- the audit — `autowork/harness/r52-0b/REPORT.md` at **`8bff7ca`**, verdict **`PASS-with-findings`**
  (43 citations re-resolved by the auditor's own commands and fetches: **34 resolved fully, 4 with a
  nit, 5 failed, 1 link-rot re-verified**), with its raw artifacts under
  `autowork/harness/r52-0b/evidence/`.
- the survey's `[R#]` in-repo anchors were held clean at HEAD by the audit; this card re-read the ones
  this ADR leans on itself (`autowork/harness/r52-1/REPORT.md` §5 carries the commands and their
  output).

**Per-claim sources the audit confirmed, as this ADR uses them:**

| Claim used here | Source | Audit verdict |
|---|---|---|
| h2c is the framing question, not a semantic one; `http2` is a non-default axum feature | `E30` (axum::serve supports HTTP/1 and HTTP/2), `E31` (axum 0.8's default feature set) | E31 **verified against the published `.crate`**, not a summary page; E30's text verified, but the *negotiation* claim exceeds it (**F11**) |
| the six cross-protocol cells refuse with `501` | `book/protocols.md:54-61` (`[R4]`) | verified |
| axum's licence | `E30`/`E31`; published axum 0.8.9 `Cargo.toml` | **audit correction F5: MIT, not MIT/Apache-2.0** |
| `rmcp` 3.4.1 is mountable on an axum router; official Rust SDK | `E3`, `E4`, `E5` | resolved; **F6: the licence is Apache-2.0 with a residual MIT→Apache transition notice, not a clean OR-dual** |
| MCP current revision 2026-07-28; stdio + Streamable HTTP | `E1`, `E2` | verified |
| A2A v1.0.0, three bindings, `/.well-known/agent-card.json`; `a2a-rs` young, all crates pre-1.0 | `E24`, `E26`, `E27` | verified (84 stars, pushed 2026-09-25, Apache-2.0) |
| `tonic` 0.14.6 (MIT); `connectrpc` 0.9.1 (Apache-2.0, pre-1.0, conformance-suite tested) | `E28`, `E29`, `E32`, `E33`, `E34` | verified |
| no public LLM provider documents chat over gRPC except Vertex | `E35`, `E37` | consistent with the auditor's own reads (a negative claim, honestly sourced) |
| management-plane MCP prior art (the (a′) shape) | `E15` (TensorZero release note), `E21` (OpenRouter, explicitly a *development assistant*, "keep calling the OpenRouter API directly"), `E38` (Helicone) | verified; `E17` (deepwiki) is a **secondary** source, flagged as such |
| REST→MCP wrapping exists and is maintained (the (c1) prior art) | `E36` (IBM mcp-context-forge, Apache-2.0), `E13a` (Kong "Map a RESTful API to MCP tools") | verified (the Kong **"AI Gateway 2.0"** qualifier is **unverified** — **F10**; the 3.14 min-version is verified) |
| the Lemonade counterexample | `r52-0b/evidence/falsification-lemonade-mcp.md` | the auditor's own fetch; recorded because it **falsifies the unbounded negative** (**F2**) |

**Findings carried into this ADR as limits on what the evidence supports** (not quietly repaired —
they are limits, and the reader is entitled to them):

1. **F1 (severe).** The survey's `[R7]` — a *rejection* and a *reopening rule* attributed to the W1
   dossier — is a fabricated citation: the dossier is silent on this axis and contains no such rule.
   §1 above states the corrected attribution and says which authority is weaker as a result. **This ADR
   repeats none of it**, and the survey carries the correction as an appended block (below).
2. **F2 (severe).** The strong prior-art negative needed the bounded form, with the Lemonade
   counterexample as a footnote. §2 writes the bounded form. The **(a)** decline survives unchanged.
3. **F3/F5/F6/F7/F8/F9/F10/F11.** The listed corrections touch claims this ADR either uses in corrected
   form (axum MIT; `rmcp`'s licence; Kong 3.14 without "2.0"; h2c negotiation unverified; the
   agentgateway gRPC cell unsupported as cited; the Cloudflare and LiteLLM cells narrowed; Portkey's
   moved URL) or does not use at all (no vote of this ADR rests on a dangling `[E23]`). The
   survey's own "the repository's only *MCP* mention" sentence is false as worded (**F4**): the mention
   also exists at `rules/tool_output.toml:110`, and in four loop-side `autowork/` files (the audit's
   sweep). The claim this ADR *does* rely on — **no MCP/A2A/gRPC/SDK code, dependency or config key
   exists in this repository** — the audit verified TRUE and this card re-verified at HEAD (no match in
   `Cargo.toml` or any crate manifest).
4. **F12 (bookkeeping).** The sources file marks `E25` "(cited)" though it is cited nowhere inline —
   cosmetic, and recorded in the appended block rather than edited into the survey's history.
5. **The audit's §4 observation, weighed here as required.** A new `/mcp` route would be a new
   *reporting surface*, and spec §9.3's rule (`:2316-2317`) makes that a contract decision rather than
   a route addition. That is exactly why **(a′)** is written as the owner's decision (§7 **O2**) and
   not as a "conditionally adopt" of this ADR.

**Not evidence, and not used:** any competitor's marketing figure, throughput or "faster" claim (none
appears in the survey, and none appears here); any spend estimate (nothing is purchased); the deepwiki
page as a primary source; and any inference from "other products ship it" to "this product should".

## 7. Open decisions for the owner — this round does not decide them

Each of these is a decision the loop **may not take** (AGENTS constraint 9 with ADR-012: the gate
definitions, the frozen corpus, the conformance assertions, the L1 envelope and the contracts
themselves are outside the loop's mutable scope; and `AGENTS.md` constraint 1 is the charter itself).
They are written in the shape `autowork/STATE.md`'s *Waiting on human adjudication* table uses, so
R52's close-out can carry them into it as new rows.

| # | What it is | Why it is not the loop's to decide | What the human must decide | Source | State |
|---|---|---|---|---|---|
| **O1** | **Whether the byte boundary ever admits a semantic ingress** — i.e. whether to grant a **third permitted mutation** (ADR-015 item 1's "exactly two" becomes three or more), or to declare a **seventh translation cell** for MCP/A2A/gRPC semantics alongside the six `501` cells | `AGENTS.md` constraint 1 *is* the charter, and ADR-015 item 5 says a third mutation is a change to it "which no round may make on its own"; ADR-012 puts `AGENTS.md`, `docs/spec.md` and `design/` outside the loop's reach, and spec §2's translation contract is the same class of text. This ADR's **(a)** verdict is a *refusal to take* this decision, not a substitute for it | whether such an ingress is wanted at all; and if so, which shape — a new mutation with its own conformance assertions, or a declared translation cell held to the determinism/lossy-point obligations of `book/protocols.md:54-82`. Until then the six `501` cells stay the whole cross-protocol promise | §2/§3 above; ADR-004; ADR-015 items 1 and 5; ADR-012 item 2; `book/protocols.md:54-61` | **open — registered 2026-09-27 by R52-1, not acted on.** No code, no cell, no text was changed by this ADR |
| **O2** | **Whether the router's own facts become a served MCP surface** (the **(a′)** `/mcp` management-plane facade) | it is a **new documented surface**: spec §9.3's rule is that a surface's shape is frozen by the change that implements it and a documented-but-unreachable surface is a defect; and the plan's rule for a new contract is "ADR + owner signature". Nothing in this ADR authorizes the route, the tool names, or what a tool may read | whether to open this surface at all; and if so, its shape as a contract (tool set, auth model, what state a tool may read/export) and its owner signature. The one-day `rmcp` spike of §4.3 is the feasibility instrument, not the decision | §2 **(a′)**/§3 above; spec §9.3 (`:2302`, rule `:2316-2317`); `crates/router-cli/src/lib.rs:584-593`; `E15`/`E17`/`E21`/`E38` (prior art) | **open — registered 2026-09-27 by R52-1.** `grep -rn -i 'mcp' crates/ Cargo.toml` remains 0 matches; no route, no dependency |
| **O3** | **Whether a comparison instrument for this question is ever funded** — the question "does anyone want a non-OpenAI ingress?" has **no comparable measurement** anywhere, and the one cheap instrument that would produce one (a shared benchmark) was **declined by the owner** | funding a shared benchmark is an owner act (it spends the owner's budget and puts the product on a public axis); the loop may not invent a demand figure, and constraint 5 forbids entering an estimated price or a borrowed competitor number as evidence | whether to reinstate a shared benchmark (or any demand instrument), or to leave this question answered on constraint grounds and user signal alone. Until then, every "who would use it" statement in this ADR is a **capability** statement with a named audience, never a measured demand | §1 item 2, §2, §5 above; the survey's §3/§5; `autowork/STATE.md` waiting-on-human row 1 (the latent-claim class) | **open — registered 2026-09-27 by R52-1.** No benchmark was run, no provider dialled |

**What this round decided, in contrast (so the boundary is unambiguous):** the three verdicts of §2,
the constraint table of §3, the experiments of §4, the change-my-mind conditions of §5, and the
adoption of **(b)** *as a direction gated on its experiment* and **(c1)** *as a documentation
direction*. Everything that would move a constraint, a gate, a frozen corpus or a documented surface
is in the table above and **not** decided here.

## Alternatives considered

| Alternative | Why rejected |
|---|---|
| **Collapse the three questions into one verdict** ("add MCP support: yes/no") | the collapse is the defect this round exists to prevent: the same word ("MCP") covers a translation surface, a transport flag and a documentation story, and they land in three different places. The audit explicitly held the separation (its §4), and §2 keeps it |
| **Treat the plan's "Not doing" entry as binding and write a no-op ADR** | the owner's direction of 2026-09-27 reopened the item, and an entry filed without a measurement cannot be answered by citing itself. Reopening ≠ adopting, which is what §2 shows |
| **Adopt (a) now because competitors have the surface** | "other products ship it" is a capability observation, not evidence about this product: it collides with constraint 1 (the one promise this gateway makes about bytes) and it has no demand measurement behind it. Vendor surfaces are not measurements |
| **Adopt (a′) because it has no constraint conflict** | a *surface* is not a *mutation*, but it is still a contract: spec §9.3's rule and the plan's new-contract rule both put its shape in the owner's hands, and the router need not expose its own facts to be useful (the report surfaces of spec §9.1/§9.2 already serve their reader) |
| **Declare "undecided — wait for user demand"** | an evaluation that returns no verdicts is not an evaluation. Each question gets a verdict, a kill condition and a reopen instrument; "wait" is what the named instruments encode, not an excuse for silence |
| **Add `rmcp` / `a2a-rs` / `tonic` / `connectrpc` to the workspace now, behind a feature flag** | a new dependency is a design-contract change (`design/DESIGN.md` §12.1's allowlist), and it would land capability with no trigger, no demand and no experiment. The (b) direction is the deliberate counterexample: **no** new dependency — a feature on an existing one |
| **Treat the survey as the decision** | the audit found a fabricated citation and an unbounded negative (F1/F2). A decision record must not inherit either; §1 and §6 carry them as limits instead |
| **Edit the survey's text in place** | the survey is a closed card's artifact and this repository does not restyle history. The corrections are **appended** (see below) with the original text left visible |

## Consequences

- **`design/decisions/` gains this ADR only.** No code, no config, no dependency, no schema, no
  conformance id, no spec/DESIGN clause, no `README.md` byte. `Cargo.toml:40` and every crate manifest
  are untouched: `grep -rn -iE 'mcp|a2a|grpc|tonic|connectrpc' Cargo.toml crates/*/Cargo.toml` returns
  nothing at this HEAD.
- **`book/roadmap.md` gains one paragraph** (in that file's *Outline*): what the question was, what
  was decided, what to expect next — with the engineering verdicts left to this ADR, per constraint 8.
- **The survey carries an appended corrections block** — "Corrections (R52-1, from R52-0b's F1/F2)" —
  stating the corrected attribution (F1), the bounded negative with the Lemonade footnote (F2) and the
  audit's minor corrections that touch its text (F3–F12). **Appended, never rewritten:** the original
  lines stay visible.
- **Two next cards are named, not created by this round** (the orchestrator cuts them):
  (i) a `book/` card writing the (c1) external-adapter page with its worked example, whose acceptance
  is §4.2; (ii) an implementation card for (b), which may only run **after** §4.1's conformance
  experiment and after the framing clause is written into spec §2/DESIGN §12.10 first (constraint 8) —
  and which must re-run the four gates and the byte/cache invariants, not only the new case.
- **The open decisions go to the human-decision ledger.** R52's close-out should carry **O1, O2 and
  O3** into `autowork/STATE.md`'s *Waiting on human adjudication* table as new rows (the next free row
  is **19**), in that table's six-column shape, since none of them is loop-actionable.
- **The evidence chain is reproducible from the branch:** the survey at `786eb40`, its audit at
  `8bff7ca`, this card's own re-reads in `autowork/harness/r52-1/REPORT.md`. **No figure of any kind is
  minted by this ADR** — it touches no cost model, no trace field and no `verified` label, and the
  `verified` rows this repository already holds (R39/R40; `D3` closed as **MET** by R41's row,
  `autowork/STATE.md:466-481`) are untouched and un-retracted by it.
- **Nothing in this ADR authorizes any implementation.** If a later round adopts (b) or (a′) it must
  satisfy its own gate; this document is a decision about *directions*, and its adoption clauses are
  conditions, not permissions.

## Honest boundaries

- **Not measured here, because nothing was run.** This round is docs-only and offline: no provider was
  dialled, no credential was read, `$0.00`. The (b) experiment, the (a′) spike and the (c1) worked
  example are **specified**, not executed — their results are what a later round would produce.
- **Not established: the h2c negotiation behaviour.** The cited doc says only that `axum::serve`
  supports HTTP/1 and HTTP/2 (`E30`); the survey's prior-knowledge-h2c sentence exceeded it (**F11**).
  This ADR asserts the *feature flag's absence today* (verified against the published `.crate`) and
  leaves the negotiation claim to §4.1.
- **The outside-world claims are second-hand by construction.** They are the audit's re-resolved set,
  not this card's own fetches; five of the survey's 43 citations failed and one of them (**R7**) is the
  reason §1 exists. Where a claim failed, this ADR either uses the corrected form or does not use it.
- **No demand measurement exists** for any direction, in either direction, and none is implied: the
  ecosystem evidence is *capability* evidence, and the only demand instrument this question could use
  was declined by the owner (**O3**).
- **The (a′) reading is a judgement, not a measurement.** Classifying the management-plane facade as
  "a surface, not a mutation" follows from `lib.rs:584-593`'s shape and the audit's own §4 observation;
  a reader who classifies it differently still lands on the same verdict (not adopted in v0.1) because
  spec §9.3 makes its *shape* the owner's either way.
- **The ADR-number gap (041/042 unused) is registered, not decided.** If the register's numbering is
  meant to be dense, that is a bookkeeping decision for whoever owns the register.
