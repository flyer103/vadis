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
   lossy notes if it did. This is where an unexplained behavioural difference usually
   starts.
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
6. **Transform** — one entry per plugin step that changed the payload: tokens added, tokens
   saved, the cache impact, the verdict, and the tee identifier if the rule was marked for
   teeing. The verdict is the accounting question (below).
7. **Usage and cost** — the normalized usage (total input, cached input, cache write,
   output, reasoning) and the five-tier cost breakdown with the total, plus the plan state
   after the charge where a plan applies. Cost is computed by the code path the product's
   cost engine uses everywhere — the same one `router replay` will use when it lands
   ([`docs/spec.md` §9.3](../docs/spec.md)) — so any figure here can be recomputed from the
   record.
8. **Result** — the status returned to the client, the upstream status, the failover origin
   if the route was switched, router's own overhead, the upstream latency, and whether the
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

- **`GET /health`** answers with what this process actually loaded, and when a `plan_policy` is
  configured it also carries a **plan section**: the family, its two routes, which account the
  family is on right now, when it moved there, and — while it is on the metered account — the
  instant at which the plan may be probed again, plus why a probe would not be admitted yet
  (the cooldown, the plan's own window, a provider cooldown). It reports state; it can also say
  that no family is configured, and it never invents one.
- **`router stats --config config.yaml --window 24h`** reads the traces in that window and prints
  the cost and cache report, the measured savings per transform, the plan family's switches and
  what they cost, and how many requests in the window have an outcome router cannot verify (see
  [Operations](operations.md)). The window is **required** — a saving that does not state its
  window cannot be checked — and `--json` prints the same figures for a script.

The point of the report is the **label on every figure**, `verified` or `inferred` (the two
conventions are the table above): you can tell a measured number from an estimate while you read
the report, not afterwards. The field-by-field shape, each figure's provenance and which record
it comes from are [`docs/spec.md` §9](../docs/spec.md).

Three things are deliberately **not** served in v0.1: **`router replay`** (recompute cost and
cache over a fixed trace through the same code path that served it), **`router trace tail`**
(follow the live decision stream) and **`GET /metrics`**. They are planned; the report above does
not depend on them. Until they land, the trace file itself is the interface — one decision record
per request, appended to `<trace.dir>/YYYY-MM-DDTHH.jsonl`, readable with any JSON tool — and the
reason a surface's shape is frozen only by the change that implements it is the rule in
[`docs/spec.md` §9.3](../docs/spec.md).

`router stats` reads the trace files directly and opens the local store read-only, so it runs
while `serve` holds that state directory (see [Operations](operations.md)).

## When cost goes up, look in this order

1. **Prefix continuity** between adjacent turns in the same session. If it dropped, a
   transform (or a translation) broke the cache and everything else is downstream noise.
2. **The per-transform accounting** — which step claims what, and whether the claim is
   verified or inferred.
3. **The failure mix** — how often the gateway switched routes, for which reason, and what
   each switch cost the cache.
4. **The model choice** — last, not first. With prefix caching working, the model is the
   second-order lever.

## Authoritative sources

- [`docs/spec.md` §6](../docs/spec.md) — the observation contract: field groups, the
  definition of prefix blocks and their hashes, and the metric definitions.
- [`docs/spec.md` §9](../docs/spec.md) — the reporting surfaces: `/health`'s plan section, the
  `router stats` report with each figure's provenance and label, and what is not served yet.
- [`docs/spec.md` §7](../docs/spec.md) — accounting: `verified` versus `inferred`.
- [`docs/spec.md` §4.1](../docs/spec.md) — trace output parameters (directory, rollover).
- [`docs/spec.md` §4.5](../docs/spec.md) — the local state store: event log (state truth)
  versus trace (analysis truth), and their join key.
- [`design/DESIGN.md` §9](../design/DESIGN.md) — replay with the same code path.
- [`design/DESIGN.md` §12.6](../design/DESIGN.md) — the record's field-by-field landing.
- [`design/DESIGN.md` §12.10](../design/DESIGN.md) — where the numbers are computed in the
  request pipeline, and where prefix blocks come from.
- [`design/decisions/ADR-005-trace-as-interface.md`](../design/decisions/ADR-005-trace-as-interface.md)
  — why the trace is the sole interface to the iteration loop.
- [`design/decisions/ADR-010-event-log-as-state-truth.md`](../design/decisions/ADR-010-event-log-as-state-truth.md)
  — the state truth, the join key and the unknown-outcome rule.
- [`autowork/STATE.md`](../autowork/STATE.md) — the measured facts and the current
  state-of-the-world as of the last round.
