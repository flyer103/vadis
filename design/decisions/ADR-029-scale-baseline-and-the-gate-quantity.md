# ADR-029 — the scale/latency baseline: what the latency gate's quantity is, how the baseline is measured, and where its numbers live

- Status: accepted
- Date: 2026-09-23
- Related: AGENTS constraints 1, 2, 4, 5 and 9; ADR-005 (the trace is the only product → analysis-loop channel);
  ADR-006/ADR-007 (integer NanoUsd; span-faithful forwarding); ADR-009 (one local store; **"Re-measurement is
  owed"**, and the latency budget as *router's own work*); ADR-012 (the gate definitions, the corpus, the
  conformance assertions and the L1 envelope are outside the loop's mutable scope); ADR-016 **DP-1.4**'s
  registered finding (the operative envelope has no contract home); ADR-017 §3 (declared value vs measured
  value); ADR-019/ADR-028 (the transform contract and the recorder); spec §6 (the metric definitions), §7 (the
  labels), §8 (the refusal contract), §9.2 (the report); DESIGN §12.1 (the dependency allowlist), §12.15,
  §12.16; the loop charter (the **blocking** latency gate); the loop state record (R2G6's
  self-overhead), the same file/the same file (*Key measured facts* + waiting-on-human **row 1**), the same file (R9-G5).

## Background

**The blocking gate rests on a number that was measured without the path it guards.** The loop charter makes
*"the decision + transform overhead p99 stays within budget (benchmarked against rtk's <10 ms shape)"* a
**blocking** gate. The only latency figures that exist are R4's `overhead_ms` p50 1 ms / p99 6 ms — a mock
upstream, measured **without** the transform path (the loop state record, R9-G5) — and R2/R2G6's self-overhead
p50 ≈ 3.6–3.8 ms / p99 ≈ 4.5–5.3 ms at a ~20 KB payload, n = 60/side (the same file). There is **no throughput number,
no concurrency number and no request-size number anywhere in the tree**, and the envelope the round records
quote as "ADR-009's" (`p50 < 15 ms / p99 < 50 ms`) has no contract home at all — ADR-016 DP-1.4 registered
that, and the loop state record's waiting-on-human row 1 still carries it as open.

**The quantity the gate names is not the quantity the shipped field reports.** `result.overhead_ms` is measured
from the request's own start to the record's commit — `crates/router-proxy/src/forward.rs:593` sets `started`,
`crates/router-proxy/src/accounting.rs:410` reads `ctx.started.elapsed()` — so it **includes the upstream
attempt**. The report's own contract says otherwise: spec §6 defines `overhead_ms_p99` as *"router's own
overhead (excluding upstream)"* and spec §9.2's provenance row excludes `upstream_ms`, while
`router stats` printed the p99 of the **raw field** (`crates/router-cli/src/stats.rs:692`, `:764`). On any run
whose upstream would carry a delay, the figure the operator reads would be the upstream's number — which is
exactly the run this ADR's baseline is. That contradiction is `R32-F5`, classified **blocking**.

**The two serialization points a scale baseline is expected to find are known before it is run.** The store is
one `Mutex<Connection>` with `PRAGMA locking_mode = EXCLUSIVE` and `synchronous=FULL` for the intent/accounting
event classes (`crates/router-store/src/lib.rs:7-15`, `:117`, `:205`, `:218`), and the trace sink is one
`Mutex<Inner>` appending one line per request (`crates/router-store/src/trace_sink.rs:39-42`). A concurrency
curve that rises and then bends is therefore the expected shape; what is not known is *where* it bends, and
until R32 no number said so.

## Decision

### D1. The gate's quantity is a subtraction, and it is derived per record

**`router_overhead_ms` = `result.overhead_ms` − `result.upstream_ms`**, per record, in integer milliseconds.
`overhead_ms` spans the whole request (start → commit, upstream included), `upstream_ms` is the answering
attempt's own latency (`forward.rs:1194-1198`), so their difference is the router's own work. A record whose
`upstream_ms` is **`null`** (a boundary refusal, a pre-route rejection, a connect failure) is **excluded from
the sample** — spec §6's definition, and §7's rule that an absent measurement is never read as a value. This
quantity is the one the loop charter means, and it is the only latency quantity R32's numbers may be reported
for.

Two consequences are part of the decision rather than footnotes:

- **The product's own surface must agree.** `router stats`'s `overhead p99` line and its `--json`
  `overhead_ms_p99` member are that quantity's product-side surface; the report's derivation is corrected to
  the subtraction (`R32-F5`, witnessed by `CONF-84`, DESIGN §12.16), in step with spec §6/§9.2 as already
  written. Until that lands, the printed figure measures the upstream and is not the gate's number.
- **The resolution is coarse and the ADR says so.** Both fields are integer milliseconds, so the difference is
  quantised and a sub-millisecond truth reads `0`. An assertion on this quantity is therefore stated in whole
  milliseconds with the resolution named, and a p99 of `0 ms` is reported as `<1 ms` and is never used as a
  ratio's denominator (D4's guard). A finer-grained field is *not* taken here: an additive trace field is a
  spec §6 change and this round keeps both schema versions at **2**.

