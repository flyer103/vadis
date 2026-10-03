# ADR-044 — `DESIGN` §12.10.3 R4's bound is **total elapsed time**, not an idle gap: the document yields to the implementation

- Status: accepted
- Date: **2026-09-28** (the owner's ruling; recorded by round R57, whose run is 2026-09-29)
- Kind: **contract repair under an owner ruling.** One sentence of `design/DESIGN.md` §12.10.3 (R4),
  two clauses of `docs/spec.md` §4.2 that name that bound, and one user-facing sentence of
  `book/protocols.md` are amended to say what the code does. **No product byte changes**: nothing
  under `crates/`, `tests/` or `.github/` is in the round's diff, **no timeout value moves, no
  config key is added**, and the code's own behaviour is not edited at all (this decision *is* that
  the code is right).
- The question was registered as the loop state record §"Waiting on human adjudication" **row 24** by
  R54's close-out, on the orchestrator's instruction, from the finding
  **`R54-1-F2`** (first raised as R54-0's own registration, carried re-derived by R54-1).
- Decided against: the measurements of the loop's report (the same file) and
  the loop's report §"Control 2" (the same file), both on 2026-09-28, each on its own
  rig, at the base `8e2cd66` and the tip `6bad197` of `round/54-sse-tap-scan`.
- Related: `AGENTS.md` constraints 1 (the byte boundary — the bound changes no relayed byte), 2
  (content determinism — untouched: the bound is a function of config and traffic, never of content),
  9 (**the measurement is not part of the search space** — hence a ruling, not a round); ADR-011
  (classification) item 6 row 3 (`unknown_outcome`); ADR-012 (the never-mutable paths); ADR-010
  item 4; DESIGN §12.10.3 R2, R4, R6, R12; `docs/spec.md` §4.2, §9.2.

---

## 1. The question this ADR answers

### 1.1 The document's letter, verbatim, as it stood before this ADR

`design/DESIGN.md` §12.10.3 (`:1791-1794` at the ruling's base `826c7e5`):

> **R4 — bounded idle.** After the head is sent, a gap with no upstream bytes longer than
> `server.upstream_attempt_timeout` is a failure: the relay ends (R6), it does not hang. The
> inbound `server.request_timeout` (default 10m) remains the outer bound on the whole request,
> including the stream.

Two properties are asserted there, and they are separable: **(i) which clock** the bound runs on —
"a gap with no upstream bytes", i.e. the silence *between* reads; and **(ii) what happens** when it
expires — the relay ends (R6), it does not hang. This ADR changes (i) and leaves (ii) exactly as it
was.

### 1.2 What the implementation does — one knob, applied twice

`server.upstream_attempt_timeout` reaches the streaming path as **one value** (`crates/router-cli/`
`lib.rs`: `Duration::from_millis(rc.router.server.upstream_attempt_timeout.0)`), and the provider
layer then applies it **twice**, with two different semantics:

| | Site | What it bounds | What a death looks like |
|---|---|---|---|
| **A — the attempt's total time** | `crates/router-providers/src/stream.rs:128`: `out.body(body).timeout(self.idle_timeout).send()` | reqwest's **per-request timeout**: the clock starts when the request is sent (it covers connect, the head and the body) and it ends when the response body is finished. A relay that is *continuously busy* for longer than the knob dies here. | reqwest's own body error — `error decoding response body` |
| **B — the gap between reads** | `crates/router-providers/src/stream.rs:181-197` (`read_chunk`): `tokio::time::timeout(idle_timeout, head.response.chunk())`, on the relay's read path (`router-proxy/src/stream_forward.rs:758` builds it from the same knob) | **the gap**: no upstream byte for longer than the knob. This is the only one the pre-ADR R4 letter describes. | `read_chunk`'s own message — `idle bound exceeded: no upstream bytes within upstream_attempt_timeout`, `timed_out: true` |

The document described **B** alone. The behaviour a client can observe is the **wider** of the two:
**A** also ends a stream that never goes idle at all.

**Which one fires, from the readings rather than from an argument.** Every death the two rigs
recorded carries **A**'s signature (reqwest's body error), never **B**'s own message: the idle arm
dies at **10.005 s** pre-fix / **10.009 s** post-fix with a **byte-identical** verdict string
(`stream truncated before its terminal event; not retried: error decoding response body`,
`error_class: stream_truncated`), and the busy-stream readings fall at **10.005–10.216 s** on the
same knob (10 s in both rigs; the shipped default is 60 s). R54-1's Control 2 states the reading
plainly: *"Which bound fires (reqwest's TOTAL per-request timeout at `stream.rs:128`, same 10 s
knob, firing ahead of the per-read arm) is R54-0's registered finding F-a."*

