# ADR-066 — the tool relay surface: four verbs vadis carries without reading, and the record that makes the spend visible

- Status: **accepted and landed** (2026-11-25). Spec §4.23, DESIGN §12.27, `crates/vadis-proxy/src/tools_surface.rs`,
  the four routes in `crates/vadis-cli/src/lib.rs`, the `tools:` section in `config.example.yaml` and the
  provider block in `providers.example.yaml`, `CONF-123` and the book's chapter land together.
- Kind: **a new public surface** — four POST paths, one optional top-level config section, one optional block
  on a provider entry, one group in the trace's `DecisionRecord`. No `WireApi` variant, no new event kind, no
  state, no `/metrics` series, no price table, no client word of its own.
- Authority: the owner's ruling of **2026-11-25** — 「修改 vadis 来支持」, extending the media surface's own
  mandate to the provider's other tool APIs (search, fetch, tts, asr), and the same day's answers to the two
  open questions: **upstream errors are relayed verbatim**, and the TTS answer is **buffered** in the first
  slice. The candidate design was reported before coding and the rulings chose its shape.
- Amends: `docs/spec.md` **§4.23** (new), **§6** (the `tools` group; the `not_implemented` producers), **§8**
  (`not_implemented`'s row); `design/DESIGN.md` **§12.27** (new); `config.example.yaml` (the `tools:`
  section), `providers.example.yaml` (a provider's `tools:` block); `crates/vadis-core/src/config.rs`
  (`ToolKind`, `ToolUrlsCfg`, `ToolsCfg`, four load-time refusals); `crates/vadis-core/src/trace.rs`
  (`ToolsRec`, `DecisionRecord.tools`); `crates/vadis-proxy/src/tools_surface.rs` (new), `revision.rs`
  (`Revision.tools`), `lib.rs`; `crates/vadis-cli/src/lib.rs` (`guarded_tools_route`, `tools_endpoint`);
  `tests/conformance/tests/conf_123_tools_relay_verbatim.rs`.
- Does **not** amend: §2 / §2.1's byte boundary as it applies to protocol cells, §4.2's walk, §4.13's
  `server.max_body_bytes`, §4.15's reload, §4.17's cache rules, §4.7's guard, §4.22's media contract;
  `WireApi`, `ProviderCfg.urls`, `supports`, the alias roster, any event kind, `/metrics`'s series set,
  `/health`'s block shape.
- Cases: **`CONF-123`** (`search` relayed both ways byte for byte with the provider's key and the caller's
  content type; an `asr` multipart envelope and boundary arrive unchanged; a non-JSON `tts` answer is relayed
  as bytes; an upstream `422` reaches the caller with the provider's own error body, not §8's; no `tools:`
  section ⇒ `501` on all four verbs and no dial; a verb without its URL ⇒ `501`; a keyless provider ⇒ `501`;
  a body over `tools.max_body_bytes` ⇒ `413` before the dial and under it ⇒ `200`; no guard token ⇒ `401`;
  the record's `tools` group with `priced: false`, `usage_missing: true` and a zero cost). Unit tests beside
  §12.27's table hold the mount's refusals and the record's shape.
- Related: `AGENTS.md` constraints **1** (nothing is parsed, so nothing can be rewritten — a stronger reading
  of the byte boundary, §2 below), **2** (the relayed bytes are the caller's own; the record is a function of
  the call and the response), **3** (the trace is the observation; the surface reads nothing back), **4** (the
  zero cost is marked `priced: false` and `usage_missing` rather than presented as free), **8** (docs before
  code), **9** (no figure is invented); **ADR-005** (the trace is the interface), **ADR-019** (transform mode
  is written as `passthrough` because there is nothing to transform), **ADR-041** (`/metrics` stays frozen),
  **ADR-061** (the client word is the caller's, and the surface adds no oracle), **ADR-065** (the media
  surface whose section shape this ADR's config block copies, and whose opposed body rule it deliberately
  inverts).

---

## 1. Why

### 1.1 The verbs existed at the provider and nowhere in this deployment

vadis is the one place this deployment buys model work: it holds the keys, enforces the guard, bounds the
bodies and writes a ledger row for every call. A client edge (the assistant runtime on the same host) needed
four more things from the same provider — a web search, a page fetched as Markdown, speech for a reply, and a
transcript for a voice message — and none of them is a chat completion. The first was solved §4.22's way: vadis
writes the body, so vadis owns the contract. The four here cannot be solved that way, and §1.2 is why.

Left outside, the same three costs §1.1 of ADR-065 named would have followed: the spend leaves no
`DecisionRecord`, the wire shapes live in one operator's process, and a second port is a second thing to keep
alive. The difference from ADR-065 is only in what faithfulness means for a body vadis cannot read.

### 1.2 A tool call is not a model call, and pretending otherwise loses the body

Every route vadis served before this ADR takes the caller's body and normalizes it around a **model**: a
message array, a `model` field, a usage block, a set of provider URL kinds. A tool call has none of that.
Its body is the provider's own contract — a JSON query, a URL, a sentence to speak, a multipart audio file —
and its answer may be analytics JSON, a Markdown page or a WAV file. Three consequences decide the design:

1. **Re-serializing is lossy.** An `asr` upload carries its multipart boundary in the content type and its
   audio as bytes; a JSON round trip of a `tts` request is a different request. The provider validated the
   caller's bytes, so the caller's bytes are what must arrive.
2. **There is no model to select, price or attribute.** A verb names an endpoint, not a route, so the
   pipeline's selection, fallback, cache and plan machinery have nothing to operate on — and inventing a
   route for them would be a claim about a decision vadis did not make.
3. **An upstream refusal is information the caller owns.** The provider's `error.type`, `code` and `param`
   name the field to fix; §8's envelope would replace them with vadis's words. The owner's ruling is explicit:
   relay, do not re-shape.

## 2. Decision

### D1 — Four paths, served only when configured, refused in a typed way

`POST /v1/tools/{search,fetch,tts,asr}` are registered unconditionally behind the normal guard, and a verb is
**served** only when the config writes a `tools:` section, names an existing provider, and that provider
declares the verb's URL **and** holds a readable credential. Every other case answers `501 not_implemented` —
the same word §4.22's audio verbs use — so a client can tell *this deployment does not serve the verb* from
*this path does not exist*. The four refusals each write one pre-pipeline record: an absent section, a missing
verb URL, a keyless provider, and a provider entry the section names that is not in the roster (that last one
refuses the file at load, per §4.15's rule, because a section naming a provider that does not exist is a typo,
not a deployment choice).

### D2 — Relay both directions: no body is parsed, one header is set

The surface performs **no** parsing of the request — no JSON reader, no multipart reader, no model rewrite —
so it cannot reformat a number, reorder a key or normalize a boundary. `ToolSurface::prepare` sets the
provider's `authorization` header from the entry's key pool and copies `content-type` when the caller sent
one, then hands the bytes through. Everything else about the request is dropped (the caller's `accept` and
`user-agent` describe the wrong client), and the answer is relayed with the upstream's status and content
type plus `x-vadis-request-id`. This is a **stronger** reading of the byte boundary than §2.1 requires: the
protocol path needs two permitted mutations only because vadis must strip its own fields and select a model;
here there is no vadis-owned field and no model to select, so the permitted set is empty and the
implementation is correspondingly bare.

### D3 — Upstream answers are relayed verbatim, including 4xx and 5xx

A reached provider's status, content type and body go back as received. Only vadis's own refusals (unserved
verb, oversize body, no answer, oversize answer, auth) use §8's envelope. The distinction the ledger keeps is
the one a caller cannot see on the wire: `tools.upstream_status` records that an answer arrived (and what it
was), while `upstream_status: null` plus `502` records that nothing came back. A relayed `4xx`/`5xx` still
writes `errors[].kind = upstream_error`, because it is a failed call even when its bytes are useful.

### D4 — The configuration is one section plus a per-provider URL block

`tools: { provider, max_body_bytes (9 MiB), max_response_bytes (16 MiB), attempt_timeout (10m) }` names one
provider; the provider entry carries `tools: { search?, fetch?, tts?, asr? }` as full `http(s)` URLs. The
gaps are deliberate: **declaring `search` promises nothing about `asr`**, because the roster's providers
differ in which verbs they sell, and a missing URL must read as *not served here* rather than as a dial to a
wrong path. `attempt_timeout` is separate from `server.upstream_attempt_timeout` on purpose: a 120-second
patience is right for a model call and wrong for a paragraph of speech, and a bound that truncated legitimate
TTS work would bill the provider and refuse the caller.

### D5 — One attempt: no retry and no failover

The forwarder's failover walk exists because another model can answer the same message. A replayed tool call
is a second **bill** — the provider charges per search, per fetch and per spoken second — and a caller who
receives the provider's own `503` or `422` is better served than by vadis's silent second attempt. So each
call dials once, and the failure message distinguishes *no request byte left this process* from *the request
was fully written, so the call may have been billed*: that sentence is the difference between a safe retry
and a double charge, and only the caller can act on it.

### D6 — Both bounds are refusals, never truncations

A declared `content-length` over `tools.max_body_bytes` is refused `413` before a byte is read (§4.13's rule,
on this surface's own knob, with the connection closed so an undrained body cannot parse as the next request);
a body that overruns during the read is refused the same way. An answer over `tools.max_response_bytes` is
refused `502` rather than truncated: a cut `fetch` page or a cut WAV file is undetectable corruption the
caller has already paid for. The answer is therefore **buffered** — streaming audio would hand the caller
partial bytes whose accounting (billed versus delivered) no bounded unit can describe, and streaming is a
change to the section, not a hidden mode of it.

### D7 — One record per call, with a group that says what kind of call it was

`DecisionRecord.tools` is present **iff** a tool call wrote the row: `{ verb, provider, upstream_status,
request_bytes, response_bytes, priced }`. `protocol.protocol_in` is `tools`, `protocol_out` is `null`,
`decision.model` is empty and `transform_mode` is `passthrough`, because a verb names an endpoint rather than
a model, and nothing was decided or transformed. `identity.client` is the **caller's own** word: unlike
§4.22's `vadis-media`, the bytes here are the client's, so the client is genuinely the client. `priced` is
`false` in this slice, with `usage_missing: true` and an all-zero `cost` whose `currency` is still set — the
only honest way to say *this call's price is not in vadis's books*, so a reader sees **unpriced**, never
**free** (§7's rule that an unmeasured number is not a zero, applied to a cost).

## 3. Alternatives considered and rejected

- **Re-serialize each verb into a chat completion, §4.22's way.** Rejected for `asr` (a multipart audio body
  cannot become a chat message without a transcription vadis does not have), for `tts` (the answer is audio,
  not a completion) and for `search`/`fetch` (their schemas are the provider's to evolve; a vadis-owned copy
  is a fork with a lag). It would also have claimed a pricing precision this slice does not have.
- **A new `WireApi` variant plus a `urls` key.** Rejected: the protocol enum names **model protocols**, each
  with a message shape, a usage block and normalization; a fourth variant would have claimed a normalization
  vadis does not perform and forced every `urls`/`supports` map to grow a key no route kind can validate. The
  trace's `protocol_in` is a plain string, so the surface names itself without touching the enum.
- **Re-shape upstream errors into §8.** Rejected by the owner's ruling, for the reason in §1.2.3: it deletes
  the field-level detail the caller needs, and it attributes the provider's decision to vadis.
- **Retry `5xx` or fail over to another provider.** Rejected for D5's reason (double billing) and for a
  second one: no two providers' tool bodies are interchangeable, so a failover would need a request
  translation that D2 forbids.
- **Stream the TTS answer in the first slice.** Deferred, not rejected — it needs a per-verb accounting for
  *bytes delivered versus bytes billed* and a client-side story for a partial file. The trigger is a caller
  that genuinely must start playback before the body ends.
- **Serve nothing and leave the four verbs to the client.** Rejected: that is the shim pattern ADR-065
  retired one surface earlier, with the same three costs — invisible spend, semantics in one head, a second
  port to keep alive.

## 4. Consequences

- **The ledger gains a group, not a price.** `/state/events`, `vadis stats` and the trace can now answer
  *which tool verbs this deployment relays and how often*, and cannot yet answer *what they cost*. That gap is
  visible in every row (`priced: false`) rather than silently read as zero.
- **A crash between the dial and the record loses the row while the spend happened.** The record is written
  after the answer arrives, so a process death mid-call leaves a paid upstream call without a ledger row —
  the same window class as a chat call, and here the more likely direction (an answer that took minutes).
  The refusal rows are written before the answer is returned, so a refused call never lacks its row.
- **A caller that disconnects after sending still spends.** The relay is buffered, so the bytes leave before
  the answer is relayed; the row records the call as it ended upstream. Bounding that would require a
  cancellation story the tool surface does not have (a chat request can be cancelled; a paid tool call
  cannot be un-billed).
- **No cache, no plan policy, no fallback, no quota.** A tool call is neither replayable nor idempotent, so
  every one of those mechanisms is a change to this ADR, not a configuration away.
- **`/metrics` is unchanged.** The verbs are observable through the trace and the events surface; a series
  set is a later decision with its own cardinality argument (ADR-041).

## 5. What a later slice would have to decide

- **Tool pricing and quota** — a price table per verb (per request, per character, per audio second), a
  budget that bounds it, and the ledger's cost fields becoming real. Trigger: the owner asks what a verb
  costs, or tool spend becomes a material share of a deployment's bill.
- **Streaming TTS** — the accounting above plus a client contract for a partial file.
- **The provider's multipart `ocr`** — a different contract from §4.22's JSON surface, and the one place
  where the same path would mean two things; it needs its own path or its own content-type dispatch.
- **A tool cache** — only for a verb whose request is a pure function of its body (a `fetch` of an immutable
  URL), and only with the byte-identity rules §4.17 already imposes.

## 6. Verification

L1: the unit tests beside §12.27's table (the mount's four refusals, `prepare`'s header rules, the record's
shape). L2: `CONF-123`, ten cases over a real `vadis_cli::serve` against a mock upstream, covering the relay
in both directions, the multipart boundary, a non-JSON answer, an upstream `4xx`, the four unserved arms, the
body bound, the guard and the record. L2 (live): one call per verb against the deployment's provider, with
the answer's bytes and the row's `tools` group read from `/state/events`. No L3 (no client in this repo
drives the surface yet); the assistant runtime's own calls are the first production exercise.
