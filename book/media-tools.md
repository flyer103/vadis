# Media tools

Status: **`ocr` is served; the two audio verbs are registered refusals.** `POST /v1/tools/ocr` takes an image
and answers with the configured model's reading of it — the verb a client edge needs when it has a file and
no chat body. `POST /v1/tools/transcribe` and `POST /v1/tools/synthesize` answer `501 not_implemented`, because
this build has no audio route kind. The contract is
[`docs/spec.md` §4.22](../docs/spec.md), the reasoning is
[`ADR-065`](../design/decisions/ADR-065-media-tool-surface.md), and the landing map is
[`design/DESIGN.md` §12.26](../design/DESIGN.md).

The surface exists so that "read this picture" is a request the deployment pays for **and can see** — a
`DecisionRecord`, a cost line, the guard, the body bound and the reload — instead of a second service beside
vadis with its own port and no accounting.

## Turn it on

Write one section. It is the only optional section in the config, and leaving it out is a supported state:

```yaml
media:
  model: zai/glm-4.6v       # an alias, or a roster provider/model that reads pictures
  max_body_bytes: 8388608   # default; base64 is about a third larger than the file
  # prompt: "Read every character in this picture."
```

`model` is required once the section is written, and it is resolved **at load**: an alias that does not exist,
or a `provider/model` that is not in the roster, is a config error naming `media.model` — not a surprise after
a 5 MiB body has been base64-encoded. The section is read per call, so the reload path of
[`docs/spec.md` §4.15](../docs/spec.md) applies a changed `model`, `prompt` or bound without a restart.

**The model is the deployment's choice, not the caller's.** A media call inlines a customer's image and spends
on the account; only an operator names the route that reads pictures.

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

Sizes: a body over `media.max_body_bytes` is `413 request_too_large` with `details` naming the bound, the
limit and the declared length, and the connection is closed — a refused body may be undrained. The bound is
separate from `server.max_body_bytes` on purpose: that 2 MiB limit is the protocol cells' and a photograph
does not fit it.

## What it costs, and what it looks like

A served call is an **ordinary chat request** inside vadis: it goes through the same selection, the same
`fallback` walk, the same response cache and the same ledger, against the route `media.model` names, with the
top-level `model` rewritten to that route's native id. So the spend shows up in `/state/events`, in `/health`
and in `vadis stats` exactly like any other request — and `identity.client` reads `vadis-media`, a word no
client can produce from its `User-Agent`.

Watch the fallback: a media route is a route, so a `fallback` chain applies to it as written. If OCR must
never silently answer from a different model, leave `fallback` empty or name the vision route first.

Every refusal **before** the pipeline leaves one record too — `identity.event_id` 0, no usage, nothing
priced, `protocol.protocol_in: "media"`, `errors[].kind` one of `not_implemented`, `request_too_large`,
`invalid_request`, `unauthorized`. A refusal that never became a request should not look like one.

## What is not built

- **Audio.** `/v1/tools/transcribe` and `/v1/tools/synthesize` are registered so a client gets a typed `501`
  instead of a `404`; nothing reaches an upstream. Serving them needs a route kind for audio, a multipart
  reader, audio usage normalization and a price — a change of its own, not a hidden behaviour here.
- **More than one image per request**, page ranges, or a streaming answer. One image in, one text out.

## Wiring a client

A client that already speaks the retired proxy's shapes needs one line: point its media base URL at vadis.
The client appends `/v1/tools/…`, so the base URL carries **no** `/v1` suffix:

```yaml
media:
  base_url: http://127.0.0.1:8290
```

`kian`'s media edge is that client (`image_ocr` / `audio_transcribe` in `kian-media`); it sends
`{"image": {"media_type", "base64"}}` and reads `{"text"}`. With `media:` written here, the second proxy that
used to sit on `127.0.0.1:8291` has nothing left to do.
