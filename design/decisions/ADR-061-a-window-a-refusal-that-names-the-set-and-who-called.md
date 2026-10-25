# ADR-061 — a window on the model list, a refusal that names the roster, and a client we can name

- Status: **accepted (2026-10-26), contract only.** It amends `docs/spec.md` (§4.19, §8, §3, §6) and strikes
  two `design/DESIGN.md` promises; it writes **no** product byte, **no** test, **no** conformance case, **no**
  price and no gate/threshold/corpus. One amendment it needs inside the frozen measurement is named in §2.7
  and **not** taken here — AGENTS constraint 9 makes that the owner's act.
- Kind: three decisions about **what a client may learn from vadis about what it can ask for and who it is** —
  one the response body of `GET /v1/models`, one the response body of a `404`, one the record's `identity`
  group. One ADR because they share one question: *the deployment already knows these facts; which of them may
  leave the process, on which surface, in which words?*
- Authority: the owner's card of 2026-10-26 (`U1`), three rulings, answering three measured findings of one
  week of production traffic (4,432 requests, 2026-10-17 → 10-23).
- Amends: **§4.19** (the model list: a fifth key, its source, its value, and the one sentence of the export
  boundary that refused it), **§8** (the `404` body: one word, one shape, the resolvable set, the forbidden
  strings), **§3** (the two sentences that describe a bare id's refusal), **§6** (the `identity` group:
  `client` derived from a closed vocabulary, `thread_id` declared null, the raw header never recorded).
- Supersedes: **ADR-055 §4's `context window` row, and only that row.** ADR-055 refused per-model metadata
  there on grounds that were *conditional* — *"for no client need this round has"*. The need was measured one
  week later (§1). Price and the declared `supports` cells stay refused for the reasons ADR-055 gave; they are
  not weakened by this ADR.
- Related: **ADR-055** (the list surface, its guard, order and byte stability — untouched here), **ADR-005**
  (the trace is the product), **ADR-004 item 3** (stateful inbound: the ruling that `thread_id` would have to
  arrive with), **ADR-019/ADR-041** (a body a reader can act on is a contract, not a message), **ADR-012 /
  AGENTS 9** (the measurement is not part of the search space), **ADR-058** (one word, one meaning).

## 0. The three rulings in one paragraph each

**D1 — a model entry declares its window, and a client reads the one that will serve it.** Each `data[]` entry
of `GET /v1/models` gains **one** key, `context_length`, an integer count of tokens: the `context` the roster's
own model entry declares for the route the alias resolves to. The number is **declared, not derived** — it is
already a required roster fact (`providers[i].models[j].context`), and the alias publishes the number of the
route it will actually reach, which is the entire point: the client's warning was *"`coding-fast` … falling back
to 256,000"* while the route behind that alias is 1 M. Nothing about a request body moves: this is a response,
on a surface that never forwards anything (constraint 1 is not engaged at all, §2.5).

**D2 — a refusal may name the set the deployment can resolve, and nothing else.** Every client-formed
resolution failure answers **one** word (`unknown_model`) and **one** body shape, whose `details` names the
requested string and the resolvable aliases — at most **8** of them, in the list's own byte-ascending order,
with the roster's total beside them so a truncation is visible rather than silent. Provider names,
provider-native ids and `provider/model` strings may **never** appear, in any member. The set is disclosed
because the refusal is behind the same guard as the list and is the *same* disclosure ADR-055 §3 already
makes to the same credential — and the **why** of the failing branch is withheld, because telling a client
*which* branch it hit is a roster-structure oracle the guarded list deliberately does not offer.

**D3 — `identity.client` is derived from `User-Agent` into a closed vocabulary, and `thread_id` stops
pretending.** The vocabulary is DESIGN §12.3's four words (`codex`, `hermes`, `claude-code`, `other`), derived
once per request, at the boundary, by one pure function; an absent or unmatched header is `other`, so every
existing row stays comparable. The raw header is **never** recorded — not verbatim, not truncated, not hashed —
and the `client_ua_raw` field DESIGN promised in two places is struck. `thread_id` is **permanently null in
v0.1**: the only string that could populate it is `previous_response_id`, which §2/§6 do not read (gap G-F) and
whose reading would also flip `state.stateful_inbound` — ADR-004 item 3's arrival, a separate contract change
this round does not pre-empt.

## 1. Context — what the week measured, and the two structural facts underneath

