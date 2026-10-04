# ADR-045 — the citable number: the evidence contract, and the streaming fidelity instrument's definitions (TTFT, the **added** inter-chunk jitter, chunk fidelity as an assertion)

- Status: accepted (the **definitions**; the instrument itself is R59-1's, the verification R59-2's, the landing R59-3's)
- Date: 2026-09-29 (round R59's contract card)
- Kind: **a measurement definition for an instrument that gates nothing** — plus the rule that makes a
  figure *citable*. It adds **no gate**, changes **no threshold**, and touches **no product byte**: nothing
  under `crates/`, `tests/conformance/` or `.github/` is in the round's diff, `TRACE_SCHEMA_VERSION` and
  `EVENT_SCHEMA_VERSION` stay 2, and no price, corpus digest, conformance assertion or L1-envelope value
  moves. Whether these observables ever become a **blocking gate** is *not decided here* and stays exactly
  where ADR-036 §"What this ADR does not decide" put it: the owner's.
- Related: `AGENTS.md` constraints **1** (the byte boundary), **2** (content determinism), **3** (the
  observation boundary), **4** (no unverified savings), **5** (no fabricated prices), **8** (docs before
  code), **9** (the measurement is not part of the search space); **ADR-012** (the mutable scope);
  **ADR-007** (span-faithful forwarding) and **ADR-015** (the two permitted mutations); **ADR-028**/**ADR-030**
  (harness-only observation; the latency measurement method and its citation rule); **ADR-029** D1/D3 (the
  quantity, and *"a figure that does not name its machine is not citable"*); **ADR-036** D3 + §283 (the
  `Observer` surface, and the gate question); **ADR-042** (the exact-match cache — a hit has no upstream);
  **ADR-044** (the streaming bound's semantics, which the truncation legs must match); **DESIGN** §12.10.3
  (**R1** no re-framing, **R2** write-through, **R4** the bound, **R6** mid-stream failure), §12.16 (where a
  baseline's numbers live), §12.23 (**the definitions' contract home, added by this round**), §13.6
  (the `Observer` row — `:4137` as this round's card cites it, `:4202` once §12.23's insertion below moved
  it); `docs/spec.md` §6 (the observation contract), §7 (the labels), §9.2/§9.3
  (reporting surfaces); `tests/conformance/tests/conf_13_sse_passthrough.rs` (what the suite asserts today);
  the loop's result record + the loop's result record (the
  counterexample); the loop's qa3 rig script (its reducer); the loop's records for that decision (this
  round's evidence); the loop state record row 18 (why the rule lands here and not in the loop charter).
- Numbering note: **ADR-044** landed with R57. This is the next free number in `design/decisions/`.

---

## 1. The question, and the honest provenance

### 1.1 What the repository claims today, and what it can show

The byte boundary is this project's wedge, and it is asserted in prose on **both** directions of the wire:

- the **request** direction is asserted *mechanically*: `AGENTS.md` constraint 1's conformance tests compare
  the upstream-visible prefix hash with the client's, modulo the two permitted span mutations (ADR-007,
  ADR-015, `CONF-01`/`CONF-10`/`CONF-27`);
- the **response** direction is asserted only where a test happens to look: `CONF-13` compares the client's
  **de-chunked event byte sequence** with the mock upstream's, event by event, and its own comment says what
  it declines to compare — *"not chunk framing, which the router may legitimately re-frame at the HTTP
  layer"*. `README.md:322`, `book/roadmap.md:12-13` and `design/DESIGN.md:4000` (P1's row) state the claim in
  prose.

So the honest statement of the gap — the 55-round review's **T1-B** — is this: *the response direction's
byte promise has an in-process conformance assertion over one canned body and **no instrument and no
numbers**.* A number is the thing a third party can check; a prose sentence is not.

### 1.2 Where the names came from: the contract named them, and the round that was to measure them never ran

This is the round's provenance, stated plainly rather than dressed up:

- `design/DESIGN.md:4137` (§13.6, the minimal-core boundary) says of the `Observer` surface: *"yes, and
  first … R41-4's moat measurement (inter-chunk jitter, chunk fidelity) is the first plugin"*;
  `ADR-036` D3 (`:110`) and its roadmap (`:216`) say the same; `DESIGN:338` lists R41-4 beside R41-3 as the
  round that would first bind a service key.
- **R41-4 does not exist.** The round-record directory holds **R41-0 … R41-3** and nothing else. The design cites a
  measurement that was never run.
- The gap was already registered, by the round that had to stop: **`R41-1-F5`** — *"the moat observable
  (client-observed chunk-interval jitter; chunk fidelity with a recorder mediating) has **no number anywhere
  in the repo**; whether it becomes a blocking gate is a measurement definition"* — owner **`human`**, due
  *"when R41-4 lands the observer"*.

Two consequences the round must carry with it, and neither is optional:

1. **The names are the contract's; the definitions are not.** "Inter-chunk jitter" and "chunk fidelity" are
   in the design as *labels*. A label is not a definition: two people can measure a different quantity under
   the same name, and this repository has already paid for exactly that (§1.3). This ADR supplies the
   definitions. It does not claim to be R41-4, and it does not close `R41-1-F5`'s gate half.
2. **The design's forward reference is repaired where this round may write, and named as residue where it may
   not.** `DESIGN:4137` and `DESIGN:338` are amended to name the truth (definitions in §12.23; the measurement
   route decided in §4 below; R41-4 unrun). `ADR-036:110` and `:216` carry the same forward reference and are
   **outside this round's write set** — they are named here as residue, not silently left to look current.

### 1.3 The second missing measurement (T1-C): why no figure in this repository is recomputable today

The repository already has a citation *rule*, as prose in one round file: R32's *"a figure that does not name
its machine is not citable"*. What it has never had is a rule about the figure's **carrier**. The
counterexample, measured (this round's own evidence, the loop's evidence for that decision,
produced by `counterexample-evidence.sh`):

| fact | reading |
|---|---|
| R32-3's aggregate for the chat-streaming C=32 cell (`C22`) | the loop's result record — **tracked**; `router_overhead_ms: {p50 3, p95 16, p99 23, max 24, n 220}` |
| R32-2's aggregate for the *same* cell | the loop's result record — **tracked**; `{p50 2, p95 7, p99 9, max 13, n 220}` |
| the 220 per-request records the 23 ms was reduced from | the loop captured trace for that run — **present on the machine that ran it, absent from the repository**: `git ls-files … /state \| wc -l` = **0**, and git says why: `.gitignore:29:state/` |
| the reducer | the loop's qa3 rig script — tracked, and it *is* a pure function of a trace directory (`scan_trace`, the same file) … but its CLI is `rung`/`ladder`/`summary` only, and `summary` re-reads **`*/result.json`** (the same file), i.e. the aggregate. **There is no published command that turns a raw into the figure.** |

**The finding, in one sentence.** The two readings disagree by 14 ms (9 vs 23) on the same cell, the
repository carries neither the evidence that produced either number nor a command that could re-derive them,
and no card may re-adjudicate R32's carve-out as a side effect — so the pair is `NOT established` and
**unrecomputable**, and any citation of it is a citation of a *claim about* a measurement rather than of a
measurement.

---

## 2. D1 — The citable number: the evidence contract

**A figure a report or the book shows is citable iff all four of these hold.** They are clauses on
*documents*, not on gates: nothing here is an operand of any gate, and the rule adds no threshold.

1. **A raw artifact exists under a `tracked` path.** What the figure was reduced *from*, not a copy of the
   figure: the per-request records, the per-chunk arrival and emission logs, the run's own config and
   command output. `.gitignore` is not a place a citable raw may live in. The tracked home follows
   the loop execution model's evidence-homing rule (R46) and the landed rounds' own precedent
   (the loop's records for that decision) — never `state/` (`.gitignore:29`), never the loop's results tree
   (`.gitignore:13`), never the loop's capture tree, never the corpus.
2. **The exact command that produced it.** Copy-pasteable from a fresh clone, with every argument that
   matters (the build, the flags, the stimulus, the caps). Not "the rig was run", not an ellipsis.
3. **The reducer.** The code that turns the committed raw into the figure — committed, and **runnable
   offline against the raw**, with no measurement, no network and no rerun. A reducer that only exists inside
   the run that produced the raw does not satisfy this clause (that is exactly §1.3's defect).
4. **The machine and the commit** — CPU model and core count, RAM, OS, and the binary's own commit. This is
   ADR-029 D3's clause, kept verbatim in force: *a figure that does not name its machine is not citable*.

**The acid test, which is the definition of done.** *A third party, given only the repository, reproduces the
figure* — by running the published reducer over the committed raw, **without re-running the measurement**.
Anything weaker (re-running it and getting "about the same") is a *reproduction of the phenomenon*, not of
the figure, and the two are not the same thing: R32's two rigs ran the same ladder and read **9 ms** and
**23 ms** on the same cell, and neither number can now be reduced from anything the repository holds (§1.3).

**What the rule is not, stated so it cannot be read as more than it is:**

- **It is not a gate, and it implies none.** No verdict, no threshold, no operand. A round's gate conclusion
  is unchanged by whether its figures are citable; what changes is that a citable figure may be *quoted* by
  an outsider.
- **It mints no label.** The two labels row 18's W1 dossier proposes (`loop-local`, a caveat-carrying
  `verified`) are **not** created here, and nothing is relabelled. The `verified`/`inferred` pair stays exactly
  what `AGENTS.md` constraint 4 and spec §7 say it is — the *saving* convention. A latency or fidelity figure
  is neither: it is a measurement of the gateway's own behaviour, and it must not be smuggled into either
  label.
- **It does not land in the loop charter.** That file is a **gate definition**, and `AGENTS.md` 9 /
  ADR-012 put gate definitions outside a round's reach. The loop state record row 18 records the same class of
  text (`"Evidence reproducibility"`, the W1 dossier's second hunk) as **owner adjudication required; no round
  may land this**. The rule above is its *documentation-side* half: it binds what a report or the book may
  show, it adds no operand to any gate, and it therefore can be landed. If the owner ever adopts the
  the loop charter sub-section, this ADR's clauses are the text it should agree with — or the divergence must be
  stated, not discovered.
- **It does not retroactively invalidate landed rounds.** A figure already labelled and recorded stays
  recorded; it becomes **non-citable**, not false. The counterexample of §1.3 is the shape: the pair is
  `NOT established` and stays there.

---

## 3. D2 — The three metrics, defined

The definitions live as contract text in **`design/DESIGN.md` §12.23** (the same home ADR-029's method has in
§12.16, and for the same reason: a number whose definition is only prose in a report cannot be cited). This
section is their argument; §12.23 is their text. **They apply to the passthrough path with the response cache
off — its default — and nowhere else** (see §3.6).

### 3.1 The stimulus, and the null baseline every figure is reported against

The instrument is a **declared-stimulus** rig, and the declaration is the whole method:

- a **harness-owned loopback stub** that emits a known sequence of chunks — known **boundaries** (one SSE
  event per emission unit), known **bytes per unit**, and known **emission instants** — with a declared
  inter-unit pause. The stub records its own emission instants: they are the reference clock for every
  *added* quantity below, and they are available precisely because the stub is ours;
- a **raw-socket client** that timestamps the arrival of every delivered byte, so the client's own clock is
  the ruler and the HTTP framing is visible to the instrument rather than normalised away by a library;
- a **null baseline**: the same stimulus, the same client, the **router out of the path** (client → stub
  directly), reporting the harness's own floor. Every figure is published beside its baseline. A figure
  without its floor is a figure whose ruler is unknown.
- `$0.00`, offline: **loopback only, no provider dialled, no credential read** (the same class as
  the loop's records for that decision's stand-ins, ADR-029 D3).

### 3.2 TTFT — from the client's clock, as a distribution, with its confound named

- **The clock, named.** TTFT is *the client's arrival instant of the first delivered body byte*, measured
  from the instant the request was written — the quantity a user waits for. The stub's emission instant of
  its first unit is the **reference**, so the round reports two figures and never one:
  - **`ttft`** (absolute; includes the stub's own emission schedule and the router's work);
  - **`ttft_added` = `ttft` − (stub's first-unit emission instant)** — what the router added.
  **`ttft_added` is the citable one**, for the same reason the jitter metric below is a difference.
- **Single value or distribution: a distribution, always.** A single TTFT is an anecdote. The instrument
  reports `p50` and `p99` over **N streams**, and — per ADR-029 D3's sample rule — a `p99` needs **N ≥ 100**
  streams to be a `p99`; below that the instrument publishes `p50` and `max` and marks the `p99`
  **`NOT established`** rather than printing a number that is really a maximum.
- **The confound, stated rather than footnoted:** the harness's own scheduling. The client's wake-up after
  the first byte, the stub's own emission jitter and the OS's scheduler all sit inside the measurement, and
  none of them is the vadis. The null baseline (§3.1) is what bounds them, and the instrument must report
  the baseline's own spread — a router `p99` that is smaller than the harness floor's spread is not a
  finding about the router.

### 3.3 The added inter-chunk jitter — a difference, per chunk, at p50 and p99

**Definition.** For each pair of consecutive emission units *k* and *k+1*:

```
jitter_added[k] = (client arrival(k+1) − client arrival(k))      # the client-observed gap
                − (stub emission(k+1) − stub emission(k))        # the stub's own emitted gap
```

aggregated over all gaps of a run and over all runs, reported as **`jitter_added_ms` p50 and p99** (`max`
beside them), with **N**, the declared pause, and the baseline's same reading.

**Why the difference and not the raw gap — the argument, not a preference.** In a rig the stub *is* the
upstream, and the stub's own pacing is part of the stimulus, not part of the gateway. A raw inter-chunk gap
would measure the stub's emission schedule plus the client's scheduler plus the OS, and the router's
contribution would be a rounding error inside them; that is a number about the harness wearing the router's
name. The **null baseline** (router out of the path) is exactly the demonstration: it is the same raw gap
with nothing in the middle, so the difference is the only quantity that can be *attributed* to the relay.
The same argument is why the figure is µs, not ms: R32's integer-millisecond grid is what turned a 1 ms vs
2 ms read of the same rung into two rounds of adjudication, and a `p99` of 3 ms on a 1 ms grid has three
distinct possible values.

### 3.4 Chunk fidelity — an **assertion**, not a statistic

The assertion is a three-limb predicate over the delivered sequence. It is the measurable form of
`AGENTS.md` constraint 1's byte boundary applied to the direction the constraint's own conformance tests do
not cover: constraint 1 is written about the **request** the upstream sees (the upstream-visible prefix hash
equals the client's, modulo the two mutations), and this is its **response-direction mirror** — the bytes the
client receives equal the bytes the stub emitted, modulo nothing at all.

- **F1 — content, order, termination (must hold).** The de-framed client byte stream equals the stub's
  emitted concatenation, exactly: every byte, in order, `[DONE]` (or the wire's own terminal carrier) last,
  and **nothing appended after it**. This is `CONF-13`'s property, promoted from an in-suite case over one
  canned body to an out-of-process assertion that also carries numbers.
- **F2 — event boundaries (must hold).** The client's SSE **event** sequence equals the emitted sequence,
  event for event: same count, same bytes per event, same order — **no merged event, no split event, no
  reordered event, no empty event inserted**. (`DESIGN` §12.10.3 **R1** is the contract this limb asserts:
  *"does not merge or split events to a preferred size"*.)
- **F3 — emission-unit boundaries.** The client's observed **unit** sequence equals the stub's emitted unit
  sequence: same count, same byte span per unit, same order, **no coalescing** (two units delivered as one),
  **no splitting** (one unit delivered as two), **no empty unit invented** before termination. This is the
  limb a raw-socket client can see and a de-chunking client library cannot, and it is the limb `CONF-13`
  explicitly declines (*"the router may legitimately re-frame at the HTTP layer"*).
  **The tension is recorded, not papered over:** the *design* promises it — §12.10.3 **R2** (*"each read is
  written to the client as soon as it is available"*) and the relay's own comment (*"one item per upstream
  chunk, verbatim"*) — while the *suite* reserves it. R59 asserts F3 (it is the strongest form of the claim
  the round exists to make checkable) and, when F3 fails, the finding is against the **contract or the
  implementation**, decided by a later round and never silently absorbed: **whether the product formally
  promises transport-level unit preservation is registered for the owner** (§6, and the loop state record row 26), and
  R59 mints no product promise.

A fidelity **failure** is not a figure: it is a verdict with the first offending index, the emitted span and
the delivered span, so a reader can see *which* byte was reframed without re-running anything.

### 3.5 Termination and error paths — where a relay is most likely to re-frame

An instrument that covered only the happy path would bless exactly the cases where the byte boundary is
under pressure. The instrument asserts on three endings, and the shared invariant is: **the client's received
stream is a prefix of the emitted stream, and the router never authors an ending the stub did not send.**

1. **Normal end.** The stub emits its terminal carrier and closes. F1/F2/F3 hold, the terminal unit is last,
   nothing is appended. The increment in run time is itself reported (a terminal event held back and
   flushed late is a jitter finding, not a fidelity one).
2. **Truncation by the bound (ADR-044).** The stub emits units, then declares a gap longer than
   `server.upstream_attempt_timeout`. The assertions: (i) the delivered bytes are a **strict prefix** of the
   emitted ones — no partial unit completed with invented bytes; (ii) **no terminal carrier is synthesized**
   (the classic re-frame: the proxy closes the body by emitting `[DONE]` the stub never sent); (iii) the
   truncation is declared in the record (`errors[]` with `error_class: stream_truncated`, ADR-044/R6's
   semantics, on the **total-elapsed** reading ADR-044 settled — *this* is the bound the leg must be written
   against, not the old "idle gap" letter); (iv) the relay **ends**; it does not hang.
3. **Upstream error mid-stream.** The stub emits a partial sequence then fails the connection (or emits an
   error frame and closes). Same prefix and no-synthesis assertions, plus: the classification is declared per
   ADR-011's taxonomy, **no failover happens after bytes have reached the client** (R6: the boundary is our
   own head), and exactly one client-visible outcome exists — never a normal completion with a
   mid-stream error hidden inside it, and never a second attempt's bytes appended to the first's.

A client-side disconnect leg (§12.10.3 **R5**) is **not** part of this definition: that path is about
cancellation, not about the delivered sequence, and it is R59-1's option rather than a limb of fidelity.

### 3.6 The cache interaction — where fidelity is *undefined*, said out loud

An **exact-match cache hit has no upstream** (`ADR-042`: the hit removes the upstream call by design). With
no emitted sequence there is no counterpart to compare, and with no upstream gap the added jitter is not a
small number — it is **meaningless**. So:

- the definitions apply to the **passthrough path, with `builtin/response_cache` off** — which is its
  documented default (ADR-042, spec §4.17);
- the instrument **refuses** a sample whose record shows a cache hit, and says so, rather than folding a
  hit's timing into a jitter number. A run that silently mixed its paths would produce the one thing this
  round exists to prevent: a figure whose provenance the reader has to guess.

### 3.7 Resolution, repetition, band — borrowed from the method that already paid for them

ADR-030 D3's rules are adopted **as written** for these metrics, because they were written by a round that
had just been burned by their absence: **(i) resolution declared** (µs here; a `0` is reported `<1 µs` and
is never a ratio's denominator); **(ii) R ≥ 3 repetitions of the load-bearing configuration, reported as a
band `[min, max]`** — an absolute band wider than the published tolerance is reported as its band and marked
`NOT established`; **(iii) a derived difference is citable only outside the union of the two bands**,
otherwise *"within the declared band — NOT established"*; **(iv) the machine, the commit and the stimulus are
part of every figure** (ADR-029 D3, and §2 clause 4 above). R = 1 is permitted only where the measurement is
a binary admission fact (a fidelity verdict) and the report says so.

### 3.8 What each number does **not** license

Written into the definitions, because this is where a measurement becomes a marketing claim:

- **No comparative claim of any kind.** Not against another gateway, not against a previous design, not
  between two of this round's own cells. A figure is *this machine, this commit, this stub, this stimulus,
  this N* — and nothing about a load, a protocol cell, a payload shape, a real provider or a user's traffic.
- **`jitter_added` licenses one sentence:** *on this declared stimulus, this build added at most X µs at p99
  between consecutive units.* It is not a throughput claim, not a TTFT claim, and not a claim about a stream
  under concurrency — the declared stimulus is a **single stream**, and a concurrency figure is a different
  measurement class (ADR-029/ADR-030's load ladder), which this round does not run.
- **The fidelity verdict licenses exactly one sentence:** *on this stimulus, this build's delivered sequence
  equaled the emitted one, limb by limb.* It says nothing about other protocols' cells beyond the ones the
  stub can emit, nothing about translated paths (there are none — the translation column is empty), and
  nothing about behaviour under backpressure, which the stimulus does not create.
- **Neither metric licenses a `verified`/`inferred` label** (constraint 4, spec §7): those labels belong to
  the *saving* convention and their use here would be a category error.
- **An absent number is not a zero** — the same rule spec §7 states for missing usage applies to a metric the
  instrument refused to produce: the report says which refusal fired.

---

## 4. D3 — The route decision: measure from **outside**, and what the `Observer` route would add

`DESIGN:4137` says the moat measurement is *"the first plugin"* — an `Observer`, product-side. **R59 does not
take that route**, and the reason is a decision, not an accident:

1. **It would be product bytes.** The constraints binding this round are zero bytes under `crates/`,
   `tests/conformance/` and `.github/`; an `Observer` is a component in the serving path, its trait and its
   service key are product contracts, and it can only run on a build that contains it.
2. **An in-process observer perturbs what it measures.** Its own scheduling, its service-table lookups and
   its effect stack sit inside the latency being measured; the outside client's clock does not.
3. **Its evidence would rest on the subject's cooperation.** This repository has already ruled on that shape:
   the loop state record **row 13** (the L4 byte-audits) chose the **harness-only** option, refusing the product-side tap
   precisely because *"the instrument's most load-bearing check would rest on the subject's cooperation"*
   (AGENTS 3). A fidelity verdict produced by the component under test is that shape.
4. **It could not measure an arbitrary build.** The outside rig works on any commit's binary — which is what
   makes a figure recomputable by an outsider (§2's acid test).

**What the `Observer` route would add — kept on the record so the choice is visible rather than implied:**
in-process visibility that no client can have: the **pre-transport bytes** (what the relay handed the
transport, before TLS/socket framing), **per-chunk attribution** to a pipeline stage, and the upstream's
arrival instants read on the process's own clock instead of inferred across a client boundary. Those are real
capabilities, and they are exactly the ones a *future* round would want if a fidelity failure ever needs to be
localised inside the process rather than observed at the client.

**The route therefore stays open, and stays the owner's.** ADR-036 already holds the question (*"whether the
moat observable becomes a blocking gate"*), and its `Observer` row is unamended. R59 takes the outside route,
records why, and **adds no gate, no plugin, no key and no product byte** — the instrument is a harness rig.

---

## 5. D4 — The one figure R59 makes citable, and **supersede**, not reconcile

**The designated figure.** R59's instrument makes **the added inter-chunk jitter at p99 on the declared
stimulus** — `jitter_added_ms.p99`, with `ttft_added` (p50/p99) and the F1/F2/F3 verdicts beside it, the
bench's own command and reducer committed next to the raw under the loop's evidence for that decision, the machine and
the commit named in the report — the round's **one citable latency figure**, and the first figure in this
repository produced under §2's rule end to end (raw in a tracked path · command · offline reducer · machine +
commit). R59-1 produces it; R59-2 verifies it by running the reducer over the committed raw in a fresh
clone; R59-3 lands it.

**And the ruling on R32's pair, in one sentence: R59 *supersedes* it — it does not reconcile it.** R32's
`C22` pair (9 ms and 23 ms, §1.3) stays exactly as R32's own record left it, `NOT established`; R59 claims no
cause for the disagreement, re-adjudicates nothing, and re-mints **no** `router_overhead_ms` reading of that
cell. The reason is stated so a later round can check it rather than trust it: reconciliation would require
either the two rigs' per-request raws (gitignored — §1.3) or a *third* reading that would itself need the
rule to be citable, and a third reading would reproduce the defect the round exists to end — three numbers,
none recomputable, none explaining the others. **What R59 does instead is make the *streaming* figure citable
by measuring a quantity whose stimulus is declared and whose raw is committed**, and it says in the same
breath that this is a **replacement**, not a resolution. *If the owner wants R32's pair adjudicated, the act
is a measurement: re-run that rung with a published command and a committed raw. That is owner-level, and it
is named here rather than taken as a side effect.*

---

## 6. What this ADR does not decide

- **Whether these observables become a blocking gate.** Unchanged and untouched: ADR-036's own words,
  *"that is a measurement definition, outside the loop's mutable scope (AGENTS 9, ADR-012)"*. R59 wires no
  gate, implies none, and would have to hand the definitions to the owner for signature before they could
  ever be an operand of one.
- **Whether the product formally promises transport-level unit preservation** (F3, §3.4). Today the *design*
  promises it (§12.10.3 R1/R2, the relay's own comment) while the *suite* reserves it (`CONF-13`'s comment).
  R59 asserts it from outside and mints no product promise; making it a formal promise — and with it a
  conformance case that withdraws `CONF-13`'s reserved freedom — is a `design/`+`crates/`-side contract act
  and is registered as the loop state record **row 26**.
- **The `Observer` plugin route** (§4). Open, unchanged, ADR-036's.
- **Adjudicating R32's `C22` pair** (§5). Superseded, not reconciled; the cause of the disagreement stays
  unknown, and the owner may commission the measurement.
- **`R41-1-F5`'s registration.** Its *gate* half is the owner's and stays open; R59 supplies the definitions
  that registration said were missing, and does not close it.
- **Any label vocabulary** (§2). `loop-local` and a caveat-carrying `verified` are row 18's; not minted here.
- **Anything about `R41-4`**: the round was never run, and this ADR neither runs it nor retires its
  intentions. `DESIGN:338`/`:4137` are amended to say so; `ADR-036:110`/`:216` are named as residue.

## 7. Reversibility

- The **definitions** are reversible: they are documentation of a method; changing one is a new ADR, and the
  numbers produced under the old text remain valid *as figures of that text* — which is why each figure's
  report names the definition's revision.
- The **rule** is reversible in the same way, and it is deliberately weak in one direction: it constrains
  what a document may *show*, never what a round may *conclude*. If it ever proves too heavy for a routine
  row, the remedy is to state the figure's provenance in prose (the standing practice) and mark it
  non-citable — no gate changes either way.
- The **route** is reversible by construction: the outside instrument is harness-only, so taking the
  `Observer` route later adds a second instrument rather than rewriting this one. The two would have to
  *agree* on the added quantity, and that comparison is a future round's business.
- The **one figure** designated in §5 is not reversible in the sense that matters: once published with its
  raw, the figure is reproducible forever, and a later correction is an erratum, not a deletion.

## 8. Register (this ADR carries no new loop finding)

- The **counterexample** (§1.3) is a property of landed evidence, not a defect of code: `R32-2`/`R32-3`'s
  aggregates are tracked, their raws are not, and no rule required otherwise at the time. Its disposition is
  the rule in §2 — **no repair is owed to R32**, and §5's supersede ruling is the remedy the round applies.
- **`R59-0-F1` (this card's own)**: `ADR-036:110`/`:216` cite `R41-4`, which never ran. Outside this round's
  write set; **named here and in DESIGN §12.23 as residue**, owner the next round that may write
  `design/decisions/ADR-036-*.md` (the ADR-036 text is otherwise untouched and its `Observer` row stands).
- The gate question, the framing-promise question and the `Observer` route are **registrations**, not
  findings: they are §6's list, and R59 wires none of them.

## 9. References

- `AGENTS.md` constraints 1, 2, 3, 4, 5, 8, 9
- `design/DESIGN.md` §12.10.3 R1/R2/R4/R5/R6, §12.16 (where numbers live), **§12.23 (the definitions)**,
  §13.1 (P1's row), §12.2 (`:338`, at the base) and §13.6 (`:4137`, at the base) — both amended by this
  round to name the `R41-4` forward reference that no round ran
- `design/decisions/` — ADR-007, ADR-011, ADR-012, ADR-015, ADR-028, ADR-029 D1/D3, ADR-030 D3, ADR-036
  D3 + `:110`/`:216` (residue), ADR-042, ADR-044
- `docs/spec.md` §2, §6, §7, §9.2, §9.3
- `tests/conformance/tests/conf_13_sse_passthrough.rs:106-108` (what the suite asserts, and what it reserves)
- the loop's result record, the loop's result record,
  the loop's qa3 rig script
- the loop counterexample record (this round's evidence)
- the loop's tree §5.3 (the pair, as R32 left it);
  the loop's tree (`R41-1-F5`)
- the loop state record row 13 (harness-only observation), row 18 (the loop charter is a gate definition),
  row 26 (registered by this round: the framing promise)

## Publication note (2026-10-03, R62-2)

The `R<n>` labels and finding ids in this document name iterations of the project's own
private analysis loop — a loop that is not part of this repository, so no label here is
resolvable by a reader of it; they are kept as the provenance of the decision. This
publication pass removed only the dead-pointer class: every reference into that loop's
working tree (its file paths and round-record names, its state record, charter, execution
model and replay contract, its scripts and module names, and the kanban card ids), each
replaced by the neutral phrase the sentence needs. Nothing else moved — no figure,
threshold, `§`/`ADR`/`CONF` id, code sample or contract sentence.