**A code reading, labelled as one (not a new measurement).** A's clock starts at the request, B's at
the last byte read; whenever the last byte arrives after the request was sent, A's deadline is the
earlier of the two. That orders A before B for every stream whose head or first event arrived at all,
which is why neither rig ever saw B's message. **Whether B is reachable at all is not claimed here** —
that would be a new measurement, and this ADR takes none. What the ruling fixes is that the document
must state the bound the reader can actually meet.

---

## 2. The ruling

### 2.1 The ruling, in the owner's words

> **The document yields to the implementation.** R4's sentence is amended to state the semantics the
> code has: the bound is on **total elapsed time since the head**, not on the gap between bytes.
> **No product code changes.**

### 2.2 Its provenance, in the words the ruling must carry

The ruling is the **owner's**, made on **the orchestrator's 2026-09-28 recommendation, following the
round that surfaced the divergence (`R54-1-F2`)**. The loop did not decide it: R54's close-out
**registered** it as row 24 with both branches of the question and *"Owner: the human"*, precisely
because the sentence lives in `design/`, which AGENTS 9 / ADR-012 put outside any round's reach.
This ADR records the owner's decision; it does not take one.

### 2.3 Why the code's reading is the safer of the two

- **An idle-gap-only bound is unbounded in wall-clock.** A stream that carries one byte per
  `upstream_attempt_timeout − ε` never trips a gap bound and may therefore hold a connection, a
  task and an upstream slot **indefinitely**. For a gateway whose whole job is to hold a
  client→upstream seam open, that is a resource risk with no floor under it — and the client on the
  other end of a hung relay is worse off than one that receives a truncated stream it can see.