From `~/docs/vadis/audits/week-2026-10-17_24/REPORT.md` (read-only audit of the live gateway's own artifacts):

| finding | evidence in the report |
|---|---|
| **the model list carries no context length**, so the client warns on every session start — *"Could not determine context length for model 'coding-fast' … falling back to 256,000"* — while the route it resolves to is 1 M | §5 U1: `GET /v1/models` returns 5 ids and no limits; the warning repeats all week in `~/.hermes/logs/gateway.error.log` |
| clients guess model names | §2: 9 × `404 unknown model or alias` (`gpt-5.4` ×8, `glm-5.1` ×1) |
| no per-client attribution exists | §1: `identity.client = "other"` on **100 %** of 4,432 rows; `thread_id` null on 100 % |

Three structural facts decide the three rulings. Each was read in the tree, not inferred from the report.

**F1 — the roster already declares the number, and no code reads it.** `ModelCfg` requires
`context: ContextVal` (`crates/vadis-core/src/config.rs:815-828`); it is the same `context:` key §4's provider
block shows and that §12.5's scalar convention parses (`k` = 1024, `m` = 1024²,
`crates/vadis-core/src/config.rs:114-131`). Nothing on the serving path consumes it: a grep of `crates/` for a
consumer finds only config tests and fixture strings, so `context` is today a **declaration with no reader at
all**. The list is the first consumer it has ever had, and that has a consequence worth stating: publishing it
cannot change a decision, a price, a refusal or a byte of any request — there is no behaviour attached to it
to disturb.

