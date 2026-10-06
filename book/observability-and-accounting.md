# Observability and accounting

Status: written for v0.1. The field-level observation contract is `docs/spec.md` §6 and the
accounting convention is §7; this chapter tells you what to look at, in what order, and what
the numbers mean.

Every request produces exactly one decision record, appended to a trace file. The trace is
the only channel between the serving path and everything that analyses it — there is no
hidden second source of truth. The gateway also keeps its own operational state (session
stickiness, the cache ledger, the quota counters, provider cooldowns) in a local store, but
that store is never read by the analysis side: the split is defined in
[`docs/spec.md` §4.5](../docs/spec.md) and ADR-010.

## One record per request, and the two truths it belongs to

The two records of a request have different jobs, and neither can be reconstructed from the
other:

- **the trace** (one JSON line per request, under `trace.dir`) is the **analysis truth**:
  what was decided, what the prefix looked like, what each transform claimed, what the
  usage and the cost were. Money questions are answered here.
- **the event log** (a table in the local store) is the **state truth**: every state
  transition — session binding, upstream intent, quota charge, config applied. "Why does
  this plan show this much used?" is answered here, because each charge is a row rather
  than a counter nobody can audit.

**They join on two keys:** the record's `request_id` and its `event_id`. The `event_id` is
the request's own `request.received` event, so a trace line always names its anchor in the
log, and the accounting events of that request carry a pointer back to the trace line. The
pairing is exact — never by timestamp, which would be a guess dressed as a join.

## How to read one decision record

Read it in this order; each group answers a different question.

