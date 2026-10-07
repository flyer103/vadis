# ADR-053 — `DESIGN` §12.10.3 R4's clock, amended again: the attempt knob bounds **head arrival only**, the body is bounded by the per-read idle gap, and `server.request_timeout` becomes the stream path's whole-request outer bound

- Status: accepted
- Date: **2026-10-07** (round **R71**'s contract card, `R71-0`; the owner's authorisations of the same date are quoted verbatim in §1.2)
- Kind: **contract reversal under owner authorisation — docs-only in this card.** It supersedes **ADR-044's clock (i)** — the *total-elapsed* reading of R4's bound — and leaves **everything else** of ADR-044 standing; it amends one paragraph of `design/DESIGN.md` §12.10.3 (R4) plus `DESIGN` §12.20's truncation limb, one clause of `docs/spec.md` §4.2, and one user-facing bullet of `book/protocols.md:114-122`; it moves §12.8's heading to `CONF-01…CONF-105`, adds the `CONF-105` row, records the round in §12.9, and freezes the **CONF-90 repair list** (§8.2). Two supersession records ride with it: a status marker and a dated note on **ADR-044** itself, and a dated note on **ADR-045 §3.5 item 2** (§8.3) — both appends, both append-only-respecting, neither rewriting a sentence of those ADRs' decisions. **No product byte changes here**: nothing under `crates/`, `tests/` or `.github/` is in this card's diff, **no timeout value moves, no config key is added**. Two consequences land later, each on a named card: the code change (`R71-1`) and the CONF-90 repair (`R71-2`).
- Authority: **the owner's directions of 2026-10-07**, quoted verbatim in §1.2. **ADR-044 §6** — the clause that pre-authorised exactly this reversal *"only via its own ADR and its own measurements"* — is the door this ADR walks through; the measurements it owes are specified in §5.
- Supersedes (in the narrow sense §8.1 spells out clause by clause): **ADR-044's clock (i) only** — the phrase *"the bound is on **total elapsed time since the head**, not on the gap between bytes"* (`ADR-044:77-78`), and R4 as ADR-044 amended it. ADR-044's own letter (R4's original *idle-gap* sentence) is **restored** as the body's bound; ADR-044 stands on disk, marked superseded for that clock.
- Related: `AGENTS.md` hard constraints **1** (the byte boundary — this ADR changes no relayed byte), **2** (content determinism — untouched: the bound is a function of config and traffic, never of content), **3** (the observation boundary), **8** (docs before code — why the `book/` bullet moves with the contract), **9** (**the measurement is not part of the search space** — why the CONF-90 repair is an owner-authorised act with a list frozen in §8.2); **ADR-011** (the upstream-error taxonomy; item 6 row 3 `unknown_outcome`; item 5's `server.request_timeout` budget sentence) and **ADR-010 item 4** (the crash window); **ADR-012** (the never-mutable paths); **ADR-044** (superseded for its clock, kept as the record); DESIGN §12.10.1 (the client's `connect_timeout`), §12.10.3 R1/R2/R4/R6/R8/R12, §12.10.5; `docs/spec.md` §4.2, §8; `book/protocols.md:114-122`.
- Cases: **`CONF-105`** (allocated by this round's owner act, §4; the row lands in `DESIGN` §12.8; its case file lands with `R71-1`). No existing ID is spent, reused or renumbered, and **`CONF-98`/`CONF-99` stay the parked branch `round/67-abandoned-attempt`'s** (`ADR-050` stays taken by that branch; nothing on it is read, switched, merged or deleted by this round).
- **Line-number convention.** Every `path:line` below resolves at **this branch's HEAD** (the commit that carries this ADR). `R71-1`'s and `R71-2`'s edits shift the numbers in the files they touch; where the drift matters it is stated rather than left to be discovered.

---

## 1. Why

### 1.1 The question, and where the previous answer came from

ADR-044 (2026-09-28) ruled that R4's bound runs on **total elapsed time since the attempt was sent** rather than on the **gap between bytes**, on the ground that the code *was* the wider of the two. That reading was correct for the code as it stood, and it still describes what the code does today:

`crates/vadis-providers/src/stream.rs:128` applies `server.upstream_attempt_timeout` to the streaming send as reqwest's **per-request timeout** — `out.body(body).timeout(self.idle_timeout).send()`. Reqwest's per-request clock runs from the moment the request is sent until the response **body is finished**, so the same knob that catches a stall also truncates a stream that never goes idle: any SSE body whose *total* length exceeds `server.upstream_attempt_timeout` dies mid-body with a bare `error_class: stream_truncated` and no `[DONE]` (R6's declared-truncation shape).

The fix the owner directed on 2026-10-07 changes **which clock** the knob runs on, and nothing else. ADR-044 §6 (open of this file, §"Reversibility", `ADR-044:202-211`) reserved that move to its own act:

> Reverting this decision means restoring one sentence of `DESIGN` §12.10.3 and two clauses of `docs/spec.md` §4.2 … **plus** the code change the letter would then demand: removing or re-scoping reqwest's per-request `.timeout(...)` at `crates/vadis-providers/src/stream.rs:128` so that a busy stream is no longer capped. That code change … would need **its own ADR and its own measurements**.

This is that ADR. It carries the owner's authorisations (§1.2) and it specifies the measurements (§5).

The question was *not* reopened by any round: the reversal is the **owner's act**, and this card instantiates it — the same act class as ADR-044 itself, which recorded an owner ruling that the document yield to the code (AGENTS 9 / ADR-012 put `design/` and `docs/` outside a round's own reach).

### 1.2 The owner's authorisations, verbatim

Two authorisations, given 2026-10-07 **in the conversation that opened this round**. They are **authorisations, not loop outcomes**: the loop did not decide this, and no round re-opened ADR-044. Quoted verbatim (Chinese original), each with an English gloss.

**(a) The clock — the streaming send's wrap.**

> 「stream.rs:128 改为 tokio::time::timeout 只包 head 到达(与 read_chunk 同构),body 只受 idle bound」

*Gloss:* *"Change `stream.rs:128` to `tokio::time::timeout`, wrapping **only head arrival** (isomorphic with `read_chunk`); the body is bounded **only** by the idle bound."* — with the **buffered** path's `.timeout` at `crates/vadis-providers/src/lib.rs:161` declared **correct** and **out of scope** (that path consumes the body inside `send`, so a per-request timeout there is the right shape, not the defect §1.1 describes).

**(b) The un-enforced `server.request_timeout` — enforcement, stream path only.**

> 「确认,按此开卡」

*Gloss:* *"Confirmed; open the card on this basis."* — said of the finding that `server.request_timeout` is documented as the outer bound on the **whole** request *including the stream* (DESIGN §12.10.1, §12.10.3 R4, §12.10.3 R12) while **no enforcement exists anywhere in `crates/`** (a measurement, not a reading: `grep -rn request_timeout crates/` returns config parsing, defaults, examples and test fixtures — no timer). The direction that follows: enforce it **this round**, on the **stream path only**, as the outer bound of the whole request (head phase + relay, **single owner**), tripping into the existing **declared** shape (R6; never a fabricated terminal). The buffered path is **not** touched this round and the buffered-side decision is **deferred and named** (§3, §6).

**Two further owner decisions of the same date ride with this ADR** (not new contract facts of their own; recorded where they belong):

**(c) The CONF-90 repair is authorised in principle.** CONF-90's three-way calibration machine is built around *"death at the total-elapsed bound"*, which the new clock makes **unreachable for a busy stream**. Repairing a conformance case is normally outside a round's reach (AGENTS 9: the measurement is not part of the search space); this owner act authorises it, and §8.2 **freezes the edit list** so `R71-2` re-decides nothing.

**(d) `CONF-105` is allocated to this round's witness.** DESIGN §12.8's heading moves `CONF-01…CONF-104` → `CONF-01…CONF-105`; the ID is spent, never renumbered, never reused; `CONF-98`/`CONF-99` stay the parked branch's and `ADR-050` stays that branch's too (§4).

### 1.3 What the implementation does today, and what this ADR changes

| Site | Today | After this ADR (landed by `R71-1`) |
|---|---|---|
| **Head arrival**, stream path | reqwest's per-request `.timeout` at `stream.rs:128` — clock runs send→body-finished, so it bounds *everything* | `tokio::time::timeout(self.idle_timeout, send())` wrapping **only** the head, **isomorphic with `read_chunk`** (`stream.rs:181-197`); the clock stops at head arrival |
| **The body**, stream path | capped *again* by that same per-request timeout (the wider, earlier of the two deadlines) | bounded **only** by the per-read idle arm (`read_chunk`, unchanged, one read at a time) |
| **The whole inbound request**, stream path | **unbounded in practice** — `server.request_timeout` is documented but enforced nowhere (§1.2 (b)) | `server.request_timeout` enforced, **single owner** (§3) |
| **The buffered path** | `.timeout` at `lib.rs:161` covers send→body-consumed | **unchanged** — correct there, out of scope by authorisation (a) |

---

## 2. The ruling

### 2.1 The clock, split into three named bounds

R4's bound is now stated as **two** bounds, plus the outer one:

1. **Head arrival.** `server.upstream_attempt_timeout` bounds **how long the upstream may take to produce a response head**. The wrap is a `tokio::time::timeout` around the send alone, so the clock stops when the head arrives, whichever way it arrives (a status, or a transport error carrying its own taxonomy).
2. **The body.** R4's **original letter** — *"a gap with no upstream bytes longer than `server.upstream_attempt_timeout` is a failure"* — is the body's whole per-read bound: the idle arm inside `read_chunk`, unchanged. A continuously busy stream is **never** truncated by the attempt knob.
3. **The whole request.** `server.request_timeout` (shipped default **10m**) is the outer bound on the whole inbound request — head phase **and** relay — and it is enforced on the **stream path** by this round (§3).

### 2.2 The classification map for the new arm

The new **Elapsed-on-head** arm adds **no new variant, no new status and no new class word**: it reuses the existing `OpenHead::UnknownOutcome` arm (`crates/vadis-proxy/src/stream_forward.rs:1512-1527`, reached today from `stream.rs`'s `Err` branch via the same arm's construction at `stream_forward.rs:1731`).

| Field | Value |
|---|---|
| HTTP status | `502` |
| `error.type` | `upstream_error` |
| `details.stage` | `"unknown_outcome"` |
| `details.error_class` | `"timeout"` (the arm's frozen literal) |
| `details.stream` | `true` |
| retry | **none** |
| failover | **none** |
| closure event | deliberately **none** (unchanged from today) |
| `result.upstream_ms` | recorded (the elapsed head phase) |

**Why this is the conservative arm, and why it is right.** At head-arrival time the vadis cannot know whether the request bytes were fully written — a `tokio::time::timeout` on the send has no visibility into reqwest's internal write state. ADR-011 item 6 row 3 fixes the rule for exactly that ambiguity: **after a full write, a retry is a probable double charge**. The arm that can never double-charge is `unknown_outcome` — no retry, no failover, the record honest about what it does not know. Choosing `NotSent` (the retry-eligible arm) would *guess* that nothing was written, and that guess is the one direction this project never takes.

**Net behaviour identical to today's head-window timeout *on the streaming path*, except the message string.** This needs stating precisely, because it is the thing a reader will otherwise mis-read as a behaviour change:

- **Today**, a head window that expires after a full write already lands in this same arm: `open()`'s `Err(e)` branch computes `wrote_full_request = !e.is_connect() && !e.is_request()` and returns `Err("request fully written but no response head (unknown_outcome): {e}")` (`stream.rs`: the `Err` arm of `open`), which maps to `OpenHead::UnknownOutcome` (`stream_forward.rs:1731`).
- **After** this ADR, the elapsed wrap's own arm feeds the same `OpenHead::UnknownOutcome`. Status, `error.type`, `details.stage`, `details.error_class`, the retry/failover decision and the record count are unchanged. The **only** observable movement is the free-form `message` string carried on the refusal — today reqwest's transport text, after the change the head-arrival window's own words. `details.error_class` remains the literal `"timeout"` in both.

**The pre-existing cross-media carriage difference is recorded, not created.** ADR-044 §6's "its own measurements" obligation and this section's map are about the **class**, and the class word is shared: a full-write-no-response fault carries `details.error_class: "timeout"` and `details.stage: "unknown_outcome"` on **both** media. The *status carriage* however differs **at HEAD, before this ADR, and this ADR does not touch it**:

| Medium | A full-write-no-response timeout produces | Where it is decided |
|---|---|---|
| buffered | `502`, `error.type: upstream_error`, `details.stage: "unknown_outcome"`, `details.error_class: "timeout"` — **as observed on the executed rigs (2026-10-07, §§10-11): the `504 upstream_timeout` row this table carried at first writing is unreachable for these faults** (the `TransportKind::Other` hardcode; a registered follow-up finding, §11) | `vadis-proxy/src/forward.rs:1938-1960` (the `WrittenNoResponse` arm) |
| streamed | `502`, `error.type: upstream_error`, `details.stage: "unknown_outcome"`, `details.error_class: "timeout"` | `vadis-proxy/src/stream_forward.rs:1512-1527` |

This asymmetry is the *"except the class string"* the owner's direction names: it is the **status carriage**, and it is already the tree's behaviour on the two media today. Closing it would be a change to **both** media and is not this round's (it is named in §6 as out of scope).

**The connect window is unchanged.** A host that refuses, or that cannot be reached, still fails inside the client's own `.connect_timeout(server.upstream_attempt_timeout)` (DESIGN §12.10.1) — i.e. as an `Ok(Err(e))` from `send()` with `e.is_connect()`, → `StreamOpen::NotSent(TransportKind::Connect, …)` (`stream.rs:167`) → the ordinary **classified** arm (retry/failover eligible). That is a **fast refusal, not an Elapsed**, so the new arm cannot swallow it. **CONF-29** (`connect_failure`, never `timeout`) is unaffected: its rig is a closed local port, which fails by connect refusal, not by the head window.

### 2.3 The same-fault-same-class invariant, kept

The invariant: **one upstream fault draws the same *class* on both forwarding media** (`docs/spec.md` §4.2's classification-evidence rule; its witness is CONF-82's shape). The new clock keeps it:

- a **slow head** (a head that misses the attempt knob): buffered → the per-request timeout after a full write → `WrittenNoResponse(Timeout)` → the `unknown_outcome` shape; streamed → the new Elapsed-on-head → `UnknownOutcome`. **Same class (`timeout`), same `stage` (`unknown_outcome`), same no-retry / no-failover decision**, each medium in its own status carriage (§2.2's table).
- the **busy stream**: streamed → completes (this ADR's whole point); buffered → the body is consumed inside `send`, so the same total there is one bounded read (the path's own correct shape).
- the **idle stream**: both media → declared truncation, never a fabricated terminal.

**`CONF-105` is the leg that witnesses it** (§4). Nothing here relaxes the invariant; the change moves only *which* clock the streaming head runs on.

---

## 3. The `server.request_timeout` enforcement contract

This round makes a documented bound real, on one path. The contract:

- **What it bounds.** The whole inbound request on the **stream path**: the head phase (the attempt's head arrival, and any failover walk before our head is sent) **and** the relay. One deadline, measured from the moment the request enters the streaming driver.
- **Where it is owned. Single owner.** Exactly one timer — the streaming request's driver in `vadis-proxy`/`vadis-cli` (the place that already owns the relay's end and the terminal accounting). The head-arrival wrap of §2.1 does **not** enforce `request_timeout`, and the relay loop does **not** re-arm it; a second timer is the defect this rule names.
- **What a trip does — R6's two columns, not one shape.** The trip's *shape* is decided by whether our response head has been sent, exactly as R6 already decides every mid-stream outcome:
  - **After our head has been sent** (the relay phase — the shape a long busy stream meets): R6 column 2's **declared truncation**. The relay ends; `errors[]` carries `error_class: "stream_truncated"`, the event's `stream_completed` is `false` with a `stream_truncated_reason` string; **no `[DONE]` is appended, no terminal event is fabricated** (R6's standing rule for both columns, `DESIGN` §12.10.3).
  - **Before our head has been sent** (the head phase): R6 column 1's **ordinary refusal** — the client receives §8's error body, conservatively in §2.2's `unknown_outcome` shape (the request may have been fully written). A stream whose head was never opened cannot be "truncated", and the vadis does not fabricate one to have something to truncate.
- **The budget relation is now checkable.** ADR-011 item 5 states an inbound request has `request_timeout` and an attempt has `upstream_attempt_timeout`; with this enforcement the relation holds on the stream path — the attempt knob's budget (× retries, × candidates) sits inside `request_timeout`.
- **The buffered side is deferred and named.** The buffered path's `.timeout` at `lib.rs:161` is declared **correct** and is **not touched** (authorisation (a)). Whether/how `request_timeout` is enforced on the **buffered** path is a **separate decision**, named here and deferred: it would bound a whole buffered request (head + body) whose body is *already* bounded by the attempt knob, and taking it in the same change would move two bounds on two paths at once — the kind of compound change this register exists to refuse.
- **No new key.** `server.request_timeout` already exists (spec §4); this round adds no timeout value, key, unit, default or example change anywhere.

---

## 4. The blast radius, stated plainly

ADR-044 §3 demanded that any change to this contract carry the blast-radius sentence. This is it, in both directions.

**Gained — the busy stream.** A legitimately long single response now completes, provided (a) it is never idle for longer than `server.upstream_attempt_timeout`, and (b) the whole request stays under `server.request_timeout`. A long reasoning stream is the obvious case: it emits continuously, never idles, and now rides to completion instead of dying mid-body.

**Where the ceiling moved.** A busy stream's de-facto ceiling moves from `server.upstream_attempt_timeout` (shipped default **60s**) to `server.request_timeout` (shipped default **10m**) — a **~10×** widening of how long a continuously-busy stream may run.

**The trickle consequence, stated as the thing an operator must now know.** A stream that sends one byte just inside the idle bound, forever, is **no longer capped by the attempt knob** (its every gap is under the bound) — it is capped by `server.request_timeout` (§3). So the sentence an operator needs is:

> **The attempt knob no longer bounds a busy stream's *duration*; it bounds head arrival and the *gap* between bytes. A busy stream's duration is bounded by `server.request_timeout`.**

**What still ends a stream, and neither is a hang.** A stream that goes **idle** for longer than the attempt knob still dies (R4's original letter, restored); a stream whose **whole request** exceeds `request_timeout` still dies (the new outer bound). Both end as **declared** outcomes, never a silently-complete one and never a fabricated terminal (R6, R8).

**The registered gap is untouched.** ADR-044 §4's *busy-but-slow throughput guard* — a minimum-throughput floor, a separate streaming ceiling, or a per-route override — stays **registered-open and NOT built** (loop state record, waiting-on-human row **25**). Nothing here adds one; the widening above is a bound, not a guard.

---

## 5. The measurements this ADR owes (ADR-044 §6's clause)

ADR-044 §6 pre-authorised this reversal *"only via its own ADR and its own measurements"*. This is the ADR; the measurements are `R71-1`'s obligation, on a **fixed rig**, recorded **before and after** on the same rig:

1. **The busy stream — the change's own claim.** Fixture: a mock SSE upstream that emits events whose **every inter-byte gap is shorter than `upstream_attempt_timeout`** while the stream's **total** elapsed exceeds it. Expect: **before** — a truncated stream at the attempt bound (`error_class: stream_truncated`, no `[DONE]`); **after** — a **byte-complete** stream with its terminal event, `stream_completed: true`, empty `errors[]`.
2. **The slow head — still refused, conservatively.** Fixture: a mock that delays its response **head** past `upstream_attempt_timeout`. Expect: both **before and after** — §2.2's refusal (stream: 502 / `upstream_error` / `stage: "unknown_outcome"` / `error_class: "timeout"`), **no** retry, **no** `failover.triggered` row, exactly **one** record.
3. **A real-client long generation, longer than `upstream_attempt_timeout`.** A live smoke through a real agent client (the codex/hermes shape) whose generation exceeds the shipped 60s knob, asserting completion. This leg is the one an offline mock cannot substitute for — `AGENTS.md`'s own environment gotchas (macOS proxy interception, the TLS-backend trap) are what makes the real-client smoke a separate obligation.
4. **The idle control — must not regress.** Fixture: a mock that goes **silent** for longer than the knob mid-body. Expect: R6's declared truncation, unchanged from today (`stream_completed: false`, a `stream_truncated_reason` string, `error_class: "stream_truncated"`, no `[DONE]`).

Each measurement records the commit it ran at. ADR-044 §6's clause is discharged by recording the before/after pair on the same rig. **No figure is minted into `design/` or `book/`** (AGENTS 9 / ADR-012): the numbers live in the implementing round's evidence, and the documents state the *construction*, never a measured value.

---

## 6. Not in scope (named, so no card adds it by stealth)

| Not in scope | Where it stands |
|---|---|
| the **buffered path's** `.timeout` (`crates/vadis-providers/src/lib.rs:161`) | **untouched.** That path consumes the body inside `send`, so a per-request timeout is the right shape there. The buffered side of `request_timeout` is **deferred and named** (§3); the pre-existing cross-media **status carriage** difference of §2.2 is likewise named, not closed |
| **new config keys** | **none.** No parameter, key, unit, default or example changes anywhere; the knob's value is untouched |
| a **per-route / per-response override** | **none.** ADR-044 §3's absence (no per-response override, no "long stream" exemption, no separate streaming ceiling) is still an absence |
| the **busy-but-slow throughput guard** | ADR-044 §4's registered row 25 — **registered-open, NOT built** by this round or any card in it |
| a **byte cap** on the failure-head read (R12) | **unchanged.** R12's read carries no byte cap of its own and is bounded exactly as the relay's own reads are; this ADR adds none |
| the **code** | zero bytes under `crates/`, `tests/`, `.github/` **in this card**. The code change is `R71-1`'s; the CONF-90 repair is `R71-2`'s (§8.2) |

---

## 7. Reversibility

**The reversal runs the other way cheaply, and its price is named.** Restoring ADR-044's clock means re-widening the head wrap to a per-request `.timeout` on the streaming send — a small code change plus one design paragraph, one spec clause and one book bullet. **But the two readings are not symmetric**, and that asymmetry is the point:

- the **total-elapsed** reading truncates legitimately long busy streams at a **10× tighter** ceiling, and needs **no guard** to be safe — the ceiling *is* the safety;
- the **head-arrival** reading lets a busy stream run to `request_timeout`, and therefore **needs the outer bound to be real** (§3). Without the enforcement this round lands, the head-arrival reading would leave a trickle bounded by nothing but the idle gap.

So reverting re-tightens the ceiling and keeps the outer bound; adopting widens the ceiling and leans on it. What this ADR fixes is that **both readings are now written down with their bounds named**, so the next reader cannot meet by incident the surprise ADR-044 §3 was written to prevent.

**ADR-044 is not edited away.** It stays on disk, **marked superseded for its clock (i) only** — its status line carries the marker and a dated note closes it — and the **append-only** rule (AGENTS 9's doc map; ADR-012) is respected: not one sentence of ADR-044's own ruling text is rewritten, and its §4 row-25 registration, its §2.4 table, its §5 and its §7 register stand as written.

---

## 8. Register

### 8.1 The supersession of ADR-044's clock (i), clause by clause

| ADR-044 clause | State after this ADR |
|---|---|
| §2.1's ruling (*"the bound is on total elapsed time since the head, not on the gap between bytes"*) | **SUPERSEDED as to the body.** The attempt knob bounds **head arrival**; the body's per-read bound is the **gap** (R4's original letter, restored), and the whole request is bounded by `request_timeout` on the stream path (§3) |
| §1.2's table, rows **A** and **B** | **A is re-scoped** (the head-arrival wrap, `tokio::time::timeout`, isomorphic with `read_chunk`); **B is promoted** from *"real but not the arm that binds first"* to **the body's only per-read bound** |
| §3's blast-radius narrative | **Replaced** by §4 above: the busy-stream ceiling is `request_timeout`, not the attempt knob |
| §4's registered busy-but-slow guard (row **25**, owner: the human) | **Stands — registered-open, not built** |
| §2.4's "does not touch" table, §5, §6's reversibility reasoning, §7's register | **Stand**, read together with this ADR |
| §6's *"its own ADR and its own measurements"* clause | **Discharged** — this ADR is the ADR; §5 is the measurements |

### 8.2 The frozen `CONF-90` repair list (executed by `R71-2`, mechanically)

`tests/conformance/tests/conf_90_giant_single_sse_event_completes.rs` is a **measurement** (AGENTS 9). Its repair is an **owner-authorised act** (§1.2 (c)); the list below is **frozen here** so `R71-2` re-decides nothing. Three edits, each named by site and by what it must say.

| # | Site | Edit | Why |
|---|---|---|---|
| **C90-1** | the module doc-comment (`:1-96`) — specifically the ADR-044 framing at `:2-3` (*"R4 — the idle bound is **not** moved"*), the R58 rationale at `:27-37`, and *"THE NAMED LIMIT"* at `:73-88` | **Reframe: the case now pins *byte-completeness under the gap bound*.** Remove or replace every sentence that reads **death at the total-elapsed bound** as the defect's legitimate signature, and state the new shape: a giant single event completes as long as **no gap exceeds the attempt knob** and the **whole request** stays under `request_timeout`. | the doc-comment *is* the case's own contract; leaving ADR-044's framing would document a bound that no longer exists and would license a run that dies at it |
| **C90-2** | the **calibration machine** — the three-way `enum Calibration { Fast, Marginal, Starved }` and its routing (`:39-88`, `:178-189`, `:339-467`) | **Re-derive against the gap bound, or collapse — pick one, and justify it in the file.** The machine's `Starved`/`Marginal` routes exist because *"limb (b) died at the total-elapsed bound"* was a possible outcome; under the new clock that outcome is **unreachable for a busy stream** (the calibration limb is continuously busy, so the attempt knob can no longer kill it — only `request_timeout`, which the fixture sets to `60s`, could, and it is ten times away). **Recommendation, so the choice is not re-opened: collapse.** The calibration existed to protect the strict limb from a *total-elapsed* death that can no longer occur on a busy fixture; a **two-limb byte-completeness witness** (a giant single event completes; the same total delivered as many events completes) is now a direct assertion and needs no machine-speed calibration. If instead the routes are re-derived, the derivation must state the *new* failure mode it protects against. | the machine's premise moved; a calibration for an unreachable death is dead weight that would otherwise silently weaken the case it was built to protect |
| **C90-3** | the `ATTEMPT_BOUND` constant and its doc-comment (`:164-167`) | **Re-name / re-document its semantics**: it names the **gap (and head)** bound — R4's idle bound — **not** *"ADR-044's total-elapsed bound"*. If `request_timeout` now participates in the fixture's bound story, state it in the fixture config (`:130-160`) and in the constant's own doc-comment. | the constant's doc-comment names a clock this ADR removes; a fixture constant that lies about its own meaning is exactly the drift the registry forbids |

**Constraints on `R71-2` (frozen with the list).** It may edit **only** `conf_90_…rs` — its own case file; **every other case file stays byte-identical**; the frozen corpora's digests are unmoved; no gate definition, threshold or L1-envelope value moves beyond the case's own body; and the round record **lists these three edits** — this §8.2 **is** that list, carried into DESIGN §12.9's register line. `CONF-90`'s **ID is unchanged**: an ID is the contract, a description is not (§12.8's own rule).

### 8.3 New items registered by this ADR

| Item | State |
|---|---|
| **`CONF-105`** | **allocated** to this round's witness — the head-arrival bound, its conservative class, the cross-media class parity and the idle control, in one case (§4 above; the row lands in DESIGN §12.8) |
| **the widened busy-stream ceiling** | **stated**, not a new capability: a busy stream is now bounded by `request_timeout` (§4). It is an identity the operator and the docs must carry, not a feature |
| **the buffered side of `request_timeout`** | **deferred and named** (§3, §6) |
| **the cross-media status carriage** (recorded at §2.2's first writing as buffered `504 upstream_timeout` vs streamed `502 upstream_error` for the same full-write-no-response fault) | **recorded at HEAD, untouched by this ADR's contract** (§2.2, §6) — closing it would move both media. *(Superseded observationally 2026-10-07, §11: the buffered `504` arm is unreachable for the executed rigs' faults — both media in fact answer `502`; the §2.2 row is corrected there and the dead arm is a registered follow-up finding.)* |
| **two documents that cite the superseded reading** | **repaired in this card.** `DESIGN` §12.20's truncation limb (ADR-045's own landing in `DESIGN`) is corrected **in place** — `DESIGN` is a living document — and **ADR-045 §3.5 item 2's parenthetical** gets a **dated note** instead (append-only, ADR-048 §8's precedent). The limb's *rig* was always the **gap** fixture (`"the stub emits units, then declares a gap longer than `server.upstream_attempt_timeout`"`), so the construction and the four assertions are unchanged; only the citation of the total-elapsed reading moved |
| anything else | **nothing.** This ADR carries no new loop finding; `CONF-98`/`CONF-99` and `ADR-050` stay the parked branch `round/67-abandoned-attempt`'s |

---

## 9. What a future reader must not conclude

- **Not** *"the bound was removed."* The attempt knob still bounds **head arrival**, and the body still has a **per-read gap** bound (R4's original letter). What was removed is the attempt knob's *second, wider* application to a busy stream.
- **Not** *"a stream can now run forever."* It is bounded by `server.request_timeout` on the whole request (§3) — the outer bound ADR-011 item 5 always said was there and that this round makes real **on the stream path**.
- **Not** *"the head-arrival arm is a new error class."* It is the **existing** `OpenHead::UnknownOutcome` arm, verbatim: 502, `upstream_error`, `stage: "unknown_outcome"`, `details.error_class: "timeout"`, no retry, no failover. The only movement is the `message` string's provenance (§2.2).
- **Not** *"the connect window changed."* A fast refusal is still a **classified** failure (`connect_failure`, retry/failover eligible, CONF-29's shape), never an Elapsed — the client's own `connect_timeout` fires first for a dead host.
- **Not** *"the calibration in CONF-90 is intact."* Its premise moved; §8.2 freezes the repair, and `R71-2` executes it — the case keeps its ID and its byte-completeness claim, not its old three-way verdict.
- **Not** *"R71-0 changed a product byte."* Zero bytes under `crates/`, `tests/`, `.github/` in this card; the code and the case repair land on `R71-1` and `R71-2` respectively.

---

## 10. Dated correction (2026-10-07, `R71-1` — append-only)

The R71-0b audit (card `t_6907644e`, comment 631; probe rigs at
`~/.hermes/profiles/qa/cache/scratch/probe_head_timeout/`, reqwest
0.12.28 matching `Cargo.lock`) found §2.2's **"today"** map wrong, and
this section corrects it without rewriting a sentence above. Two
claims move, both about the pre-change behaviour this ADR's "before"
column described:

1. **"Today, a head window that expires after a full write already
   lands in this same arm" is false.** A per-request `.timeout()`
   expiry is built by reqwest as `Kind::Request`, so `is_request()`
   is TRUE and `wrote_full_request` (`stream.rs:150`) is FALSE —
   today's streamed slow head lands in
   `Ok(StreamOpen::NotSent(Timeout, …))`, classifies `timeout`
   (`fails_over()` false), and refuses chain-exhausted: **502,
   `upstream_error`, `details.error_class: "timeout"`, NO
   `details.stage`**, one record, `upstream_ms` null. The `Err` →
   `UnknownOutcome` arm was reachable before this round only by a
   non-`Kind::Request` send error, which a deadline expiry never is.
   Consequently "**net behaviour identical to today except the message
   string**" is wrong: after `R71-1` the refusal gains
   `details.stage: "unknown_outcome"` and `result.upstream_ms`; the
   observable delta is **three fields** (stage, upstream_ms, message),
   not one. Status/code/retry/failover are unchanged — also
   conservative before, conservative after; the class word
   `"timeout"` is identical on both sides.

2. **§5 leg 2's before-expectation** ("both before and after —
   §2.2's refusal") cannot be recorded as written: the BEFORE shape is
   the no-stage chain-exhausted 502 above. `R71-1` recorded the honest
   before/after pair on the CONF-105 rig (limb (b)'s streamed leg):
   before — 502, no `stage`, `error_class: "timeout"`; after — 502,
   `stage: "unknown_outcome"`, `error_class: "timeout"`, no
   `failover.triggered`, one record.

The §2.2 cross-media table's buffered row is mis-premised the same
way (a buffered slow head also dies `NotSent(Timeout)` → 502/no
stage); the buffered `504 upstream_timeout` arm is reached only when
the deadline fires during the body read. An executed R71-1 probe of
the recommended mid-body-stall rig observed the buffered carriage as
**502 `upstream_error`** with the shared `stage: "unknown_outcome"` /
`error_class: "timeout"` — the 504 branch requires
`kind == Timeout`, but the buffered body-read failure arm hardcodes
`TransportKind::Other` (`vadis-providers/src/lib.rs:186-189`), so
this fault cannot reach the 504 arm either. The limb-(b) buffered-leg
rig choice is parked with the owner (options in comment 631);
CONF-105's limb (b) is parked `#[ignore]` until that ruling, and
DESIGN §12.8's row is untouched by `R71-1`.

This note is a factual correction of this round's own unlanded text,
authorized by the orchestrator's ruling on R71-0b (2026-10-07) and
recorded for owner ratification in the round record — not a silent
gate edit. Nothing in §2's rulings, §3's contract, or the after-shape
table changes; the after-shape is implemented as written.

## 11. Dated ruling note (2026-10-07, `R71-5` — append-only)

**The owner ruled on limb (b), and it is enabled as authored.** Shown
the executed-rig facts of §10's closing paragraph (a buffered
mid-body stall answers **502** with the shared
`details.stage: "unknown_outcome"` +
`details.error_class: "timeout"`; the 504 `upstream_timeout` arm is
unreachable for this fault), the owner asked for the recommendation
("你的建议是什么？" — "what is your recommendation?"), was shown:
enable limb (b) as authored (it asserts the observed truth and §2.3's
one-fault-one-class-both-media invariant) + a one-line DESIGN §12.8
re-scope + register the 504-unreachability as a follow-up finding for
a future buffered-path round (the `TransportKind::Other` hardcode is
NOT fixed now — the buffered path is out of scope by this round's
boundary). The owner answered: **"确认"** ("confirmed").

**Consequences.** `CONF-105`'s limb (b) is un-ignored and runs green
(4 passed / 0 ignored on the case; the workspace's ignored count
drops 12 → 11). DESIGN §12.8's CONF-105 row now scopes the buffered
leg to the mid-body-stall rig with its observed `502 upstream_error`
carriage. §10's closing sentences ("parked with the owner … parked
`#[ignore]` until that ruling … DESIGN §12.8's row is untouched by
`R71-1`") are superseded by this note. §2.2's cross-media table's
buffered row carries the pointer.

**FOLLOW-UP FINDING (registered).**

- **class:** dead-arm
- **site:** `crates/vadis-providers/src/lib.rs:186-189` (the buffered
  body-read failure arm's hardcoded `TransportKind::Other`), together
  with `vadis-proxy/src/forward.rs`'s `timed_out` branch, which
  requires `kind == Timeout`
- **effect:** on the buffered path today, both the mid-body-stall and
  the slow-head faults answer **502 `upstream_error`**; the 504
  `upstream_timeout` arm is unreachable on the buffered path
- **owner:** human (a buffered-path contract change is not this
  loop's to make — AGENTS 9 / ADR-012)
- **due:** a future buffered-path round (`R72+`)

Fixing it is deliberately out of this round's scope: the buffered
path is untouched by ADR-053's boundary, and the classification
either way is an owner act.