**F2 — the alias namespace is guaranteed to resolve to a declared model entry before the process serves
anything.** `VadisConfig` validates every alias at load: `aliases.<name>` naming a route with no such
provider/model is a **load error** (`crates/vadis-core/src/config.rs:2278-2286`, "unknown route '…': no such
provider/model in the roster"), and `has_route` matches on provider **name** and model **id**
(`crates/vadis-core/src/config.rs:1759-1763`). So `alias → RouteSpec → exactly one ModelCfg` is an invariant of a
loaded configuration, not a possibility the list has to handle: every entry of the list can carry a window, and
no entry can need a fallback value. A surface that had to invent a number for an unresolvable alias would be a
surface publishing a guess, and this one never has to.

**F3 — attribution was declared and never derived.** `IdentityRec.client` is a `&'static str` documented as
"UA-normalized client kind: `codex` / `hermes` / `claude-code` / `other`"
(`crates/vadis-core/src/trace.rs:83-93`), and the enum it was designed against is DESIGN §12.3's
`pub enum ClientKind { Codex, Hermes, ClaudeCode, Other }` (`design/DESIGN.md:410`). Every writer passes the
literal `"other"`: `crates/vadis-proxy/src/accounting.rs:604`, `auth.rs:157`, `body_limit.rs:78`,
`forward.rs:141`, `crates/vadis-store/src/trace_sink.rs:180`, plus the builder's own default
(`crates/vadis-core/src/trace.rs:561`). `thread_id` is `None` at all six of the same sites. The field was never
wrong; it was never *computed*.

## 2. D1 — a model entry declares a window, and the alias publishes the route's

### 2.1 The field

| Property | Value |
|---|---|
| Name | **`context_length`** |
| Type | JSON **integer** — a count of **tokens**, base 10, no suffix, no unit string. (`"1m"` is a *config* spelling, §12.5's scalar convention; the wire carries the integer it parses to, `1048576`) |
| Position | a top-level member of each `data[]` entry, **after `owned_by`** — the four keys ADR-055 froze (`id`, `object`, `created`, `owned_by`, §4.19) keep their positions, so the change is an *append* and every reader that depends on the frozen four sees them where they were |
| Cardinality | exactly once per entry, on **every** entry, always present, never `null` (F2 makes it always knowable) |
| Semantics | the context window of the model the entry's route resolves to: the total token budget the upstream accepts for one request. It is **not** an output cap, **not** a remaining budget, and **not** a promise about billing |

The name is chosen by the consumer, not by taste. `context_length` is the **first** key in the client's own
window-key table (`_CONTEXT_LENGTH_KEYS`, `/Users/yifeizhang/.hermes/installs/d910f03b4d18ee94/environments/
fbcb1656d0a349e3a7b3d0357f2b8f18/workspace/agent/model_metadata.py:416-420`), and it is the key whose
per-entry read populates the client's cache (`_parse_models_payload`, same file :1020-1028). A name outside that
table would be a correct number nobody reads — the failure mode this ruling exists to end. (Outside-repo path:
cited as **evidence** only. No test may read it — AGENTS constraint 6 — and none is asked to.)

### 2.2 Where the number comes from: declared once, in the roster, at the model entry

The value is the `context` of the **roster model entry the alias's route names** — `providers[i].models[j]`
where `i` is the provider the route names and `j` the entry whose `id` equals the route's model (F2). It is

- **declared per config model entry**, never per alias, and never derived: not from a sibling model, not from a
  family tag, not from the vendor's page at request time, not from a tokenizer estimate;
- **one source.** §4.0's single-source discipline — written for price, and the reason it was written applies
  verbatim to a window: an `aliases.<name>.context_length:` key would be a *second* place to write one number,
  and the two would eventually disagree, with the disagreeing copy being the one a client reads while the
  serving path uses the route's. The alias therefore gets no window of its own; it *inherits* the route's.

### 2.3 The alias case is the entire point

The reported failure is not "the list is thin". It is that a client configured with an **alias** read the alias's
name out of `data[]`, found no window, and fell back to a 256 k assumption for a route that serves 1 M. The
client's read is by entry, keyed on the string the client was configured with: `_parse_models_payload` maps
`entry["id"] → entry` (id = the alias name, the only name §4.19 publishes), and
`_resolve_endpoint_context_length` looks the requested model up in that map
(`agent/model_metadata.py:1088-1099`). So the entry that must carry the route's number is exactly the entry
§4.19 already publishes for the alias — no new name, no new key in the namespace, no second lookup path.

**Consequence, stated so it is not discovered later:** for `coding-fast → (provider, model)` whose model entry
declares `context: 1m`, the entry answers `"context_length": 1048576`, and the client's fallback warning cannot
fire for that alias. For an alias pointed at a 200 k model, it answers `204800`. The number follows the
*route*, so repointing an alias changes the number the client sees — which is the correct coupling, and the
reason it may not be declared per alias.

### 2.4 Order and byte stability: unchanged, and required unchanged

`data[]` remains **ascending byte order of `id`** — the `BTreeMap`'s own order, which §4.19 states as a
contract because the wire order is observable and ADR-055 decision 4 pins it. Two calls answered by one
revision remain **byte-identical**. The new key does not touch either property: its position is a constant of
the shape, and its value is a pure function of the roster — no clock, no RNG, no process state, no per-call
counter. An entry is now *five* keys, in the fixed order `id`, `object`, `created`, `owned_by`,
`context_length`.

### 2.5 The response boundary: constraint 1 is not engaged

This is a **response**, on a surface that reads the roster and writes nothing (§4.19's Reads/Writes rows: no
upstream, no trace line, no store row on the admitted arm). It touches no request byte because there is no
request byte in play: `GET /v1/models` is not on the forwarding path, its handler has no body extractor
(ADR-055 §6's landing plan), and the client's own `model` string continues to travel exactly as §2 requires —
resolved to the route's provider-native id, recorded as `decision.requested_model`. A later reader must not
have to wonder whether adding a field to a response could perturb the passthrough: it cannot, and the reason is
structural (the surface never touches a completion request), not a promise.

### 2.6 What the entry still does not say

No price (one source, §4.0 / constraint 5), no `max_tokens` or output cap, no `supports` cells, no route, no
provider, no `account:`/plan classification, no family tag, no rate limit, no per-call metadata. ADR-055 §4's
table stands except for the one row this ADR supersedes — the `context window` row, and only it (the header's
*Supersedes* line is the ruling; §2.3's measured need is why ADR-055's own condition, *"for no client need this
round has"*, no longer holds).

### 2.7 The one frozen assertion this collides with — named, and not taken here

`CONF-107` limb (a) asserts the entry's key set by **equality**: `assert_eq!(keys, [...])` over the four keys
(`tests/conformance/tests/conf_107_v1_models_lists_aliases.rs`, the sorted-key assertion in the *(a)* block),
and DESIGN §12.8's `CONF-107` row repeats the word *"exactly"* (`design/DESIGN.md:1092`). A fifth key makes
that limb fail **by construction**. AGENTS constraint 9 puts conformance assertions outside the mutable scope,
so this ADR does not edit the case and no implementing card may:

- **the amendment this ADR puts to the owner** is one word and one array element: limb (a)'s sorted key set
  becomes `["context_length", "created", "id", "object", "owned_by"]`, and its prose *"exactly
  `{id, object, created, owned_by}`"* becomes *"exactly `{id, object, created, owned_by, context_length}`"*;
  DESIGN §12.8's `CONF-107` row takes the same word. Nothing else in (a)–(g) moves: (b) is a *negative* limb and
  a number cannot reintroduce a route string; (d)'s empty-roster body is `{object, data}` and gains nothing;
  (e), (f), (g) do not read entries.
- **until that ruling lands**, the implementing change is expected to leave limb (a) red and to say so; a green
  suite may not be bought by editing the case, and a red limb may not be bought by omitting the field from
  entries whose window happens to be unknown (F2: it is never unknown).

The unit tests that live in product code — `crates/vadis-proxy/src/models_read.rs`'s
`every_entry_has_exactly_the_four_keys` and its byte-exact sibling — are **not** conformance and move with the
change, as ADR-055's landing plan already anticipated for that module.

## 3. D2 — the refusal names the resolvable set, and nothing about the roster's shape

### 3.1 One word, one body, for every client-formed resolution failure

Today `resolve_route` (`crates/vadis-proxy/src/forward.rs:2431-2467`) and its streaming twin
(`crates/vadis-proxy/src/stream_forward.rs:1944-1981`) answer **three** different sentences, and the sentence
itself is the leak: `unknown provider '<p>'` says *this deployment has no entry named p*, while
`unknown model '<m>' on provider '<p>'` says *it does, and that id is not on it*. That is a roster-structure
oracle — the same class of fact ADR-055 §3 keeps behind the guard — delivered in prose, to a client, as a
side effect of an error message.

The contract therefore becomes: **one word and one body for all three branches.**

- `error.type` is **`unknown_model`** whenever a client's `model` string does not resolve: a bare string that
  is no alias, a `provider/model` whose provider is not on the roster, and a `provider/model` whose model is not
  declared on that provider. The three are indistinguishable in the body.
- `unknown_provider` keeps its place in §8's vocabulary and its two producers, which are **internal
  invariants**, not client answers: the defensive "the selected/reloaded route is no longer on the roster" arms
  (`forward.rs:966-973`, `stream_forward.rs:541-548`), unreachable from a client string once a configuration has
  loaded (F2). §8's row records that it has **no client-request producer** — the same shape §8 already uses for
  `not_implemented` — so a reader is not left to discover it as a regression.

### 3.2 Exactly which strings may appear

The body is §8's unified shape (`crates/vadis-core/src/error.rs:67-90`): `error.type`, `error.message`,
`error.request_id`, `error.details`. `details` gains exactly three members:

| Member | Type | Content | Bounds |
|---|---|---|---|
| `requested` | string | the client's own `model` string, **verbatim**, JSON-escaped | never truncated (it is the client's bytes, already bounded by §4.13's `max_body_bytes`) |
| `resolvable` | array of strings | **alias names only**, taken from the `aliases:` keys of the revision that answered | **at most 8**, in ascending **byte** order of the name — the same order, and the same names, §4.19 publishes. `[]` when the roster is empty |
| `resolvable_total` | integer | the number of alias names the roster holds | always present; equal to `resolvable.length` when nothing was truncated |

`error.message` keeps its present sentence, now for every branch: `unknown model or alias '<requested>'`. It
carries no names beyond the requested string, because the message is a sentence a human reads and a log line
an operator greps — a message that grew with the roster would be a roster mirror with worse ergonomics than
the surface designed to carry it, and would change shape with every config edit.

The truncation bound is a **contract constant (8)**, not a computed value: it is chosen so a roster of the size
this deployment runs (5 aliases today) is named in full on one printed line, and so the body stays an error
rather than an inventory. The staleness of a truncated answer is bounded by the field beside it — a client that
sees `resolvable_total > resolvable.length` knows to ask `GET /v1/models`, which answers the whole set under
the same credential.

### 3.3 Forbidden strings — and why the argument is ADR-055 §3, not a new one

May **never** appear in any member of the body, including `message` and including inside `requested`'s echo:

- a **provider-native model id** (the roster's `models[].id`) or any string that contains one;
- a **provider name** (the roster's `providers[].name`), including `account:`-shaped names and plan families;
- any **`provider/model`** string the client did not itself send, and any `/`-joined route the deployment
  synthesised;
- the alias → route mapping, the number of routes, the number of providers, or the *reason* the string failed.

The guard argument is **ADR-055 §3's**, and this ADR does not re-derive it: the alias set is *"a fact about the
deployment, not about liveness"*, it is the operator's client-facing vocabulary, and it is therefore answered
**only behind `server.auth_token_env`'s guard**. A `404` on `/v1/chat/completions`, `/v1/responses` or
`/v1/messages` is behind the same guard, reaches the same credential class, and can therefore disclose the same
set — no more. Anything beyond the set (a provider name, a native id, a route) would be a **new** disclosure
that §4.19 does not make at all, and is refused here for exactly that reason.

One consequence is stated rather than hidden: when `server.auth_token_env` is unset the guard admits everyone,
so both surfaces are open to anyone who can reach the port. That is one boundary — the operator's choice about
inbound auth (§4.7) — not two, and this ADR does not create a second answer to it.

### 3.4 What the body must not distinguish, and why that is the ergonomic fix rather than a loss

The client learns *your string did not resolve, here is what does*. It does **not** learn whether the provider
half was wrong or the model half was wrong, and it does not learn whether a given provider exists. That is the
substance of the oracle ADR-055 §4's retrieval row already refuses ("a `404` becomes a query whose answer
distinguishes *not configured* from *not routed*"); closing the branch here keeps that refusal honest while
granting the ergonomic half the audit actually asked for.

### 3.5 The trace is unchanged by D2

`decision.requested_model` already carries the client's string verbatim and is already present-and-null
(`crates/vadis-core/src/trace.rs:114-120`); the failure record's shape, its `usage_missing` stance and its
`errors[].kind` mapping are untouched. The set named in the body is **not** written to the record — a roster
echo per failed request would put a config fact in a traffic log for no reader.

### 3.6 The reference body (illustrative; the names and the numbers are not vendor facts)

```json
{
  "error": {
    "type": "unknown_model",
    "message": "unknown model or alias 'gpt-5.4'",
    "request_id": "req-3f9c1a7e",
    "details": {
      "requested": "gpt-5.4",
      "resolvable": ["chat-fast", "coding-bulk", "coding-fast", "glm-plan", "kimi-fast"],
      "resolvable_total": 5
    }
  }
}
```

With a 12-alias roster the `resolvable` array holds the first 8 names in byte order and `resolvable_total` reads
`12`. With `aliases: {}` the array is `[]` and `resolvable_total` is `0` — never an omitted key, never a `500`.

## 4. D3 — `identity.client` is derived, and `thread_id` stops pretending

### 4.1 The vocabulary is closed, and it is the one DESIGN already declared

`codex`, `hermes`, `claude-code`, `other` — byte-identical to `ClientKind` (`design/DESIGN.md:410`) and to
`IdentityRec.client`'s own doc comment (`crates/vadis-core/src/trace.rs:87-89`). No fifth word may be added for
a client nobody has measured, and no word may acquire a second meaning. The field stays `&'static str`; a
vocabulary word is a constant, which is exactly what that type says.

### 4.2 The derivation: one pure function, at the boundary, once per request

- **Input**: the request's `User-Agent` header value, and nothing else. No clock, no config, no per-process
  state, no counters (constraint 2's determinism discipline, applied to an identity rather than to a payload).
- **Rule**: the **first** match wins over an ordered table of prefix rules; matching is ASCII-case-insensitive
  on the prefix; a value that matches no rule is `other`. Ordering is stated because "first match wins" is
  observable as soon as two rules could both match, and a table whose order is not a contract is a table that
  changes answers when someone sorts it.
- **Place**: the **boundary** — the same place that reads the headers for §4.7's guard — so the value exists
  before the pipeline and is therefore available to the pre-pipeline records too (a `401`, a `413`) as well as
  to an ordinary row.
- **Once**: derived once per request and carried to every writer. Today the literal `"other"` is repeated at
  six sites (`accounting.rs:604`, `auth.rs:157`, `body_limit.rs:78`, `forward.rs:141`, `trace_sink.rs:180`,
  `trace.rs:561`); after this ruling there is **one** call site and the writers take the value. A second
  derivation would let one request's record disagree with its own event log.

### 4.3 The bindings this round may carry, and the evidence each one needs

A binding is `prefix → word`, and it is admitted only with the header value **attested**: by the client's own
source on the operator's machine, or by a capture of the value on the wire, dated and cited here. This round
admits exactly one:

| Prefix (case-insensitive) | Word | Attestation |
|---|---|---|
| `HermesAgent/`, `Hermes-Agent/` | `hermes` | Hermes Agent's own attribution UA, set by the client itself: `plugins/model-providers/fireworks/__init__.py:34-38` and `plugins/model-providers/opencode-zen/__init__.py:17-21` (`f"HermesAgent/{get_version_info().base_version}"`), `plugins/model-providers/gmi/__init__.py:13`, `tools/xai_http.py:56-58` and `tools/mcp_oauth_provider.py:29-33` (`Hermes-Agent/<version>`) |

`codex` and `claude-code` are **eligible and not bound**. The trap is real and is written down so the next
round does not fall into it: hermes can emit `codex_cli_rs/0.0.0 (Hermes Agent)` for a non-official codex base
URL (`agent/codex_headers.py:42-49`) and `codex-cli` on its OAuth path (`hermes_cli/auth_codex.py:762`). A
future `codex` binding must therefore match `codex_cli_rs/` **and not** a value carrying `(Hermes Agent)`, and
it must cite a capture of the real codex CLI against a vadis endpoint. No capture of the real codex CLI's UA
exists in this repository; no binding is written on a guess.

### 4.4 The measured fact about *this* deployment: hermes sends the SDK's UA, so its rows land in `other`

Hermes Agent is the operator's client, and the mapping question is not academic: it decides whether U3's finding
is fixed by this round or only made fixable. It is **not** fixed by this round, and the reason is measured, not
assumed.

The operator's provider entry declares no attribution headers — `~/.hermes/config.yaml`'s `providers.vadis`
holds `base_url`, `api_key_env`, `key_env`, `model`, `api_mode` — and hermes sets `default_headers` on a
custom-provider client **only** when the entry declares `extra_headers`
(`hermes_cli/config_providers.py:487-513`). The transport then issues the call on the shared `openai` SDK client
(`client.responses.create(...)`, `agent/codex_runtime.py:1241`; `agent/auxiliary_client.py:1580`), whose
`User-Agent` is the SDK's own default. Measured 2026-10-26 against a local listener on a scratch port, with that
SDK and no attribution headers — `openai` 2.24.0 → `User-Agent: OpenAI/Python 2.24.0`:

```
listening http://127.0.0.1:62704/v1
openai version: 2.24.0
path: /v1/chat/completions
attribution-ish headers: {"User-Agent": "OpenAI/Python 2.24.0", "X-Stainless-Lang": "python"}
```

`OpenAI/Python <version>` names the **SDK**, not the harness: every Python program built on `openai-python`
sends it. Binding it to `hermes` would be precisely the invention the owner's card forbids — a fixed-looking
field that mislabels every future SDK-based client, discovered only by the next audit that trusts it. So it is
**not** bound, and `hermes`'s rows keep landing in `other` until the client says who it is.

**The remedy is one line on the client side, and it is the client's to write** (the same boundary ADR-055 §3
already drew for codex's reachability probe — *"the honest remedy is the client's"*): the operator's `vadis`
provider entry gains

```yaml
    extra_headers:
      User-Agent: HermesAgent/<version>
```

after which the binding in §4.3 fires, `identity.client` reads `hermes`, and the census can say who called.
vadis does not detect hermes by guessing at an SDK string, and this ADR does not claim the audit's U3 finding is
closed — it claims the mechanism is in place and names the one condition under which it reports anything other
than `other`.

### 4.5 A request with no `User-Agent` is `other`

Not `null`, not a new word, not an inferred name. The field's contract is a closed vocabulary and every row
carries one of its words, so a row from a header-less client stays comparable with the 4,432 rows already
written. From vadis's side, an absent header and an unmatched header are the same observation: *this process
cannot name the client.*

### 4.6 The raw header is never recorded — in any form — and two DESIGN promises are struck

The `User-Agent` value is read, reduced to one of four words, and **dropped**. It is not written to the trace,
to the event log, to an error body, to a `details` member, or to any tee. It is not truncated, not
prefix-preserved for debugging, and not hashed into a "fingerprint" field: a header whose plausible value set is
small — a few dozen SDK strings and version numbers — survives a hash by enumeration, so a hash would buy the
appearance of redaction while delivering a reversible identifier, and it would add a trace field whose only
consumer is a one-off investigation that a capture answers better and exactly.

`design/DESIGN.md` promises that field twice, and both promises are struck by this ADR rather than left to a
later reader to implement: the `IdentityRec` sketch's `pub client: ClientKind, pub client_ua_raw: Option<String>`
(`design/DESIGN.md:766-769`) and the field-table note *"(`client_ua_raw` is an appendix, for troubleshooting
when UA normalization fails)"* (`design/DESIGN.md:828`). No such field exists in the record
(`crates/vadis-core/src/trace.rs:83-93`), so nothing is deleted from the product — a documented-but-impossible
field is retired, which is the "documented but unreachable" defect in the other direction and the same defect
ADR-060 §2 retired for `errors[].kind`.

### 4.7 `thread_id` is permanently null in v0.1, and its arrival condition is named

The field stays in the record — `Option<String>`, present-and-null like `prefix.continuity` and `plan_switch`
(`crates/vadis-core/src/trace.rs:91`) — and its meaning is stated so no reader treats it as a gap or as a bug:
***`null` means this process observed no thread identity, always.***

The only client-sent string that could populate it is `previous_response_id`, and reading it is not this round's
to authorise:

- §2/§6 do not inspect it — *"the two fields are never inspected — they are forwarded as the client's own
  bytes"* (`docs/spec.md:2627`), and §6 records the whole group as a constant under **gap G-F**;
- ADR-004 item 3 is the ruling that would make it live, and it moves **two** things at once: `stateful_inbound`
  would start reporting a real value (`docs/spec.md:2489`) and `state.stateful_inbound: false` would stop being
  a constant;
- `session` (from §4's `key_sources`, preferring `prompt_cache_key`) is the only conversation-scoped identity a
  client actually sends, and it already has its own field. Deriving `thread_id` from the same input would be a
  duplicate column dressed as a second fact.

So: **permanently null here**, and the condition for its revival is named — a round that lands ADR-004 item 3
(reads `previous_response_id`, flips `stateful_inbound`) may populate it from the same read. Until then a null
is the honest value, and the audit's "`thread_id` null on 100 %" stops being a finding.

### 4.8 The admission rule, for every name, forever

A word enters §4.3's table only when the header value it maps from is **attested** (client source at a cited
path, or a dated capture of the wire) **and** the value distinguishes that client from every other client the
table already binds. "Plausible", "documented elsewhere", "looks like the vendor's name" and "the only client
this operator runs" are not attestation. A wrong binding is worse than `other`: it is a durable, confident,
wrong answer in the product's only product.

## 5. What this ADR does not touch

No price, no token estimate, no saving and no `verified`/`inferred` label (constraints 4, 5). No widening of
`/v1/models` beyond §2: no single-model retrieval, no pagination, no `?fields`, no provider list, no `supports`
cells, no rate limits, no `account:` classification, no per-call `X-Vadis-Request-Id` on the `200` arm
(ADR-055 §4's remaining rows stand). No change to `/metrics`, `/state/events`, `/health`, the decision pipeline,
the cost engine, the store schema, or any rule file. No gate, threshold, corpus or L1 envelope
(constraint 9 / ADR-012). No code, no test, no conformance case.

## 6. Conformance and measurement boundary

- **`tests/conformance/` is untouched by this ADR and by the card that implements it.** The single amendment
  the round needs inside the frozen measurement is named in §2.7 and belongs to the owner.
- **The evidence this ADR cites from outside the repository** — the client's `model_metadata.py`,
  `codex_headers.py`, `config_providers.py`, and the operator's `~/.hermes/config.yaml` — is evidence, held
  under the audit's own directory, and **no test may read any of it** (constraint 6: a suite that needs a file
  outside the tree is not a suite). The UA measurement in §4.4 is reproducible with a local listener and the
  SDK; it is not a fixture any gate depends on.
- **Nothing here moves a `verified` number.** The two window values quoted in §2.3 are the *roster's* declared
  numbers, and the report's 256 k is the client's own assumption; this ADR mints no measurement of its own.

## 7. Alternatives considered

| Alternative | Why rejected |
|---|---|
| add `context_length` to the **alias** entry in configuration (`aliases.<name>: {route, context_length}`) | A second source for a number the roster already declares — the §4.0 disease, one key over — and the copy a client reads would be the copy that drifts from the route the serving path uses. F2 makes the alias's number knowable for free; a config key would let it be *wrong*. |
| derive the window from the family tag, a sibling model, or the vendor's page | Nothing lets a gateway publish a vendor fact it did not read off the vendor (constraint 5); the roster entry is where the operator put the number they read. |
| publish `max_tokens` / output cap alongside the window | A different number with a different meaning, refused by the same discipline as price: no roster source exists for it, so publishing one would be inventing it. It is also the key the client checks *last* for a window, precisely because Anthropic payloads mislead on it (`agent/model_metadata.py:817-824`). |
| publish the window as a string (`"1m"`, `"1048576"`) or under `limit.context` | The client's table is a flat top-level read of integer-ish values; a nested shape would be a correct number nobody reads. The wire carries the parsed integer because a suffix is a *config* spelling (§12.5). |
| put the window in a top-level `limits:` map outside `data[]` | The client keys on entries; anything outside `data[]` is invisible to it and would be a second surface for one fact. |
| reject an unknown model with `400` instead of `404`, or with the set in `message` | `404` is §8's word for "does not resolve" and clients branch on it; the set belongs in `details` where a machine reads it, while `message` stays a sentence a human reads and a log an operator greps. |
| keep `unknown_provider` distinct and just add the set | The distinguishing word is the oracle: it answers "does this provider exist" for a request that failed. Adding the set beside a leak leaves the leak. |
| name the set without a truncation bound | A 200-alias roster would put a 2 KB inventory in every typo's error body; the bound plus `resolvable_total` keeps the body an error and tells the client when to ask the surface that answers the whole set. |
| record the raw `User-Agent` "for troubleshooting" (DESIGN's `client_ua_raw`) | A verbatim UA is a fingerprinting identifier in a log the operator shares, and the field never existed in the record. Troubleshooting needs one capture, not a permanent column. |
| record a **hash** of the UA instead | A hash over a low-entropy header is reversible by a dictionary of known UAs: it looks like redaction and behaves like an identifier. |
| bind `OpenAI/Python …` → `hermes` because hermes is the only client this operator runs | §4.4: the string names the SDK. The mapping would be confidently wrong for every future SDK-based client, and a wrong word in the only product outlives the round that wrote it. |
| derive `thread_id` from `prompt_cache_key` | That is `session`'s input, and it is already `session`. One input, two names, is a duplicate column, not a second fact. |
| derive `thread_id` from `previous_response_id` now | That read is ADR-004 item 3's arrival, and it flips `state.stateful_inbound` — a contract change that must land as one piece, with its conformance, not as a side effect of an identity field. |
| remove `thread_id` from the record | It would retype a published record for every reader that already tolerates a null, to say something a declared null says for free. |

## 8. Honest boundaries

- **D3 does not close the audit's attribution finding; it makes the finding fixable and names the condition.**
  After the implementing change, hermes's own rows still read `other` unless the operator adds the one
  `extra_headers` line (§4.4). Anyone reading the next census must read that sentence first.
- **D1 and the client's caching.** The client caches a `/models` read per (base URL, key fingerprint) for a
  short TTL and **stops probing** on `401`/`403` (`agent/model_metadata.py:72`, `:1059-1067`; the lookup is
  `:1088-1099`), so the probe must carry
  the token — a client fact this contract cannot assert, the same caveat ADR-055 §3 records for codex's
  reachability probe. A client that probes unauthenticated learns nothing and keeps its 256 k assumption; the
  honest remedy is the token, never an exemption of the route.
- **A window is not a guarantee.** The number is what the deployment's configuration declares about the route.
  If the operator typed a stale figure, the client will size its context from a stale figure — the roster is
  the only source and its accuracy is a transcription duty (constraint 5), not something this surface can check.
- **D2 cannot make a wrong name resolvable.** It shortens the loop from "the client guesses" to "the client
  reads the set and asks for one of them"; it does not make vadis accept a string the roster does not declare.
- **The truncation bound is a judgement, not a measurement.** 8 is chosen for a deployment with 5 aliases and
  a printed line; a deployment with 50 aliases is served correctly (the set is always one guarded `GET` away)
  but not fully in the error body.
- **Nothing is claimed about clients other than the two already named in this repository's evidence.** No UA
  table is complete, and none is claimed to be.

## 9. Register updates

- `docs/spec.md`: **§4.19** (the body's fifth key, its table row, the source rule, the export-boundary row it
  leaves, the assertions paragraph), **§8** (the `404` row, the forbidden strings and the one-word rule),
  **§3** (the bare-id refusal's two sentences point at §8's body), **§6** (the `identity` group: `client`
  derived, `thread_id` null, no raw header).
- `design/DESIGN.md`: **§12.3**'s `IdentityRec` sketch and **§12.8**'s field-table note lose `client_ua_raw`;
  §12.8's `CONF-107` row carries the amendment proposed in §2.7 **when the owner rules on it**, not before.
- `design/decisions/ADR-055-the-model-list-surface.md`: **not edited** (ADRs are append-only). ADR-055 §4's
  `context window` row is superseded by §2 here, and this bullet is the forward reference a reader of that row
  needs.
- `book/` and `README.md`: the landing change owns the user-facing sentence (an alias may declare nothing and a
  client may read its window; a `404` now names the set). Constraint 8: the same change updates the book.
- `tests/conformance/`: **nothing** (§2.7, constraint 9).
