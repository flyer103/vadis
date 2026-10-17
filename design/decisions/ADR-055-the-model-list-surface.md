# ADR-055 — the model list surface: `GET /v1/models`

- Status: **accepted, contract only** (R73-0, 2026-10-17). The contract below is frozen; the route
  that serves it lands in a **separate** change (`R73-1`), and this ADR says exactly what that change
  owes. Until it lands, the endpoint answers `404` and `CONF-107` is parked `#[ignore]`d.
- Kind: **a contract change with no product byte.** It adds one guarded read-only endpoint's
  contract, one conformance case file (parked), and the register entries; `crates/` is untouched.
- Owners: repo owner (authorisation: chat ruling 2026-10-17, *"open a round R73"*).
- Supersedes: nothing. §4.7's guard rule, §4.15's reload rule, §4.16's and §4.18's surfaces and the
  byte boundary (AGENTS constraint 1) are all **unchanged**; this endpoint reads the roster and
  writes nothing, so not one of them is touched.

## 0. Context — the client question this answers

A vadis deployment has **no** model-discovery surface. `GET /v1/models` is not a route:
`crates/vadis-cli/src/lib.rs:1067-1100` merges `/v1/chat/completions`, `/v1/responses`,
`/v1/messages`, `/metrics`, `/state/events` and `/health`, so a request for `/v1/models` falls through
to axum's `404`. Measured on the owner's tree (2026-10-17, board `router`, HEAD `d5854c0`):
`GET /v1/models → 404`, `GET /v1/responses → 405` (POST `400`), `GET /health → 200`.

The consequence is not only a missing convenience. `codex doctor`'s **reachability** probe is a
`GET <base_url>/models` request — measured by the orchestrator on the owner's machine with a
purpose-built mock that answers that path and `405`s everything else (port 8291): doctor flips from
`1 fail` to `0 fail`, and the mock's log holds exactly one line, `GET /v1/models`. Against the real
gateway doctor has long displayed `✗ reachability … route returned 404 (required)` while the
reasoning chain itself was healthy — a client-side health check reading the gateway as broken.

Two further facts bound the design:

- `aliases:`'s right-hand side is `provider/model` (`RouteSpec { provider, model }`,
  `crates/vadis-core/src/config.rs:226`), and the owner's live roster pairs alias names that carry
  account semantics with entries whose `account:` distinguishes a coding plan from a metered API.
  An index that echoed the route would publish the deployment's account topology to every client
  holding the token.
- `aliases` is a `BTreeMap<String, RouteSpec>` (`crates/vadis-core/src/config.rs:1099`), so the alias
  namespace has one natural, stable order — the name's bytes — and needs no invented sort key.

## 1. Decision

**One guarded, read-only endpoint: `GET /v1/models` answers the configured alias names as an
OpenAI-compatible model list.** Eight decisions, each one observable (spec §4.19 is the full
contract; the wording there governs):

1. **Method face: `GET` only.** Any other method on the path answers `405` with an `allow` header naming
   `GET` — no §8 error body, **no trace record**, no event row. The `405` is stated as a decision, not as
   a consequence of axum's defaults: it is the behaviour the contract fixes, and the case asserts it.
   (`HEAD` is answered as `GET` is with the body removed — axum's `get` route serves it. That is stated
   in spec §4.19 so a reader is not surprised, and deliberately **not** asserted: it is not a variant of
   the contract.)