### D2. The metric set: one row per metric, with its unit, its definition and its source

| Metric | Unit | Definition | Source |
|---|---|---|---|
| `router_overhead_ms` | ms (integer) | D1's subtraction, per record; aggregates p50 / p95 / p99 / max plus **n** (valid samples) | trace `result.overhead_ms`, `result.upstream_ms` |
| `throughput_rps` | requests/s | completed requests over the rung's steady window (the declared warm-up excluded) | the harness's own clock |
| `throughput_tokens_per_s` | tokens/s | Σ the rung's measured usage (`input_total + output`, per spec §6's normalization) over the same window | trace `usage`, records with `usage_missing: false` only |
| `error_rate` | ratio | non-2xx **as the client saw them** / requests attempted — reported beside, never merged with, the trace's `usage_missing` share (two different facts) | the client's own responses **and** the trace |
| `write_pressure_delta` | ms | p99(`router_overhead_ms`) on the fresh-session arm **minus** the same on the sticky arm at one payload and concurrency. A **delta**, not a store write time: the product records no store-write timing, and this is the honest substitute that needs no new field | trace, two rungs |
| `rss_max` | KiB | the `router serve` process's peak resident size during the rung, read by the harness from the process table | the operating system |
| `client_elapsed_ms` | ms | the client's own per-request elapsed **less the stand-in's declared injected delay**; p50 / p99 | the harness, **corroboration only** — never a gate input, and never presented as the trace's measurement |

**Explicitly not measured in R32, with the reason:** *store write latency*. No field records it, adding one is a
spec §6 change, and both schema versions stay 2 this round. `write_pressure_delta` is what R32 can honestly say
about the write path; the direct measurement stays `to be measured` until a round has a field to read it from.

### D3. The load shape is frozen as a declared ladder, and the stand-in is declared with it

**The ladder (R32's rungs).** Each rung is a declared tuple (protocol cell × payload × concurrency ×
session/sticky × store), and every rung's report names its tuple:

| Ladder | Rungs | Purpose |
|---|---|---|
| **A — the core ladder** | `chat` non-streaming, 20 KB payload, concurrency **1 / 8 / 32 / 128 / 256**, fresh session per request, fresh store | the concurrency curve; **the envelope is read off this ladder** (D5) |
| **B — payload** | `chat` non-streaming, concurrency 32, payload **~2 KB / ~20 KB / ~200 KB**, fresh session, fresh store | where buffering and body-size cost appear |
| **C — the media sweep** | all six native cells (`chat` / `responses` / `anthropic` × streaming / non-streaming), 20 KB, concurrency **1 and 32**, fresh session, fresh store | the same quantity on both media, so a path-specific regression is visible |
| **D — the store arm** | `chat` non-streaming, 20 KB, concurrency 32: fresh session vs **one sticky session**; and **grown store** (≥ 3 MiB, the R2G6 precedent) vs fresh | D2's `write_pressure_delta`, and the store's own effect on the curve |
| **E — the smoke pair** | 10 requests at concurrency 1 against a stand-in at the rung's declared delay, and the **broken control**: the same rung with a deliberately large injected delay | proves the harness measures what D1 says: the control moves `client_elapsed_ms` and must **not** move `router_overhead_ms` |

**Per-rung rules.** 200 requests or 30 s, whichever ends first; a declared warm-up excluded from every
aggregate; **≥ 100 valid samples** (`upstream_ms` present) or the rung is reported `insufficient sample` instead
of a number. `state.dir` is per-run **free by construction**: each rung's config file lives in its own fresh
directory, so the fixed `state/router.db` and the trace land under it (spec §4.5, §4.1) — ADR-009's one-writer
rule is respected by never sharing a directory between a running process and a seed. The grown-store arm seeds
to a **declared and measured** size before `serve` starts and reports the file's byte size.

