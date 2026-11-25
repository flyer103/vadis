# Media tools

Status: **served** — `POST /v1/tools/ocr` answers with the configured model's reading of an image, and
`POST /v1/tools/transcribe` / `POST /v1/tools/synthesize` answer on the same JSON shapes by adapting them
to the provider's own `asr`/`tts` endpoints, with `ffmpeg` doing the container conversion. The contract is
[`docs/spec.md` §4.22](../docs/spec.md), the reasoning is
[`ADR-065`](../design/decisions/ADR-065-media-tool-surface.md), and the landing map is
[`design/DESIGN.md` §12.26](../design/DESIGN.md).

The surface exists so that "read this file" is a request the deployment pays for **and can see** — a
`DecisionRecord`, a cost line, the guard, the body bound and the reload — instead of a second service beside
vadis with its own port and no accounting.

## Turn it on

Write one section. It is the only optional section in the config, and leaving it out is a supported state:

```yaml
media:
  model: zai/glm-4.6v       # an alias, or a roster provider/model that reads pictures
  max_body_bytes: 8388608   # default; base64 is about a third larger than the file
  # prompt: "Read every character in this picture."
  # ffmpeg: ffmpeg          # default; the audio arms' converter
```

`model` is required once the section is written, and it is resolved **at load**: an alias that does not exist,
or a `provider/model` that is not in the roster, is a config error naming `media.model` — not a surprise after
a 5 MiB body has been base64-encoded. The section is read per call, so the reload path of
[`docs/spec.md` §4.15](../docs/spec.md) applies a changed `model`, `prompt` or bound without a restart.

**The model is the deployment's choice, not the caller's.** A media call inlines a customer's image and spends
on the account; only an operator names the route that reads pictures.

### The audio arms need a second section

`transcribe` and `synthesize` dial the provider's own endpoints, so they also need a `tools:` block naming
the provider and its two URLs (the same block the [tool relay](tools-relay.md) reads):

```yaml
tools:
  provider: deepseek
  attempt_timeout: 10m      # default 600s — a long TTS call legitimately takes minutes

providers:
  - name: deepseek
    # … the usual urls, api_key_env, models …
    tools:
      asr: https://api.deepseek.com/v1/tools/asr
      tts: https://api.deepseek.com/v1/tools/tts
```

Without `tools:`, without the verb's URL, without a readable credential, or without a working converter, the
audio arms answer `501 not_implemented` **before dialling** and `ocr` keeps serving: nothing else in this
chapter changes for images. `search` and `fetch` are not needed for audio.

## Ask for a reading

```bash
B64=$(base64 < photo.jpg | tr -d '\n')
curl -sS http://127.0.0.1:8290/v1/tools/ocr \
  -H "authorization: Bearer $VADIS_TOKEN" \
  -H "content-type: application/json" \
  -d "{\"image\": {\"media_type\": \"image/jpeg\", \"base64\": \"$B64\"}, \"prompt\": \"这张图中有什么?\"}"
```

```json
{"text": "图中是一栋夜晚拍摄的建筑，招牌上写着 …"}
```

- `image.media_type` is required and must be an `image/*` type; `image.base64` is required and must be
  non-empty. Anything else is `400 invalid_request` naming the field.
- `prompt` is optional. Precedence: your `prompt`, else the config's `media.prompt`, else a built-in
  transcription instruction. The default asks for the **text** in the picture (describing it only when there
  is none) because what a caller usually does with the answer is feed it to another model as a transcript.
- Everyone who can reach the port without a token is refused `401 unauthorized`, like every other route.

## Transcribe audio

```bash
B64=$(base64 < memo.m4a | tr -d '\n')
curl -sS http://127.0.0.1:8290/v1/tools/transcribe \
  -H "authorization: Bearer $VADIS_TOKEN" \
  -H "content-type: application/json" \
  -d "{\"audio\": {\"media_type\": \"audio/mp4\", \"base64\": \"$B64\"}, \"language\": \"zh-CN\"}"
```

```json
{"text": "会议定在周四下午三点，地点还是原来那间。"}
```

- `audio.media_type` is required and must be `audio/*`; the container is never guessed from the bytes.
- `audio.base64` is RFC 4648 with padding, non-empty, and at most the provider's 8 MiB upload bound after
  decoding (`413` past it).
- `language` is an optional tag of at most 35 characters from `[A-Za-z0-9-_]` (`zh-CN`).
- **Your bytes are converted, always**: `media.ffmpeg` turns them into the 16 kHz mono 16-bit WAV the
  provider's ASR accepts, so any container ffmpeg reads (m4a, mp3, ogg, wav …) works. A container ffmpeg
  cannot read is `400`.
