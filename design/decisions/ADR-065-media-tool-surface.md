# ADR-065 — the media tool surface: one served verb (`ocr`), two honest refusals, and the section that turns them on

- Status: **accepted and landed** (2026-11-25). Spec §4.22, DESIGN §12.26, `crates/vadis-proxy/src/media_tools.rs`,
  the route in `crates/vadis-cli/src/lib.rs`, the `media:` section in `config.example.yaml` and `CONF-108`
  land together; the book gains one chapter.
- Kind: **a new public surface** — three routes, one optional config section, one value in the trace's
  `errors[].kind` vocabulary, one server-derived client word. No new `WireApi`, no provider concept, no event
  kind, no state, no `/metrics` series.
- Authority: the owner's ruling of **2026-11-25** — 「修改 vadis 来支持」, answering the finding that a client
  edge had grown a hand-written proxy beside vadis to turn an image into text. The candidate design was
  reported before coding; the ruling chose the surface and the OCR-only scope.
- Amends: `docs/spec.md` **§4.22** (new), **§6**'s `errors[].kind` set (gains `not_implemented`), **§8**'s
  `not_implemented` row (its media producer); `design/DESIGN.md` **§12.3** (`ClientKind` gains
  `VadisMedia`), **§12.26** (new); `config.example.yaml` (the `media:` block);
  `crates/vadis-core/src/trace.rs` (`ErrorCode::NotImplemented` serializes as `not_implemented`, not
  `internal`); `crates/vadis-proxy/src/{media_tools.rs,body_limit.rs,lib.rs}`;
  `crates/vadis-core/src/config.rs` (`MediaCfg`, two refusals); `crates/vadis-cli/src/lib.rs`
  (`guarded_media_tools_route`, `media_tool_endpoint`, `media_error`); `tests/conformance/tests/conf_108_media_ocr.rs`.
- Does **not** amend: §2 / §2.1's byte-faithfulness for protocol cells, §4.2's walk, §4.13's
  `server.max_body_bytes`, §4.15's reload, §4.17's cache rules, §4.7's guard; `WireApi`, `ProviderCfg.urls`,
  `supports`, the alias roster, any event kind, `/metrics`'s series set, `/health`'s block shape.
- Cases: **`CONF-108`** (`ocr` answers `{"text"}` and forwards the synthesized chat body; no `media:` ⇒ three
  `501`s with no upstream contact; both audio verbs `501` with the section written; an unusable body is `400`
  and never dials; a body over `media.max_body_bytes` is `413`, and the same shape under it is `200`; the
  surface is behind the guard). Unit tests beside §12.26's table hold the pure shapes and the record.
- Related: `AGENTS.md` constraints **1** (the synthesized body breaks no relayed byte: no client body is being
  replayed), **2** (the body is a pure function of the tool request and the config), **3** (the trace is the
  observation; the media module reads nothing), **8** (docs before code), **9** (no figure is invented);
  **ADR-005** (the trace is the interface), **ADR-019** (transform mode is resolved, not
  assumed, for the media call too), **ADR-041** (`/metrics` is frozen: the surface adds none), **ADR-042**
  (the cache applies as written), **ADR-061** (`identity.client`'s closed vocabulary and its anti-oracle
  property), **ADR-064** (the spec §4.21 shape this ADR's section copies).

---

## 1. Why

### 1.1 The job to be done is "read this file", and it had no verb here

A media edge has an image and a question. What it needs is one round trip: the file in, the model's reading
out. vadis sells exactly the round trip — a request against a configured, priced, guarded route — but every
route it exposed takes the **caller's own body** (constraint 1, spec §2.1) and selects the route from a
`model` the **caller** names. A client holding a JPEG has neither a chat body nor a defensible opinion about
which of the deployment's routes should read it.

The gap was filled **outside the repository**: a small Python proxy on `127.0.0.1:8291` in front of this
process, with its own port, its own log, and no `DecisionRecord`. Three costs followed, and they are the
finding this ADR acts on:

1. **The spend was invisible.** A request the deployment pays for left no trace, so `/state/events`,
   `/health` and `vadis stats` could not answer "what does media cost me" at all — not approximately, at all.
2. **The semantics lived in one operator's head.** The data-URI shape, the HEIC problem, the prompt default:
   each was a line in someone's Python, untested by this repo and free to drift from §2's protocol shapes.
3. **A second port is a second thing to keep alive.** Health, guard, body bound and reload came for free with
   vadis and had to be re-invented beside it.

### 1.2 The retired shim's two defects are the design's requirements

The proxy that existed (not in this repo) was seen failing for a day in a live deployment, twice for real
reasons and not for its own bugs:

- **It passed the image through as HEIC** and the upstream rejected the format deterministically, so the
  client received `[image]` and no transcript. A media surface that has no opinion about the media type it
  forwards is not a media surface.
- **It guessed at its request's shape** — a bare base64 string that began with `/` (every JPEG's `/9j/4AAQ…`)
  was taken for a filesystem path. Untyped inputs fail late and confusingly.