- **The wider reading cannot silently lose a *completed* response.** It truncates early, loudly and
  observably: the stream ends without the protocol's terminal marker, the trace carries
  `error_class: stream_truncated` in `errors[]`, and no cost is invented for what did not arrive
  (R8's usage honesty and ADR-010 item 4 are untouched by this ADR).
- **Choosing the letter would have meant changing the code** to a *weaker* bound — and the code's
  own reading is the one the two rigs measured on real bytes. Between "make the code match an
  under-specified sentence" and "say what the code does", the second is the smaller, safer change.

### 2.4 What this ADR supersedes, and what it does **not** touch

**Supersedes:** the **letter (i)** of R4 as quoted in §1.1 — the phrase *"a gap with no upstream
bytes"* as the definition of the bound. That sentence is amended in place
(`design/DESIGN.md` §12.10.3 R4), with this ADR as its authority.

**Does not touch — named explicitly, because a reader will ask:**

| Untouched | Where it stands |
|---|---|
| **The knob's value** | `server.upstream_attempt_timeout` keeps its configured value (shipped default **60 s**). No value, unit, default or example changes anywhere; this round's diff contains no `timeout` value at all. |
| **The bound's duration** | Still exactly `server.upstream_attempt_timeout`. Nothing is widened, shortened or added: not one parameter, key or constant. |
| **`server.request_timeout`** | Still the outer bound on the whole request, including the stream (R4's own second sentence, kept verbatim). |
| **The read path's semantics (R6, R12)** | R6's two columns are unchanged: before our head a failure is the ordinary classified path; after the first event failover is impossible and the stream ends without a fabricated terminal event. R12's failure-head read is unchanged — including its *"no byte cap of its own"* clause. |
| **Soft / failover behaviour and the attempt budgets** | ADR-011's classification, the retry/failover walk and every provider/attempt budget are untouched: a stream that dies at the bound is `unknown_outcome` after a full write (ADR-011 item 6 row 3) exactly as before, and the plan's own budgets keep their values. |
| **The code** | Zero bytes under `crates/`, `tests/`, `.github/`. The per-read arm (B) is left in place, unedited, as the relay's own read bound. |
| **The measurement apparatus** | No gate definition, corpus digest, conformance assertion or L1-envelope value moves (AGENTS 9 / ADR-012). |

---

## 3. The blast radius, stated plainly

This is the part a reader must not have to discover by incident.

**With the total-elapsed semantics, a legitimately long single response is capped by the same knob.**
A long **reasoning stream** is the obvious case: a model that emits a stream of reasoning tokens for
minutes is *continuously busy* and never idle, and it is bounded all the same — at
`server.upstream_attempt_timeout` from the moment the attempt was sent, not from the last byte. The
reader-visible consequences, in the order a client meets them:

1. the client receives a **truncated stream** (a strict prefix of the events the upstream would have
   sent) and, for chat completions, **no `[DONE]`** — R6's standing rule: router never presents a
   truncated stream as complete;
2. the trace records the failure (`errors[]` with `error_class: stream_truncated`) and the usage it
   actually saw (R8: no carrier ⇒ `usage_missing`, no invented cost);
3. the request is **not retried and cannot fail over** — the client's output is already committed
   (R6 column 2), and after a full write it is the `unknown_outcome` of ADR-011 item 6 row 3;
4. **the fix at hand is operational, not architectural:** configure a larger
   `server.upstream_attempt_timeout` (or stop relaying that stream through this gateway). There is no
   per-response override, no "long stream" exemption and no separate streaming ceiling today —
   naming that absence is the point of the next section.

**Scope of the risk, stated with its bound.** The truncation is a *time* bound on one attempt, not a
size bound: the same knob that catches a stall catches a slow-but-endless stream. A client that
needs an unbounded stream today cannot get one from this gateway — and it did not get one before this
ADR either (the behaviour is pre-existing and **unchanged by R54**, whose `crates/` diff contains
zero occurrences of `timeout` and does not touch `stream.rs`). What changes here is that the
document now says so.

---

## 4. The gap the divergence exposed, and where it is registered

The genuine gap is **not** the divergence between letter and code — that one is closed by this ADR.
It is that **nothing guards "continuously busy but extremely slow"**: a trickle whose every gap is
shorter than the knob and whose total is longer than it. Today that case is handled by *the same
knob doing double duty*: the total-elapsed bound truncates it (good), but it does so at the same
ceiling as every other stream, with no distinction between "a fast stream that just runs long" and
"a stream that is really a stall wearing a trickle's clothes".

Distinguishing the two needs a **new contract** — a new parameter (a minimum-throughput floor, a
separate streaming ceiling, or a per-route override) — and a new parameter is an owner-level act,
not a loop outcome. It is therefore **registered, not built**: a new row in
the loop state record §"Waiting on human adjudication" (row **25**, owner: the human), stating the
question with both branches. Until it is decided, **the total-elapsed bound is the whole contract**
and no card may add a guard of its own.

### 4.1 The risk accepted, and its mitigation

- **Accepted:** a busy-but-slow stream is truncated at the same ceiling as a stalled one, and a
  long reasoning stream therefore needs an operator-raised `server.upstream_attempt_timeout` to
  complete.
- **Mitigation in force today:** the truncation is *observable* (a strict prefix, no terminal
  marker, `errors[]` + `error_class`, `usage_missing` where no carrier arrived) and *configurable*
  (one key, no code change), and every record that describes it is honest about what it does not
  know.
- **Mitigation owed:** the registered row 25 guard. It is not a defect of this ADR; it is the
  capability this ADR declines to invent.

---

## 5. What a future reader must not conclude

- **Not** *"the bound is a stall detector"*. It is an attempt-length bound. A stream can die at it
  without a single idle moment.
- **Not** *"the per-read arm (`read_chunk`) is the bound"*. It is real code on the relay's read path
  and it is **left running**, but the readings never show it firing, and the total-elapsed arm is the
  earlier deadline whenever a byte has arrived at all. The code's own comments still call the bound
  an "idle bound" (`crates/router-providers/src/stream.rs:33`, `:70`, `:90-91`, `:178`;
  `router-proxy/src/stream_forward.rs:9`) — those are `crates/` bytes, deliberately untouched by this
  docs-only round, and named here so the vocabulary mismatch is **recorded rather than discovered**.
- **Not** *"R4 was rewritten wholesale"*. One sentence's definition of the clock changed; the
  consequence (*the relay ends, it does not hang*), the outer `server.request_timeout` sentence and
  the R1–R3/R5–R6/R12 rules around it are byte-identical to their pre-ADR state.
- **Not** *"the reading in §1.2 is a measurement"*. §1.2's clock ordering is a **code reading**; the
  only measurements cited are the two rigs' elapsed times and verdict strings, quoted from their own
  reports.

## 6. Reversibility

**Cheap, and its price is named.** Reverting this decision means restoring one sentence of
`DESIGN` §12.10.3 and two clauses of `docs/spec.md` §4.2 (and one `book/` sentence) — a docs-only
diff of a few lines — **plus** the code change the letter would then demand: removing or re-scoping
reqwest's per-request `.timeout(...)` at `crates/router-providers/src/stream.rs:128` so that a busy
stream is no longer capped. That code change is **not** small in consequence: it re-opens the
unbounded-relay risk of §2.3 and would need its own ADR and its own measurements. So the decision is
**reversible in text, costly in behaviour**, and that asymmetry is the reason it is recorded as the
owner's rather than as a loop's tidy-up.

## 7. Register (this ADR carries no new loop finding)

| Item | State |
|---|---|
| `R54-1-F2` (carried from R54-0's own registration) | **CLOSED by this ADR** — the divergence between R4's letter and the code is resolved in the code's favour, with the letter amended. |
| `R54-1-F1` (case sensitivity of `CONF-90` to the cursor limb) | Untouched by this round; owner `registry owner`, due the next conformance-registry pass. |
| `R54-1-F3` (DESIGN §12.8 has no `CONF-90` row; its header reads `CONF-01…CONF-89`) | Untouched by this round; same owner and due. |
| **New: the busy-but-slow guard** | **Registered, not built** — the loop state record waiting-on-human **row 25**, owner the human (§4 above). |
| The code's "idle bound" comments and the neighbouring documents that still name R4's bound by its old label (`DESIGN` §12.8's `CONF-82` cell, §12.10.3 R12, §12.10.5's note; `docs/spec.md`'s read clause) | **Left as written.** They cite **R4**, whose rule now covers both applications of the knob, so none of them is false; this ADR names them so a future docs pass can align the vocabulary if it wants to. `crates/` comments are outside this round's write set by construction. |

## Publication note (2026-10-03, R62-2)

The `R<n>` labels and finding ids in this document name iterations of the project's own
private analysis loop — a loop that is not part of this repository, so no label here is
resolvable by a reader of it; they are kept as the provenance of the decision. This
publication pass removed only the dead-pointer class: every reference into that loop's
working tree (its file paths and round-record names, its state record, charter, execution
model and replay contract, its scripts and module names, and the kanban card ids), each
replaced by the neutral phrase the sentence needs. Nothing else moved — no figure,
threshold, `§`/`ADR`/`CONF` id, code sample or contract sentence.