- **Long clips are refused, not truncated.** More than 120 s after conversion is `413 request_too_large` — a
  transcript of the first two minutes presented as the whole recording would be a lie.

The provider's answer is its JSON `text`, trimmed, returned as `{"text": …}`. A provider that answers
without one is `502 upstream_error`, never an empty transcript.

## Synthesize speech

```bash
curl -sS http://127.0.0.1:8290/v1/tools/synthesize \
  -H "authorization: Bearer $VADIS_TOKEN" \
  -H "content-type: application/json" \
  -d '{"text": "你好，这是一段语音。", "format": "mp3", "voice": "stella"}'
```

```json
{"audio": {"base64": "SUQzBAAAAAA…", "media_type": "audio/mpeg"}}
```

- `text` is required and must be non-blank; over the provider's 10,000-character bound is `413`.
- `format` is `mp3` (default), `wav` or `pcm`; any other value is `400` naming the supported set.
- `voice` is the provider's voice name (`stella`, `tide`); absent means `stella`.
- The provider speaks `wav` or raw `pcm` only, so `mp3` is transcoded after the answer arrives. For that
  reason a host without ffmpeg still serves `wav` and `pcm`; only the `mp3` request is `501` there.

The answer is the audio itself as base64 plus its media type (`audio/wav`, `audio/pcm`, `audio/mpeg`). A
provider that answers with no audio is `502 upstream_error`.

## Bounds and the converter

- A body over `media.max_body_bytes` (for the whole surface, audio included) is `413 request_too_large` with
  `details` naming the bound, the limit and the declared length, and the connection is closed — a refused body
  may be undrained. The bound is separate from `server.max_body_bytes` on purpose: that 2 MiB limit is the
  protocol cells' and a photograph or a recording does not fit it.
- The audio arms also bound the **provider's** answer by `tools.max_response_bytes` and the dial by
  `tools.attempt_timeout`; an answer past the bound is `502`, never a truncation.
- `media.ffmpeg` is a name `PATH` resolves at call time. The probe (`<ffmpeg> -version`) is cached per name
  for the process, so pointing the key at a different binary re-probes, while replacing the binary behind the
  same path needs a restart. On a host with no ffmpeg at all, `ocr` still works.

## What it costs, and what a record shows

A served `ocr` call is an **ordinary chat request** inside vadis: it goes through the same selection, the same
`fallback` walk, the same response cache and the same ledger, against the route `media.model` names, with the
top-level `model` rewritten to that route's native id. So the spend shows up in `/state/events`, in
`/health` and in `vadis stats` exactly like any other request — and `identity.client` reads
`vadis-media`, a word no client can produce from its `User-Agent`.

Watch the fallback: a media route is a route, so a `fallback` chain applies to it as written. If OCR must
never silently answer from a different model, leave `fallback` empty or name the vision route first.

The **audio arms are not chat requests**: they dial one provider URL once, with no route selection, no
`fallback` walk and no cache. Their record is a tool row — `protocol.protocol_in: "media"`, the `tools` group
(`verb`, `provider`, `upstream_status`, `request_bytes`, `response_bytes`, `priced: false`) and
`usage_missing: true`. The cost is zero **because the surface is unpriced, never because the call was free**.

Every refusal **before** a dial leaves one record too — `identity.event_id` 0, no usage, nothing priced,
`protocol.protocol_in: "media"`, `errors[].kind` one of `not_implemented`, `request_too_large`,
`invalid_request`, `unauthorized`. A refusal that never became a request should not look like one.

**A provider's own refusal is not reshaped.** On the audio arms a provider `4xx`/`5xx` comes back verbatim
— its status, content type and error body — because its explanation (a content filter, a quota, a voice it
does not know) is more useful than a generic envelope. vadis-side failures keep the usual §8 shape.

## What is not built

- **Audio pricing.** The audio arms write `priced: false`; there is no verified price source yet, so the
  zero is a gap, not a gift.
- **Audio streaming.** Both arms answer one buffered JSON object. A client that wants the provider's streaming
  `tts` talks to the [tool relay](tools-relay.md) directly, which relays any body.
- **The provider's native multipart `ocr`**, more than one image per request, page ranges, or a streaming
  answer. One file in, one text out.

## Wiring a client

A client that already speaks the retired proxy's shapes needs one line: point its media base URL at vadis.
The client appends `/v1/tools/…`, so the base URL carries **no** `/v1` suffix:

```yaml
media:
  base_url: http://127.0.0.1:8290
```

`kian`'s media edge is that client (`image_ocr` / `audio_transcribe` / `speech_synthesize` in
`kian-media`); it sends the shapes this chapter shows and reads `{"text"}` or `{"audio"}`. With `media:`
written here and a `tools:` block for audio, the second proxy that used to sit on `127.0.0.1:8291` has nothing
left to do.