1. **Identity** — which request this was: `request_id`, `event_id`, the normalized client,
   the resolved `session` (the client's own key, or null when it sent none), and
   `turn_index` within the session. Start here, because every later number is per-request.
2. **Protocol** — `protocol_in`, `protocol_out`, whether a translation happened, and the
   lossy notes if it did. On every record this build writes the two wires are equal — a route
   is attempted only on a cell it declares — and no translation happens, so this group is the
   check that the passthrough promise held. It is also where an unexplained behavioural
   difference usually starts.
3. **Decision** — `provider`, `model`, `selection_source` and the plugin chain. If the
   request did not go where you expected, this is the answer to "why".
4. **State** — whether the inbound request carried server-side state, whether the sticky
   binding was hit, and how many cache-control breakpoints were placed. **This group is a
   constant in v0.1**: every record carries `false` / `false` / `0`, because inbound state is
   not detected and the sticky-binding read does not reach the record (known gap G-F). Do not
   read `false` as "the client sent no server-side state", and do not expect the `true` of
   the illustration below to appear yet.
5. **Prefix** — the block-level breakdown of the upstream-visible prefix, and
   `prefix_continuity` against the previous request in the same session. **This is the
   fidelity number.** A block is a structural unit (a message, a tool definition, an input
   item), each with its own token count and hash; continuity is the longest common block
   ratio. When it drops, something is breaking the upstream prefix cache.
6. **Transform** — the **mode** the record was served under, plus one entry per step that changed the
   payload: the path it edited with the bytes before and after, the tokens added and saved, the cache
   impact, the verdict, and the tee identifier if the rule was marked for teeing. The mode is a field
   of its own because an empty step list is ambiguous — *no mode* and *mode asked for, nothing matched*
   would otherwise look identical — and v0.1 writes `passthrough` on every request, because no
   transform is wired yet (ADR-019; [`design/DESIGN.md` §12.12](../design/DESIGN.md)). The verdict is
   the accounting question (below).
7. **Usage and cost** — the normalized usage (total input, cached input, cache write,
   output, reasoning) and the five-tier cost breakdown with the total, plus the plan state
   after the charge where a plan applies. Cost is computed by the code path the product's
   cost engine uses everywhere — the same one `vadis replay` will use when it lands
   ([`docs/spec.md` §9.3](../docs/spec.md)) — so any figure here can be recomputed from the
   record.
8. **Result** — the status returned to the client, the upstream status, the failover origin
   if the route was switched, vadis's own overhead, the upstream latency, and whether the
   upstream reported usage at all.
9. **Failures** — `errors[]`, an array that is always present. No failure is an **empty
   array**, never a missing field; a failed transform, a failed trace write, an upstream
   failure and an internal error each appear here with a kind and a message.

A trimmed illustration of the shape (values illustrative; the normative field list with
types is [`docs/spec.md` §6](../docs/spec.md) and the Rust form is
[`design/DESIGN.md` §12.6](../design/DESIGN.md)):

```json
{"schema_version":1,"ts":"2026-09-19T07:00:00.000Z",
 "identity":{"request_id":"req-…","event_id":41,"client":"codex","session":"…","turn_index":2},
 "protocol":{"in":"responses","out":"responses","translated":false,"lossy":[]},
 "decision":{"provider":"…","model":"…","selection_source":"explicit","plugin_chain":["builtin/cache_guard"]},
 "state":{"stateful_inbound":false,"sticky_hit":true,"cache_control_breaks":0},
 "prefix":{"blocks":[{"kind":"input_item","index":0,"tokens":0,"hash":"…"}],"continuity":1.0},
 "transforms":[],"usage":{"input_total":0,"input_cached":0,"cache_write":0,"output":0,"reasoning":0},
 "cost":{"input_miss":0,"input_hit":0,"cache_write":0,"output":0,"total":0},
 "result":{"status":200,"upstream_status":200,"overhead_ms":0,"upstream_ms":0,"usage_missing":false},
 "errors":[]}
```

(The money fields above are fixed-point integers in nano-USD; this chapter shows shape, not
amounts — price figures live in exactly one place, your config.)

## Verified versus inferred — the only accounting question that matters

| Convention | Where the number comes from | What you may do with it |
|---|---|---|
| `verified` | a measured difference in the upstream's own usage: a control turn with a transform on and off in the same session, or an attributable change in cached tokens | report it as a saving, and use it in a gate |
| `inferred` | a local estimate, with no control | diagnose with it; label it, and never present it as measured |

Two consequences that surprise people, both deliberate:

- **A saving is a difference between two worlds, and the trace holds only one of them.** The request
  that ran is measured by the provider's own usage; the request that would have run *without* the edit
  appears nowhere. So a transform's figure is an estimate until a **pair** exists — a control turn with
  the rule on and off over the same content, or a replay that computes the counterfactual with the same
  code ([`docs/spec.md` §7](../docs/spec.md), and [§9.3](../docs/spec.md) for what is not served yet).
  Where no pair exists, the report shows **no verified saving**, which is an answer, not a gap.

- **A figure computed from local estimates stays `inferred` even when its inputs are
  measured.** The replay cost of a failover, for instance, is computed at decision time
  from prefix block token estimates, so it is inferred then; it becomes verified only once
  the switched attempt's real usage lands, and a failover with no following turn keeps the
  inferred label and says so.
- **An absent number is not zero.** When the upstream reports no usage (a streamed chat
  response whose terminal usage was not requested is the common case, and a stream that
  ended before its terminal event is another), the record says usage is missing, the cost
  is left uncomputed and no plan charge is invented. Reporting an invented number would
  violate the convention at exactly the moment it matters most.
- **Streaming requests keep the same books.** A streamed request is recorded exactly like a
  buffered one — session, prefix, usage, cost, one trace line — with the one difference the
  medium forces: the record is written when the stream ends, not when a body completes. Two
  stream-specific observations ride on the event log's `upstream.responded` row
  (`stream_completed`, `bytes_relayed`); a stream that died mid-way says so there and in the
  record's errors, and is never billed twice.

Any statement of "how much was saved" must carry three things: the convention, the sample
size, and the time window. Mixing conventions in one number is an error, not a rounding
question.

## Reading the reports

Two surfaces read the records back out, and both are **read-only** — neither is a second source of
truth, and neither prices anything you cannot already find in a record:

- **`GET /health`** answers with what this process actually loaded, and when a plan family is
  configured — either spelling: `plan_policy`, or the `plan_policies` list — it also carries a
  **plan section**: the family, its two routes, which account the
  family is on right now, when it moved there, and — while it is on the metered account — the
  instant at which the plan may be probed again, plus why a probe would not be admitted yet
  (the cooldown, the plan's own window, a provider cooldown). It reports state; it can also say
  that no family is configured, and it never invents one.
- **`GET /metrics`** (spec §4.16) renders §9.2's own figures over the last 900 seconds in the
  Prometheus text exposition format, behind the same token guard as the three protocol
  endpoints, with each labelled figure carried as a machine-readable `provenance` value.
- **`vadis stats --config config.yaml --window 24h`** reads the traces in that window and prints
  the cost and cache report, the measured savings per transform, the plan family's switches and
  what they cost, and how many requests in the window have an outcome vadis cannot verify (see
  [Operations](operations.md)). The window is **required** — a saving that does not state its
  window cannot be checked — and `--json` prints the same figures for a script.

The point of the report is the **label on every figure**, `verified` or `inferred` (the two
conventions are the table above): you can tell a measured number from an estimate while you read
the report, not afterwards. The field-by-field shape, each figure's provenance and which record
it comes from are [`docs/spec.md` §9](../docs/spec.md).

Two things are deliberately **not** served in v0.1: **`vadis replay`** (recompute cost and
cache over a fixed trace through the same code path that served it) and **`vadis trace tail`**
(follow the live decision stream). They are planned; the report above does not
depend on them. Until they land, the trace file itself is the interface — one decision record
per request, appended to `<trace.dir>/YYYY-MM-DDTHH.jsonl`, readable with any JSON tool — and the
reason a surface's shape is frozen only by the change that implements it is the rule in
[`docs/spec.md` §9.3](../docs/spec.md).

`vadis stats` reads the trace files directly and opens the local store read-only, so it runs
while `serve` holds that state directory (see [Operations](operations.md)).

## How to read a stream's own numbers

When a response streams, the gateway's only job is to pass it through — and "pass it through" is
three separate promises, each with its own number. They are defined in
[ADR-045](../design/decisions/ADR-045-citable-numbers-and-the-streaming-fidelity-instrument.md)
and stated as contract text in [`design/DESIGN.md` §12.23](../design/DESIGN.md); what follows is
how to read them as a user.

- **TTFT — time to first byte, measured on your side.** From the moment the request is written to
  the moment the first body byte arrives *at the client*. That is the number you actually feel. It
  is reported as a distribution, never as one value: one request's time-to-first-byte is an
  anecdote. Alongside it sits the gateway's share of it — the same clock, minus the instant the
  upstream emitted that first byte.
- **Added inter-chunk jitter — the honest one.** For each gap between two consecutive chunks that
  arrive at you, subtract the gap the upstream itself left between emitting them. What is left is
  what the gateway added. **Why the subtraction and not the raw gap:** with a stand-in upstream
  under the harness's control, the raw gap is the upstream's pacing plus the machine's scheduler —
  a figure that would describe your upstream while wearing the gateway's name. The same rig is run
  once with the gateway *out of the path*; that null baseline is published beside every number, and
  it is how you can tell how much of what you see is the ruler rather than the thing measured.
- **Fidelity — an equality, not a statistic.** A stream is faithful when the bytes that reach you
  are the bytes the upstream sent: same bytes, same order, same events, nothing merged, nothing
  split, nothing invented, the terminal `[DONE]` last and nothing after it. This is the
  response-side twin of the promise the gateway makes about the *request* it forwards, and unlike a
  latency number it is a verdict — you either got the stream the upstream wrote or you did not. The
  endings count: a stream the gateway cuts short (see [Protocols](protocols.md) for the bound) must
  reach you as a **strict prefix** of what was sent, with no terminal marker the upstream never
  sent, and the cut must be recorded as such. A gateway that only got the happy path right would
  look faithful on exactly the cases where it is most likely to re-frame what it relayed.

**How to recompute a figure yourself.** Every number the gateway publishes about its own streaming
behaviour obeys one rule: its **raw artifact is committed under a tracked path in this repository**,
beside **the exact command** that produced it and **the reducer** that turns the raw into the
figure, with **the machine and the commit** named (a figure that does not name its machine is not
citable). So you can check one without trusting it, and without re-running any measurement: clone
the repository, find the committed raw under the measurement evidence the figure names, run the
published reducer over it, and compare. What that gives you is the **figure**, not the **measurement**:
a stranger reproduces the figure from the committed raw and the reducer alone, while repeating the
measurement — re-running the run the raw came from — needs a built gateway binary (it is not in the
repository; it is built from the commit the figure names) and a machine of the kind the figure
names. The figure is the part that has to hold still; the measurement is yours to repeat. If a
figure cannot be reproduced that way, it is not citable — and
that is a statement about the figure's carriers, not about whether the measurement was honest. The
counterexample the rule was written for is in ADR-045 §1.3: two published readings of the same cell
that disagree, with the records each was reduced from left out of the repository.

**What these numbers do not say.** No figure compares this gateway with any other, or with a
different design: each one is *this machine, this commit, this upstream, this stimulus, this number
of samples*, and nothing about your traffic, your model or your load. And latency and fidelity
figures are not savings figures: `verified` and `inferred` (the table above) are the labels of the
*money* convention, and a latency number puts on neither.

## When cost goes up, look in this order

1. **Prefix continuity** between adjacent turns in the same session. If it dropped, something at the
   *front* of the conversation changed: the client's own content (a different first message, an edited
   turn), a route change (a failover or a plan switch re-prefills on another upstream cache), a
   configuration change — or, if the transform mode is in use, a mid-session
   mode or rule change (the drop is how that change is made visible). On the default path no transform
   runs, so the number is a statement about the conversation and the routing.
2. **The per-transform accounting** — which step claims what, and whether the claim is
   verified or inferred. In the default (passthrough) mode the list is empty on every request
   (`transform_mode` is `passthrough`, `transforms` is empty), which is itself the check: a non-empty
   list means a mode was asked for and something ran. Every figure it then shows is `inferred` until a
   paired on/off measurement exists.
3. **The failure mix** — how often the gateway switched routes, for which reason, and what
   each switch cost the cache.
4. **The model choice** — last, not first. With prefix caching working, the model is the
   second-order lever.

## Authoritative sources

- [`docs/spec.md` §6](../docs/spec.md) — the observation contract: field groups, the
  definition of prefix blocks and their hashes, and the metric definitions.
- [`docs/spec.md` §9](../docs/spec.md) — the reporting surfaces: `/health`'s plan section, the
  `vadis stats` report with each figure's provenance and label, and what is not served yet.
- [`docs/spec.md` §7](../docs/spec.md) — accounting: `verified` versus `inferred`.
- [`docs/spec.md` §2.1](../docs/spec.md) — the transform mode: the exact byte promise when a content
  transform is enabled, and why a saving measured there is an estimate until a pair exists
  ([`ADR-019`](../design/decisions/ADR-019-transform-mode-and-the-content-edit-contract.md),
  [`design/DESIGN.md` §12.12](../design/DESIGN.md)).
- [`docs/spec.md` §4.1](../docs/spec.md) — trace output parameters (directory, rollover).
- [`docs/spec.md` §4.5](../docs/spec.md) — the local state store: event log (state truth)
  versus trace (analysis truth), and their join key.
- [`design/DESIGN.md` §9](../design/DESIGN.md) — replay with the same code path.
- [`design/DESIGN.md` §12.6](../design/DESIGN.md) — the record's field-by-field landing.
- [`design/DESIGN.md` §12.10](../design/DESIGN.md) — where the numbers are computed in the
  request pipeline, and where prefix blocks come from.
- [`design/decisions/ADR-005-trace-as-interface.md`](../design/decisions/ADR-005-trace-as-interface.md)
  — why the trace is the sole interface to anything that analyses it.
- [`design/decisions/ADR-010-event-log-as-state-truth.md`](../design/decisions/ADR-010-event-log-as-state-truth.md)
  — the state truth, the join key and the unknown-outcome rule.
- [`design/decisions/ADR-045-citable-numbers-and-the-streaming-fidelity-instrument.md`](../design/decisions/ADR-045-citable-numbers-and-the-streaming-fidelity-instrument.md)
  — the definitions of TTFT, the added inter-chunk jitter and the fidelity assertion, the
  measurement's route, and the rule that makes a figure citable.
- [`design/DESIGN.md` §12.23](../design/DESIGN.md) — those definitions as contract text; §12.16
  for where a baseline's measured numbers live.