So: the surface must **state** the shape (§2), **refuse** what it cannot read (§2.7), and never invent an
answer.

## 2. The decision

### 2.1 Three paths; one serves

`POST /v1/tools/ocr` is served. `POST /v1/tools/transcribe` and `POST /v1/tools/synthesize` are
**registered** and answer `501 not_implemented`. All three answer `501 not_implemented` when the config
writes no `media:` section.

Registered, not absent, because the refusal must be **typed**: a client has to be able to distinguish "this
build cannot do audio" from "this path does not exist", and only §8's body carries that distinction. A `404`
would be a lie of omission — the capability is declared by the surface's own existence and is deliberately
withheld.

### 2.2 The OCR request and answer

`{"image": {"media_type": "image/jpeg", "base64": "…"}, "prompt": "…"}` → `200 {"text": "…"}`.

- `image.media_type` required, must be `image/*`; `image.base64` required, non-empty; `prompt` is the only
  other member read. Unknown members are ignored and never forwarded — the synthesized body is built from
  these and nothing else, so an unread member cannot become relayed bytes by accident.
- Prompt precedence: caller's non-blank `prompt` > config's non-blank `media.prompt` > `DEFAULT_OCR_PROMPT`
  (transcribe the text; describe only if there is none). The default is a transcription instruction because
  the transcript is what the caller's model reads next; a paraphrase of a signboard has lost the characters
  the request existed for.
- The answer is `choices[0].message.content` as a string (trimmed) or as parts joined by newlines; empty,
  missing, or a non-`2xx` upstream is `502 upstream_error`. **No fallback text is fabricated** — an empty
  reading is a failed reading.

### 2.3 The section: optional, and off by default

`media: { model: <alias | provider/model>, max_body_bytes: 8388608, prompt: <optional> }`.

Every other section is required because a hidden default is the most expensive silent failure; this one is
optional **with the safe default** — absent means the capability is off and nothing is spent. `model` is
resolved at load (an unknown alias or roster name is a config error naming `media.model`) because the surface
has no client-chosen model: the deployment decides which route reads pictures, so a typo must fail at startup
and not after a 5 MiB body has been base64-encoded. `prompt` is the deployment's default instruction;
`max_body_bytes` is §2.4's bound.

**Why not let the caller name the model?** Because the caller inlines a customer's image and the account pays.
A caller-chosen route would be a spend oracle over the whole roster, and §4.7's guard admits a token holder,
not an operator. The one place a deployment expresses "which model reads pictures" is its config.

### 2.4 A second body bound

`media.max_body_bytes` (default 8 MiB, floor 1 KiB) is checked **in the handler**, before the body is read:
a declared `Content-Length` over it is refused without draining; an undeclared body is read through
`axum::body::to_bytes` with the same bound. Both are `413 request_too_large` with `details` naming
`bound`, `limit_bytes`, `content_length` and `connection: close` (§4.13's reasoning: a refused body may be
undrained, and keep-alive would parse the remainder as the next request).

It cannot be `server.max_body_bytes`: base64 is about a third larger than the file, and the 2 MiB protocol
bound is far below one photograph. Growing the protocol bound instead would raise a limit every cell depends
on for one verb's convenience.

### 2.5 The call is an ordinary forward

`ocr_chat_body` emits an OpenAI chat body — `{model: media.model, stream: false, messages: [user: [text
part, image_url data-URI]]}` — and the route hands it to
`Forwarder::forward_with_client(WireApi::Chat, &body, request_id, &headers, mode, "vadis-media")`: the same
pipeline, the same ledger, the same `/metrics`, the same guard. Consequences, stated so a reader does not
have to derive them:

