# Tool relay

Status: **`search`, `fetch`, `tts` and `asr` are served when the deployment configures the provider URLs.**
`POST /v1/tools/{search,fetch,tts,asr}` send the caller's bytes to one provider endpoint and return that
endpoint's answer, unchanged in either direction. The contract is
[`docs/spec.md` §4.23](../docs/spec.md), the reasoning is
[`ADR-066`](../design/decisions/ADR-066-tools-surface.md), and the landing map is
[`design/DESIGN.md` §12.27](../design/DESIGN.md).

The surface exists for the verbs that are **not model calls**: a web search, a page fetched as Markdown, a
sentence spoken to audio, an audio file transcribed. None of them has a message array or a usage block to
normalize, so vadis does not read the body at all — it adds the provider's credential, forwards the bytes,
and writes the ledger row that makes the spend visible. That is the whole difference from
[Media tools](media-tools.md), where vadis writes the body because the caller has none.

## Turn it on

Two blocks: one top-level section naming the provider, and one block of complete URLs on that provider's
roster entry. Both are optional, and the default — neither written — answers `501 not_implemented` on all
four paths:

```yaml
tools:
  provider: deepseek            # the roster entry that sells the verbs
  max_body_bytes: 9437184       # default 9 MiB
  max_response_bytes: 16777216  # default 16 MiB
  attempt_timeout: 10m          # default 600s: the dial's own bound
```

```yaml
providers:
  - name: deepseek
    tools:
      search: https://api.deepseek.com/v1/tools/search
      fetch:  https://api.deepseek.com/v1/tools/fetch
      tts:    https://api.deepseek.com/v1/tools/tts
      asr:    https://api.deepseek.com/v1/tools/asr
```

The blocks are read **per verb**, and that is deliberate: declaring `search` promises nothing about `asr`,
because providers differ in which verbs they sell. A verb with no URL answers `501` naming the missing key
rather than dialling a guessed path. Everything is validated at load: a `tools.provider` that is not a
roster entry, an entry with no verb URL, and a URL that is not `http(s)` all refuse the file (spec §4.15),
and a provider that holds no readable credential answers `501` before a byte of the request is read.

## Ask for something

The body is the provider's own contract — vadis neither parses nor validates it, and the provider's
documentation is the reference for each verb:

```bash
curl -sS http://127.0.0.1:8290/v1/tools/search \
  -H "authorization: Bearer $VADIS_TOKEN" \
  -H "content-type: application/json" \
  -d '{"query": "vadis tool relay", "count": 3}'

curl -sS http://127.0.0.1:8290/v1/tools/fetch \
  -H "authorization: Bearer $VADIS_TOKEN" \
  -H "content-type: application/json" \
  -d '{"url": "https://example.com/some/page"}'

curl -sS http://127.0.0.1:8290/v1/tools/tts \
  -H "authorization: Bearer $VADIS_TOKEN" \
  -H "content-type: application/json" \
  -d '{"text": "你好，这是一段语音。", "voice_id": "stella", "format": "wav"}' \
  -o hello.wav

curl -sS http://127.0.0.1:8290/v1/tools/asr \
  -H "authorization: Bearer $VADIS_TOKEN" \
  -F 'file=@hello.wav'
```

Three details a caller should know:

- **The answer is the provider's**, byte for byte: `search` returns its JSON, `fetch` a Markdown page, `tts`
  an `audio/wav` body (write it to a file; it is not JSON), `asr` its transcript object. vadis adds
  `x-vadis-request-id` and forwards the upstream's `content-type`; nothing else.
- **Your `content-type` travels as received.** `curl -F` produces `multipart/form-data; boundary=…`, and that
  header is the boundary's only home — rewriting it would be a body the provider cannot parse.
- **The provider's errors reach you untouched.** A `400`, `422`, `429` or `503` comes back with the
  provider's own status, `content-type` and error body: its `error.type`, `code` and `param` name the field
  to fix, and re-wrapping them in vadis's envelope would delete exactly that. Only vadis's own refusals
  (`401` unauthenticated, `413` oversize body, `501` unserved verb, `502` no answer or oversize answer) use
  vadis's error shape.

## Sizes and time

- **`tools.max_body_bytes`** (default 9 MiB) is checked header-first: a declared `content-length` above it is
  `413 request_too_large` before a byte is read, and the connection is closed so an undrained body cannot
  parse as the next request. The bound is separate from `server.max_body_bytes` (2 MiB, the protocol cells'):
  an audio upload does not fit that one.
- **`tools.max_response_bytes`** (default 16 MiB) bounds the answer. Above it the call is `502 upstream_error`
  naming the bound and the observed size — **not** a truncated relay: a cut page or a cut WAV is silent
  corruption, and the provider has already billed it.
- **`tools.attempt_timeout`** (default 600s) bounds the dial and nothing else. It is separate from
  `server.upstream_attempt_timeout` on purpose: 120 seconds is right for a model call and wrong for a
  paragraph of speech.
- **One attempt, no retry, no failover.** A replayed tool call is a second bill on the account, and no two
  providers' tool bodies are interchangeable. A dial that fails after the request was fully written says so
  in the error — *the call may have been billed* — because that sentence decides whether a retry is safe.

## What it costs, and what the record shows

One `DecisionRecord` per call, with a `tools` group:

```json
"tools": {"verb": "search", "provider": "deepseek", "upstream_status": 200,
          "request_bytes": 34, "response_bytes": 812, "priced": false}
```

- `protocol.protocol_in` is `tools`, `protocol_out` is `null`, `decision.model` is empty and
  `transform_mode` is `passthrough`: a verb names an endpoint, not a route, so nothing was selected,
  transformed or cached — `fallback`, `plan_policies` and the response cache do not apply to this surface.
- `upstream_status` records that an answer arrived and what it was; it is absent when nothing came back. A
  relayed `4xx`/`5xx` therefore has both the provider's bytes for the caller and `errors[].kind =
  upstream_error` for the ledger.
- `priced` is **false** in this build, with `usage_missing: true` and an all-zero `cost` whose currency is
  still set. That combination says **unpriced**, never **free**: vadis has no price table for these verbs yet
  (spec §7's rule — an unmeasured number is not a zero).
- `identity.client` is your own client word. Unlike [Media tools](media-tools.md)'s `vadis-media`, the bytes
  here are yours, so the client that pays is the client that called.

## What is not built

- **Tool pricing and quota.** No cost line, no budget that bounds the verbs; `priced: false` is where a
  reader sees that.
- **Streaming TTS.** The answer is buffered (and bounded); a WebSocket or a progressively relayed audio body
  is a later change, because *delivered bytes* and *billed bytes* need their own accounting.
- **A provider's multipart `ocr`.** `/v1/tools/ocr` is the JSON surface of [Media tools](media-tools.md); the
  provider's own page-shaped `ocr` takes multipart and is not relayed under the same path — one path may not
  mean two contracts.
- **Cache and fallback on this surface.** A tool call is neither idempotent nor interchangeable.

## Wiring a client

Point the client's tool base URL at vadis; it appends `/v1/tools/…`, so the base carries **no** `/v1`
suffix:

```yaml
base_url: http://127.0.0.1:8290
```

A client that already speaks the provider's tool shapes needs nothing else — the path, the body and the
answer are unchanged. A client that speaks [Media tools](media-tools.md)' `ocr` shape uses the same base URL
and the same `/v1/tools/…` prefix: the two surfaces share a prefix and differ in contract, which is why
`ocr` is documented there and the four verbs here.