**The upstream is a harness-owned loopback stand-in**, at $0.00, of the class the conformance mocks are
(`tests/conformance/src/lib.rs`'s loopback HTTP/1.1 mock: canned response, byte-recording) — a loopback HTTP/1.1
server that records the bytes it received, answers a canned body, keeps the connection alive, and carries a
**declared per-request delay**. The delay is declared because it is what makes D1's quantity legible: the
router's own work must be visibly separable from the stand-in's, and the mechanism that separates them is the
run's own control (ladder E).

**The machine is part of the report.** CPU model and core count, RAM, OS and the exact binary commit are stated
with every number; a figure that does not name its machine is not citable.

### D4. The ceiling is defined mechanically, and it is where the curve breaks

Walking ladder A upward, the **ceiling** is the first rung at which any of these fires, and the report names
which one did:

1. **the curve bends**: p99(`router_overhead_ms`) > 5 × that ladder's concurrency-1 p99, where the reference is
   `max(p99_derived, 1 ms)` — the guard exists because a sub-millisecond p99 reads `0` and `5 × 0` would make
   the criterion fire on every rung;
2. **throughput saturates**: `throughput_rps` at rung *i* < 1.10 × rung *i−1* while the router process's CPU
   stays ≥ 90 % of the machine's core count;
3. **errors**: `error_rate` > 0 — a rung that answered errors is not a throughput rung, whatever its p99 says.

The ceiling is a property of *this machine at this commit*, not a constant: each measurement recomputes it from
its own files.

### D5. The method gets a contract home; the budget does not

This ADR is the **method's** home — the quantity (D1), the metric set (D2), the shape (D3) and the criterion
(D4) — which is the part of ADR-016's registered gap a loop can close. It is **not** the envelope's home in the
sense the waiting-on-human row means, and it deliberately does not become one:

- **No threshold is set here.** The number the blocking gate compares against is a **human** decision (ADR-012;
  the loop state record's row 1, still open). R32 reports measured numbers against the **declared** reference
  the loop charter already names (rtk's <10 ms shape) and asserts no budget of its own.
- **R32's numbers are the non-transform baseline.** The run's transform plugin configuration is declared with
  its numbers (ladder A–E: **no rule engine loaded and no `X-Router-Transform` header**, i.e. the v0.1 assembly
  with `plugins:` absent), and R33 re-runs **the same shape** with the plugin enabled so the two are comparable
  field for field. The transform path's contribution is `to be measured by R33`; R32 measures nothing about it.
- **The declared/measured pair ADR-017 §3 established still governs.** A record may restate the declared
  reference beside R32's measured numbers, and may derive no timeout, no threshold and no gate verdict from
  either.

### D6. Where the numbers live

Per-run raw files under the loop's evidence for that decision (one machine-readable file per rung plus a summary table), the
round record, and the loop state record's *Key measured facts*. **Not in this ADR and not in DESIGN**: DESIGN
§12.16 records the quantity's two fields and points here, and states no number, for the same reason §12.5's
prices have one source — a second copy of a measurement is a copy that drifts.

### D7. What this ADR does not authorize

`GET /metrics` (spec §9.3 keeps it planned-not-served, and no product surface can carry a client-observed load
number anyway); rate limiting or per-client connection limits (each is a second admission rule with its own
contract, and spec §4.13 is explicit that the body bound is not one); a sharded or multi-process store
(ADR-009's single writer is the design whose cost this baseline measures); inbound TLS termination; any change
to the loop charter's gate definition, to a threshold, to the corpus or to the L1 envelope (ADR-012); and any
new Rust dependency — the load harness is **Python-side** (DESIGN §12.1: `criterion` would be an allowlist
decision, and the harness must drive the real `serve` binary as a black box rather than a benchmark target).

## Alternatives considered

| Alternative | Why rejected |
|---|---|
| report `client_elapsed_ms` as the gate's quantity | it measures the client, the socket and the stand-in as well as the router, and it cannot separate them; the trace already carries the router's own two fields, so the client-side number is demoted to corroboration (D2) rather than used as the measurement |
| change `overhead_ms` itself to exclude the upstream | it moves the meaning of a field that has shipped (R2G6 records its inclusive reading, the loop state record), and every past record's value would need re-reading; the report-side derivation reaches the same number while leaving the trace's field and every historical record unchanged |
| add µs-resolution trace fields (`overhead_us` / `upstream_us`) | additive and schema-preserving in principle, but it is a spec §6 field-group change and a product change; this round's constraints keep both schema versions at 2 and spend the round's contract budget on the body bound. It is the right change if a future round needs sub-millisecond claims |
| a real provider as the upstream | the round's cap is $0.00, a real upstream's latency is not the router's, and an unmeasured cross-machine delay would make D1's quantity unreadable |
| `criterion` / a Rust benchmark binary | a new dependency is a decision, not an import (DESIGN §12.1), and an in-process benchmark bypasses the listener, the store and the trace — i.e. exactly the layers whose capacity is unknown |
| a single concurrency rung (e.g. 32) | a throughput number with no curve cannot show where it breaks, and the gate's p99 is a different figure at C = 1 than at C = 128 |
| freeze a budget from R32's own numbers | the gate's threshold is outside the loop's mutable scope (ADR-012) and the envelope's contract home is an open human row; the loop would be setting the bar it is judged by |
| expose the load numbers on `GET /metrics` | a new endpoint is its own scope (spec §9.3), and the numbers are harness-side observations of a client, not product facts |
| measure the transform path in R32 | it is R33's scope by the plan of record, and a baseline taken with the plugin enabled could not be the *baseline* of anything |

## Rationale

- **The quantity follows the gate's own words.** The loop charter says *the decision + transform overhead* —
  the router's work, not the provider's. The subtraction is not a convenience; it is the only reading under
  which the gate is about this repository at all.
- **A declared injected delay is what makes a nonzero measurement meaningful.** With a zero-delay stand-in the
  inclusive and the exclusive quantities coincide, which is precisely why the R4 figure (mock, no transform
  path) could never have caught `R32-F5`. Declaring the delay turns the difference into a quantity that can be
  wrong visibly, and the broken control (ladder E) is the assertion that it is not.
- **The stand-in's class matters more than its features.** Using the conformance mocks' class (loopback,
  canned, byte-recording) is what makes R32's numbers comparable with the numbers every later round will take,
  and it keeps the byte-fidelity control (AGENTS 1) available inside a load run at no extra cost.
- **One ladder, declared in advance, is what makes a wrong number visible as a finding.** R32-3 reproduces the
  same rungs with its own generator and its own trace read; if the freeze had left the shape to the
  implementer, "the numbers disagree" would have no defined meaning.
- **The threshold stays the human's, and saying so is the point.** ADR-016's finding was not that the envelope
  was unknown but that it had no home. Giving the *method* a home while leaving the *number* where ADR-012
  puts it is the difference between a measurement and a self-granted permission.

## Consequences

- R32's implementing cards build the harness and the ladder under the loop's evidence for that decision, run it, and report
  rungs, the ceiling and the machine. **No `verified` figure is minted by any of it** (AGENTS 4): these are
  latency and capacity numbers, and no token or dollar delta is claimed.
- `R32-F5`'s repair lands with `CONF-84`, and `CONF-83` lands with spec §4.13's bound (DESIGN §12.8's rows).
- R33 re-runs the same shape with the transform path enabled; that comparison is the gate's first
  transform-inclusive measurement, and it is the round that may present the pair to the human.
- The envelope's threshold remains waiting-on-human row 1. This ADR does not close it; it makes closing it a
  decision with a measured quantity attached.
- Honest boundaries: the baseline is **one machine at one commit**; the quantity is integer-millisecond
  coarse; `write_pressure_delta` is a delta and is not a store-write latency; the store's own write latency
  stays unmeasured for want of a field; and nothing here says anything about how the router behaves above the
  ceiling, because D4 defines the ceiling and stops there.

## Publication note (2026-10-03, R62-2)

The `R<n>` labels and finding ids in this document name iterations of the project's own
private analysis loop — a loop that is not part of this repository, so no label here is
resolvable by a reader of it; they are kept as the provenance of the decision. This
publication pass removed only the dead-pointer class: every reference into that loop's
working tree (its file paths and round-record names, its state record, charter, execution
model and replay contract, its scripts and module names, and the kanban card ids), each
replaced by the neutral phrase the sentence needs. Nothing else moved — no figure,
threshold, `§`/`ADR`/`CONF` id, code sample or contract sentence.