- `protocol_in == protocol_out == chat`; the bytes recorded as the upstream request are exactly those sent.
- The client's received headers travel with the call (a vision route may need a provider header), while the
  top-level `model` becomes the route's native id — the permitted mutation of constraint 1.
- §4.2's `fallback` walk applies as written. A deployment whose OCR must not silently answer from another
  model leaves `fallback` empty or names the vision route first.
- §4.17's response cache applies unchanged; the key is the synthesized body's digest, which is what makes a
  repeated identical tool call replayable.
- `stream: false` is pinned: the tool answer is one JSON object, and a stream would have to be buffered
  before it could be parsed.

### 2.6 One server-derived client word

`identity.client` gains `vadis-media`. It is **read from no header**: §4.7's path derives the word from
`User-Agent` prefixes against a closed table, and the media route passes this word itself. So ADR-061 D3's
anti-oracle property is untouched — no client can make its traffic look like the deployment's own media
spend — and an operator can still tell the two apart in `/state/events`.

### 2.7 The refusals, and what each leaves behind

Unsupported verb, missing section, unusable image, oversized body and an unauthenticated call are four
distinct arms (plus the guard), each writing **one §6 pre-pipeline record**: `identity.event_id` 0, usage
marked missing, nothing priced, `result.attempts` 0, `protocol.protocol_in` `"media"`, and
`errors[].kind` = `not_implemented` | `invalid_request` | `request_too_large` | `unauthorized`. A call that
reaches the pipeline writes the ordinary chat row and **no extra refusal row** — the refusal class is for
things that never became requests, and a second row for a served call would double-count it.

`ErrorCode::NotImplemented` used to serialize as `kind: "internal"`, which told an operator to hunt a bug the
machinery does not have. It is now `not_implemented`, and §6's set and §8's row say so.

### 2.8 Why audio is a 501 and not a silent approximation

This build has no audio route kind (`WireApi` is closed to chat/responses/anthropic), no multipart reader, no
audio usage or price, and no way to relay a binary provider answer to a tool-shaped JSON answer. Anything
served today would be invented: a shape claimed to work, untested against a provider, unpriced. A typed
`501` says the true thing — the capability is reserved, not present — and §9.3 prefers a declared absence to a
documented-but-unreachable surface.

### 2.9 What this ADR does not claim

No saving, no cost reduction, no quality claim about OCR. The ocr route is priced by the route it names, and
the figures live in `/state/events` like every other request's.

## 3. Alternatives rejected

1. **Leave the proxy where it is.** Cheapest for one day, most expensive for a year: no record, no price, no
   guard, a second port, semantics that drift. Rejected on constraint 3 (the trace is the observation) and on
   the operator cost of two deployments.
2. **Let the client shape the chat body itself** (`/v1/chat/completions` with a data-URI image part). vadis
   already accepts that body — but then every client re-implements the data-URI encoding, the prompt default,
   the answer parse and the media-type validation, and each one can drift. The verb that exists is the one that
   states the shape once.
3. **A general `/v1/tools/*` plugin surface.** More surface than the job needs: a plugin registry, a discovery
   story, a schema per tool. Three routes and one section can be read in a sitting, which is the bar.
4. **Expose a caller-chosen `model`.** Rejected in §2.3: a spend oracle.
5. **Fake audio through the chat cell.** Rejected in §2.8: invented semantics, no usage accounting, wrong
   shape for a client that expects the tool answer.
6. **One shared body bound.** Rejected in §2.4: it would raise the protocol bound for one verb's convenience.

## 4. Consequences

- **Good.** One process instead of two; media spend in the ledger; the guard, the bound, the reload and
  `/metrics` for free; the shapes stated once and tested (`CONF-108`); the shim retires.
- **Cost.** A second body bound (two knobs an operator can confuse, both documented with their reason); a fifth
  client word in a closed vocabulary; three paths whose audio arms are, today, refusals — a reader may mistake
  the 501s for a stub that will never land, so §4.22 and the book say plainly what would have to be built.
- **Neutral.** `/health`, `/metrics`, `/state/events`' shapes are unchanged; the surface is visible through
  the records it writes, which is the point.

## 5. What the operator does

In the deployment that motivated this ADR: write `media:` naming the vision route, restart or reload, point
the client's media base URL at vadis (`http://127.0.0.1:8290`, without a `/v1` suffix — the client appends
`/v1/tools/…`), and stop the second proxy. The client change is a config line; no client code changes because
the shapes are the ones the retired proxy already spoke.