2. **Auth precedence, stated.** The guard runs at the route layer, so it decides **before** method
   dispatch: a non-`GET` **without a valid token** is the guard's `401` (with its one record), never
   a `405`. This is the documented difference between axum's `Router::route_layer` (path-matched; its
   doc: *"useful for middleware that return early (such as authorization) which might otherwise
   convert a `404 Not Found` into a `401 Unauthorized`"*) and `MethodRouter::route_layer`
   (method-matched; its doc's own example answers an unauthenticated `POST` with `405 Method Not
   Allowed`, because that layer wraps the per-method endpoints and not the method fallback) — the two
   surfaces already built this way (`guarded_metrics_route`, `guarded_state_route`) use the former,
   and this route joins them. Asserted in both directions, so a later wiring change that quietly
   flips the order is caught.
3. **Body: the OpenAI list shape, four keys per entry, alias names only.**
   `{"object":"list","data":[{"id":…,"object":"model","created":…,"owned_by":…}]}`. `id` is the alias
   name **verbatim**; the resolved `provider/model` appears **nowhere** in the body — not as `id`,
   not as a member, not in an error. `created` is the **constant `0`** and `owned_by` the **constant
   `"vadis"`** (reasons in §2).
4. **Order and byte stability, asserted.** `data[]` is in **ascending byte order of `id`** (the
   `BTreeMap`'s own order), and two calls against one revision are **byte-identical**. The ordering
   is a contract, not an implementation detail a reader may infer: a later `HashMap` would change
   the body and must fail the case.
5. **Guard: the same `guard_mw`, word `"models"`.** `GuardState::proto_in = "models"`, so a refusal
   writes the same one pre-pipeline record a refused scrape or state read writes, and a §6 reader
   can filter on the vocabulary `{chat, responses, anthropic, metrics, state, models}`. spec §4.7's
   word list gains `"models"`.
6. **The admission's observation cost is zero, and that is a boundary in its own right.** The `200`
   arm writes **no trace record, no event row, no cost, no session, no prefix block** — the endpoint
   is not on the decision pipeline and adds no stage to it. A refusal writes exactly the guard's one
   record. Stated as a stage-set assertion (§5), not as "the body is fine".
7. **One request, one revision.** The handler answers from the revision the request **captured at
   receive** (ADR-040 D2's capture-once rule, already carried by `guard_mw` into the request's
   extensions), read as `Revision.forwarder.config.aliases` — never a second capture from the cell.
   `aliases` is a reloadable face (spec §4.15), so after an accepted reload the list follows the new
   revision **with no restart**, while a request already in flight answers from the revision that
   admitted it.
8. **Boundary shapes.** An empty `aliases:` roster answers `200` with `{"object":"list","data":[]}`
   — not `404`, not `500`. No query parameter is read (a query string is ignored, so an
   OpenAI-compatible client that appends `limit` is not refused), there is no pagination, and there
   is no single-model retrieval.

## 2. The body, and why each constant is a constant

```json
{"object":"list","data":[
  {"id":"chat-fast","object":"model","created":0,"owned_by":"vadis"},
  {"id":"coding-bulk","object":"model","created":0,"owned_by":"vadis"},
  {"id":"coding-fast","object":"model","created":0,"owned_by":"vadis"}
]}
```

- **`created: 0` — because vadis holds no per-alias creation instant.** The OpenAI `Model` object
  has this member, so omitting it invites a client-side deserializer to fail; inventing a value
  invites a reader to believe it. `0` is the one value that reads as *no date*. It is also the only
  value that keeps the response a **pure function of the roster** (AGENTS constraint 2 / spec §4's
  determinism discipline): the wall clock, the process start, the config file's mtime and a build
  timestamp were each rejected — the first three make two calls differ over an unchanged
  configuration, the fourth makes the body depend on when the binary was built.
- **`owned_by: "vadis"` — because the alias namespace is vadis's, not the provider's.** The operator
  names aliases; the provider only appears in the (never exposed) route. Naming the provider here
  would reintroduce, one key at a time, exactly the leak decision 3 refuses.
- **`object: "list"` / `object: "model"` — the shape a client already parses.** No new vocabulary.

## 3. Why this route is **not** a §4.7 exemption

§4.7's exemption is `/health`'s alone, and it is **structural**: the guard is applied to each
guarded route's own layer, so no path comparison inside the guard can ever mis-exempt a route.
`/health` earns it because a supervisor's liveness probe **cannot carry a credential** and answers
only *"is this process up"*. `GET /v1/models` answers a different question — *"which names will this
deployment route"* — and an unauthenticated answer is a roster inventory: the alias names are the
operator's own client-facing vocabulary, and their set is a fact about the deployment, not about
liveness. It is also not a structural probe: whatever asks for it is a **client** with a base URL
and (when inbound auth is on) a token available to it — the same client that sends completions.

Consequences, stated so no later reader re-opens it: the route is guarded, its refusal carries the
word `"models"` in `protocol.protocol_in`, and the token is read in the two accepted forms of §4.7
(`Authorization: Bearer`, `x-api-key`).

**What this ADR does not claim:** that guarding the endpoint leaves `codex doctor` green. Codex is
configured with `env_key`, so the credential is available to it, but whether its reachability probe
attaches it is a **client** fact this contract cannot assert from here. If the probe is sent
unauthenticated, it will read `401` and doctor will keep reporting a failed reachability check —
the honest remedy is the client's (export the named variable), never a silent exemption of the
route. `R73-1` measures it and records the answer.

## 4. What the surface may not carry (the export boundary)

The same discipline as §4.16 and §4.18, one surface further. The body is `object` plus `data[]`;
anything a future round adds is a **contract change**, not an extension.

| Not carried | Why not |
|---|---|
| per-model metadata: context window, price, the declared `supports` cells | Price has **one** source — the roster, §4.0 — and copying it into a second surface creates the drift §4.0 exists to prevent (and AGENTS constraint 5's no-fabricated-prices rule). Context and the declared cell set are wiring facts whose refusal behaviour lives in the protocol path (§4.9, ADR-051); an index that mirrored them would have to stay in step with every load-time refusal, for no client need this round has. |
| the provider list, the `account:` classification, the plan families | The roster's topology is exactly what decision 3 refuses to publish: it distinguishes a coding plan from a metered API for every client holding the token. |
| the resolved route (`provider/model`) in any member | Same reason, stated as its own row because it is the one value a well-meaning implementer will reach for: the alias points at it, and echoing it makes the surface an account map. |
| `GET /v1/models/{id}` (single-model retrieval) | Retrieval turns the endpoint into an **alias-name oracle** — a `404` becomes a query whose answer distinguishes "not configured" from "not routed" — and adds a second status set for one client need (discovery plus reachability) the list already serves. |
| pagination / `limit` / `after` / `?fields` | The roster is a configuration map, not a growing collection: a page contract needs an order and a cursor the configuration does not have. A query string is therefore **ignored** rather than refused (an OpenAI-compatible client appends one without meaning anything by it), and the body is the whole roster every time. |
| any `/metrics` counter | The endpoint reads the roster, not traffic. A counter ("how often was the index probed") is a **§4.16 contract increment** with its own semantics and its own `provenance` label — it is not added here, and this ADR adds no series. |
| a per-call `X-Vadis-Request-Id` on the `200` arm | There is no request record to correlate: this route allocates no request. `/metrics` and `/state/events` already have exactly this shape, and `CONF-106` asserts it. The refusal keeps the header (§8's "always" is about refusals, and a refusal is the case an operator needs to correlate). |

## 5. Conformance

`CONF-107` (`tests/conformance/tests/conf_107_v1_models_lists_aliases.rs`, parked
`#[ignore = "CONF-107: depends on R73-1"]`), through the real `serve` assembly over loopback HTTP:

- **(a)** a three-alias roster answers `200` with **exactly** three `id`s, each entry's keys exactly
  `{id, object, created, owned_by}`, `created: 0`, `owned_by: "vadis"`, and `data[]` in ascending
  `id` order; two calls are **byte-identical**.
- **(b)** the route strings are absent: with the roster's provider named `zai-cn-plan` and its model
  `glm-5.3`, the body contains neither, no `provider/model` string appears at all, and no `id`
  contains `/`.
- **(c)** no token ⇒ `401` with §8's body (`details.header: null`), `X-Vadis-Request-Id` present, and
  **exactly one** pre-pipeline record whose `protocol.protocol_in` is `"models"` and whose
  `errors[].kind` is `"unauthorized"`; a wrong token ⇒ the same with `details.header:
  "authorization"`.
- **(d)** an empty `aliases:` roster ⇒ `200` with `object: "list"` and `data: []`, and the body's key
  set is exactly `{object, data}`.
- **(e)** the `200` arm's stage set is empty: over two admitted calls the trace directory gains no
  line, and after `serve` stops the store's event log holds only the boot's `config.applied` row —
  asserted as a total row count (`rows.len()` equals that one row), since the live store is not
  re-opened mid-run for a before-read.
- **(f)** `POST` **with** the token ⇒ `405` with an `allow` header naming `GET` and no record; `POST`
  **without** the token ⇒ `401` and one record (decision 2's precedence, both directions).
- **(g)** one accepted reload that changes the alias set ⇒ the same process, the same port, the new
  list, **no restart** (the reload observed through `/health`'s `config_digest`, the technique
  `CONF-96` uses).

## 6. Landing plan (`R73-1` owes this, and only this)

| File | Change |
|---|---|
| `crates/vadis-core/src/config.rs` | **nothing** — `aliases: BTreeMap<String, RouteSpec>` is already the read this surface needs |
| `crates/vadis-proxy/src/models_read.rs` (new) | the body as a **pure function** of `&BTreeMap<String, RouteSpec>` (the projection and the four constants), plus its unit tests including the empty-map case |
| `crates/vadis-cli/src/lib.rs` | `guarded_models_route` beside `guarded_metrics_route`/`guarded_state_route`: a `get` route, **no body extractor and no body layer** (the route reads no request byte, so §4.13's bound is not installed), the same `guard_mw` with `proto_in: "models"`, and the handler reading the revision the guard captured into the request's extensions |
| `docs/spec.md` | §4.19's status line flips *contracted* → *served*; §9.3's exception paragraph is removed (the surface is no longer documented-but-unreachable) |
| `design/DESIGN.md` | §12.9's register line records the landing; §12.8's `CONF-107` row is unchanged (an ID is the contract) |
| `book/connecting-clients.md`, `README.md` | the sentence and the table row flip from "contracted, not served" to the served fact |
| `tests/conformance/tests/conf_107_v1_models_lists_aliases.rs` | the `#[ignore]` attribute is removed and the seven limbs must pass |

## 7. Alternatives considered

| Alternative | Why rejected |
|---|---|
| list `provider/model` routes as `id` (the "obvious" expansion) | Publishes the account topology (decision 3's leak) **and** silently promises that every listed string is a routable model — while several roster models are deliberately not aliased. The alias namespace is the operator's client-facing vocabulary; the route is wiring. |
| list the roster's models (and not aliases) | Then `id` becomes `provider/model` for every entry, which is the row above, and the operator's own names — the thing clients are configured with — disappear from the surface meant to describe them. |
| no auth on this route (a "public" discovery endpoint) | §3: the alias set is a deployment fact, not a liveness fact, and the route is not a supervisor probe. |
| answer through `/health` (no new route) | `/health` is exempt from auth (§4.7) and answers liveness; overloading it with the roster would publish the alias namespace unauthenticated, and would give a supervisor probe a body it must not need. |
| answer through `/metrics` | `/metrics` renders §9.2's figures over a 900-second window; a roster list is not a figure, has no window and no provenance label, and adding it would be a §4.16 contract reversal. |
| serve a `models.json` catalog per model (context, capabilities) | That is codex's **client-side** catalog (`model_catalog_json`), produced by the operator and read by codex; a gateway-side copy would be a second source of capability facts (see §4's first row) with no consumer that the client-side file does not already serve. |
| return `created` from the clock (or the process start) | Two calls over an unchanged roster would differ, and the value would mean nothing — §2's constant is the same decision §4's determinism discipline makes everywhere else. |
| `GET /v1/models/{id}` | §4's retrieval row. |
| land the route and this contract in one change | The owner's ruling splits the round: the contract is reviewed on its own (`R73-0`), the wiring lands against a frozen contract (`R73-1`), and the case is parked `#[ignore]`d in between so the suite is never red. This is the repo's own parking rule (`CONF-20…25`), applied to a surface. |

## 8. Honest boundaries

- **The contract is ahead of the code, and spec §9.3 calls that a defect.** §9.3's rule is *"a
  surface's shape is frozen by the change that implements it, and a documented-but-unreachable
  surface is a defect"*. This round writes the shape one change earlier, on the owner's ruling, and
  pays for it explicitly: §4.19's status line says **contracted, not served**, §9.3 registers the
  bounded exception, the book and README say the same in the user's own words, and `CONF-107` is
  parked with the implementing card named in its `#[ignore]` reason. The exception ends when `R73-1`
  lands; nothing else may be added to it.
- **A `200` here does not say the routing works.** The endpoint dials no upstream: it says the
  process is up, the revision is published and the roster has these alias names. Doctor's
  reachability check is exactly that weak, and this contract does not strengthen it.
- **Discoverability is bounded by the alias namespace.** A model reachable as `provider/model` is
  servable (spec §2/§3) and **not** listed. That asymmetry is deliberate (decision 3) and is written
  into the book, so an operator who wants a route discoverable writes an alias for it.
- **Nothing is claimed about clients other than codex.** The OpenAI-compatible discovery convention
  is what the shape follows; no other client's behaviour was measured for this round.
- **No figure, no price, no capability fact** is exposed here, so no §9.2/§4.16 accounting statement
  moves with this surface.

## 9. Register updates

- `docs/spec.md`: **§4.19** (the endpoint's contract, with the status line naming this ADR), **§4.7**
  (the guarded non-protocol route list and the `protocol_in` vocabulary gain `/v1/models` and
  `"models"`), **§9.3** (the bounded, dated exception paragraph).
- `design/DESIGN.md`: **§12.8** (the heading `CONF-01…CONF-106` → `CONF-01…CONF-107`, the `CONF-107`
  row, the allocation paragraph), **§12.9** (the register line for this round).
- `book/connecting-clients.md` (the endpoint list's own sentence), `README.md` (the feature table's
  row and the API table's row, both marked *contracted, not served yet*).
- `tests/conformance/tests/conf_107_v1_models_lists_aliases.rs` (new, parked).
- `design/decisions/ADR-055-the-model-list-surface.md` (this file).
- `crates/`: **nothing**.
