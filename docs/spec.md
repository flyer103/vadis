# Spec (WHAT) — router v0.1

Convention: this document is the single source of truth for **external behavior and the config
contract**. Implementation details are in `design/DESIGN.md`; the why is in `design/decisions/`.

## 1. Goals and non-goals

**What it is**: a local-first multi-protocol LLM gateway. Clients (codex / hermes / claude code) point
their base_url here; router sends each request to the configured (provider, model), saving tokens along
the way **without breaking the upstream prefix cache**, and records every decision and every cent as a
replayable trace.

**v0.1 non-goals** (explicit exclusions — do not add them on the side):

| Not doing | Reason / later path |
|---|---|
| Server-side session state (`store:true`, `previous_response_id`) | Measured: clients do not use it (the capture in ADR-004); stickiness keeps fidelity when it is absent. **Not implemented in v0.1**: the two fields are never inspected — they are forwarded as the client's own bytes (§2) — so the trace's `state.stateful_inbound` is `false` for every request (gap G-F, §6) |
| Automatic model selection / effect optimization | Explicit specification is primary; `auto` is left as a plugin slot (ADR-004) |
| Semantic response cache, context summarization | Large conflict surface with prefix caching; a measured ledger is needed first (P4) |
| Multi-user, multi-tenant, multi-node deployment | Single-operator local process; local state is one SQLite/WAL file behind `trait Store` (§4.5, ADR-009) — the hosted form is a second implementation of the same trait, not a v0.1 goal |
| Historical algorithms such as bandit / MF / BT | Kept as experiment assets; later returned as tier-A plugins |
| A retrieval channel for `tee` original text (retrieve endpoint) | A rule may declare `tee`, but the storage location and retrieval channel are **not implemented in this version** (§4) |

## 2. Protocol contract

The three inbound protocols are semantically equivalent (the same router decision chain) and mirror
upstream semantics per protocol:

| Inbound | Endpoint | Outbound native condition |
|---|---|---|
| OpenAI chat completions | `POST /v1/chat/completions` | provider `wire_api: chat` |
| OpenAI responses | `POST /v1/responses` | provider `wire_api: responses` |
| Anthropic messages | `POST /v1/messages` | provider `wire_api: anthropic` |

**Outbound selection rule** (3×3): inbound protocol equal to the provider's `wire_api` → **native
passthrough** (byte-faithful); different → **deterministic translation** (the same content always yields
the same upstream bytes, keeping the prefix cache stable). Every provider must declare its supported
capabilities in config; every translation cell must explicitly mark its lossy points.

**Only the native diagonal is served in v0.1, and the failover walk never crosses it.** Two different questions
share this rule and must not be merged: for the route the **client named**, a cell that needs translation is
answered (`501 not_implemented`, §8 — the client asked for that cell and is told what is missing); for a
**candidate the client did not name** — the plan family's `overflow` route or an entry of the `fallback` list
(§4.2) — an entry whose `wire_api` is not the inbound protocol is **skipped**, exactly as an entry this process
holds no key for is skipped, and the walk continues. A candidate can therefore only ever be served on a wire
equal to the inbound protocol, which is why a served request's `protocol_out` equals `protocol_in` (§6); when no
candidate at all may serve, the request is refused in the one shape §8 freezes.

**The outbound `model` is the provider-native model id.** Routing is resolved **before** anything leaves
the process: the client's `model` string is a **route name** (`provider/model` or an alias, §3), never a
name the upstream is expected to understand. The outbound request therefore carries the roster entry's
own model id (§4) — the string that provider's API actually accepts — and the client's literal string is
kept in the trace as `decision.requested_model` (§6). Two client-side forms that resolve to the same
route produce byte-identical upstream requests.

Consequently the **byte boundary permits exactly two mutations**, both byte-level, both scoped, both
auditable:

| # | Mutation | Scope |
|---|---|---|
| (a) | **deleting router-owned top-level fields** (`router_meta` echo, routing hints) | whole members, with their separators; invariant: the output is valid JSON and every retained member is byte-for-byte its input span |
| (b) | **replacing the value of the top-level `model` member** with the resolved provider-native model id | the member's **value span only** — its key, its position, the separators and whitespace around it, and every other byte are untouched |

Everything else is passed through verbatim: message content, order, whitespace, tool schemas, unknown
fields (§8) and even malformed value content the upstream is entitled to adjudicate. There is **no
parse → reserialize round trip anywhere on the serving path** (ADR-007): (b) is a byte-level span
replacement, not a re-encoding, precisely because a round trip would rewrite every byte of the body,
invalidate the upstream prompt cache for the whole conversation and destroy the meaning of
`body_hash` / `prefix_blocks[]`.

List of lossy points (each must be handled one by one when translating — not "best effort"):

| Semantics | chat | responses | anthropic | Handling |
|---|---|---|---|---|
| System instruction | `messages[0].role=system` | `input[0].role=developer` (measured: codex does not fill the top-level `instructions`) | top-level `system` | Map; keep the position stable (at the very front of the prefix) |
| Tool calls | `tool_calls` + `role=tool` | `function_call` / `function_call_output` item | `tool_use` / `tool_result` block | Map both ways, preserving id and order |
| Reasoning content | `reasoning` field | `reasoning` item (`include:["reasoning.encrypted_content"]`) | `thinking` block | If it is not passed through, it is dropped and a lossy marker is recorded; **re-encoding is forbidden** |
| Cache breakpoints | none | `prompt_cache_key` | `cache_control` breakpoints | When translating to an anthropic upstream, inject breakpoints by a stable rule (same content → same position) |
| usage | `usage.prompt/completion_tokens` | `usage.input_tokens_details.cached_tokens` etc. | `usage.input_tokens/cache_read_input_tokens` | Normalize into the internal `Usage` (§6) |

### 2.1 Transform mode — the byte promise when a content transform is enabled

Everything above describes the **passthrough path**, and a request is on it **unless it asks for a
transform**: the operator's config decides which transforms exist, the client's request decides
whether they apply (ADR-019). No config key, plugin, alias or default can enable a content transform,
so the §2 promise above is unconditional for every request that did not ask, and stays testable
without a configuration in the picture.

| | Passthrough (default) | `X-Router-Transform: transform` |
|---|---|---|
| outbound body | the client's bytes + the two mutations of §2 | the client's bytes + **a declared, ordered list of value-span edits** + the same two mutations |
| the promise | byte equality, modulo (a) and (b) | *span* equality: every byte outside the declared value spans is the client's, byte for byte |
| audit surface | the two-mutation table | the same table **plus** one trace ledger entry per step (rule, path, bytes and tokens in/out, label) |

- **The opt-in is a request header** — `X-Router-Transform: passthrough | transform` (absent or
  `passthrough` ⇒ the byte path). Any other value is `400 invalid_request`, decided before the body is
  read: a typo must not silently disable a saving the client asked for, and it must never silently
  enable one. A header is not a body byte, so asking for a transform cannot perturb the prefix.
- **Span-faithful, never a re-encode.** An edit replaces the *value span* of one addressed node (a
  tool/environment payload), located by the same single-pass scan as mutations (a)/(b) — a
  parse → reserialize round trip is forbidden on this path exactly as it is on that one (§2, ADR-007).
- **Payloads only.** Edits may target tool/environment payloads. They may **not** touch a user or
  assistant message, the system instruction, the tool schemas or any structural member; a transform
  that changes *what the model is asked to do* is not this mode and needs its own decision (ADR-019).
- **Three invariants bind every transform** (ADR-019): content determinism (same inbound bytes + same
  configured set → same outbound bytes, no clock/turn/RNG); per-set prefix monotonicity (with the set
  unchanged, an appended conversation extends the previous turn's outbound bytes instead of rewriting
  them); and closed-mode byte equality (a request that did not ask is byte-equal modulo (a)/(b) **even
  when matching rules are configured**). A client that changes its request mid-session is obeyed, and
  the change is visible: the ledger moves and `prefix.continuity` (§6) drops for that turn.
- **Fail-safe.** A rule that cannot load, compile, pass its inline tests or apply edits nothing: the
  payload passes through verbatim and the record carries `errors[].kind = "transform_error"` (§8).
- **A saving measured here is `inferred` until a pair exists** (§7): the served request is one world,
  and the untouched one is not in the trace.

## 3. Selection semantics

The `model` field accepts three forms:

1. `provider/model` — hits one entry in the roster directly;
2. **alias** — a name defined in config `aliases:` (example: `coding-fast → deepseek/deepseek-v4-pro`);
3. `auto` — v0.1 returns `400` + an explicit error body (saying a plugin takes it over); the `Selector`
   slot is reserved structurally.

Whichever form is used, selection resolves to exactly one **route** = (provider, model id) taken from the
roster (§4). The route's id — the roster entry's model id, not the client's string — is what the outbound
request carries (§2). The form the client used is recorded in the trace as `selection_source`
(`explicit` / `alias`), and the client's literal string as `decision.requested_model` (§6). A bare
provider-native model id is **not** an accepted inbound form: it is not a `provider/model` route and not
an alias, so it resolves to nothing and is answered `404 unknown_model`.

With an explicit specification, router still runs the **Guard stage** (quota / cost cap / capability /
`max_tokens`); on a guard hit it acts per the configured policy (reject and explain why, or downgrade to
the `fallback` chain). A guard `Downgrade` picks another route, so it rewrites the outbound `model` to
that route's id too (§2).

## 4. Config schema (the contract of `config.example.yaml`)

```yaml
server:   { addr: "127.0.0.1:8790", upstream_attempt_timeout: 60s, request_timeout: 10m,
            max_body_bytes: 2097152,         # §4.13: the largest inbound request body the router will read,
                                             #   in bytes (default 2 MiB); a larger one is refused at the
                                             #   boundary with §8's `413 request_too_large`
            auth_token_env: ROUTER_TOKEN }   # optional (§4.7): name of the env var whose value every
                                             # inbound request must present; absent ⇒ no inbound auth
session:  { key_sources: ["prompt_cache_key", "header:session-id", "header:thread-id"], ttl: 12h }
cache:    { sticky: true, breakeven: { enabled: true, min_remaining_turns: 3, safety_factor: 1.2 } }
trace:    { dir: "./state/traces", rollover: hourly }

providers:                             # §4.14: the roster, written inline here. Exactly one of this key
                                       #   and `providers_file` (below) is written — both, or neither,
                                       #   is a load refusal naming both keys
  - name: deepseek
    region: intl                       # cn | intl (absent ⇒ intl, §4.8): the regional deployment this entry's
                                       #   endpoint and key belong to. Display/audit only — it routes nothing
                                       #   and it does not choose a currency
    currency: USD                      # USD | CNY (absent ⇒ USD, §4.8): the unit of every `price` below.
                                       #   Never converted: a CNY page is transcribed as CNY, no rate is ever
                                       #   applied, and two currencies are never added (AGENTS constraint 5)
    urls:                              # spec §4.9: the **complete** URL for every protocol this entry
                                       #   declares in `supports` below. The router sends it verbatim — it
                                       #   composes no path (ADR-020). A declared wire with no URL here, a
                                       #   key outside `supports`, or a non-http(s) value = a load error
      chat:      https://api.deepseek.com/chat/completions
      responses: https://api.deepseek.com/responses
    api_key_env: DEEPSEEK_API_KEY      # secrets are read from env only
    wire_api: chat                     # chat | responses | anthropic
    supports: [chat, responses]        # inbound protocols this provider can be translated to
    account: api                       # coding_plan | api (absent ⇒ api): the metered account (§4.6)
    models:
      - id: <unique model id within the provider>   # the provider-native id; this is what goes upstream (§2)
        family: <family tag>           # optional (§4.8); absent ⇒ the id itself. This is the tag
                                       #   plan_policy.family (§4.6) matches, so two routes whose native ids
                                       #   differ can still be one family
        context: <context limit>
        price:                         # five-tier price, 1K token **in this entry's currency**: this file fixes only the **schema and convention**, it copies no values.
                                       #   This is the **flat shape**: one price for every input length. A vendor
                                       #   that publishes its prices **banded by input length** writes `tiers:`
                                       #   instead — §4.10 (rules, second example, refusals). A price block is
                                       #   exactly one of the two shapes, never both
          input_miss: <base price: cache miss>
          input_hit: <cache-hit price>
          cache_write: <cache-write price; 0 = upstream does not charge separately>
          output: <output price>
          peak: { multiplier: 2.0, windows: [{ days: [mon,tue,wed,thu,fri], start: "01:00", end: "04:00", tz: UTC }] }
        source: "<official pricing page URL> @<fetch date>"   # required, traceable (see "Price convention" below)
  - name: deepseek-plan              # a second entry for the same vendor, the **subscription** account;
                                     # names follow the §4.8 convention `<vendor>[-<region>][-<account>]`,
                                     # with `<vendor>` matching the product name its api_key_env uses
    region: intl                       # §4.8
    currency: USD                      # §4.8
    urls:                              # one complete URL per protocol declared in `supports` below
      chat:      <the plan's OpenAI-form endpoint, in full>
      responses: <the plan's Responses endpoint, in full>
      anthropic: <the plan's Anthropic endpoint, in full>
    api_key_env: CODING_PLAN_KEY
    wire_api: anthropic                # chat | responses | anthropic
    supports: [chat, responses, anthropic]
    account: coding_plan               # coding_plan | api (absent ⇒ api): the subscription account (§4.6)
    models:
      - id: <the model id this account serves>
        family: <the tag plan_policy.family matches; equal to the id above when the two routes' ids agree (§4.8)>
        context: <context limit>
        price: { input_miss: <…>, input_hit: <…>, cache_write: <…>, output: <…>, peak: { multiplier: 2.0, windows: [<…>] } }
        #       the flat shape (§4.10): one price for every input length
        source: "<official pricing page URL> @<fetch date>"
    quota:                             # subscription plans (coding plan etc.), optional
      - { models: ["<this provider's model id>"], window: monthly, tokens: <plan allowance>,
          reset_day: 1, over_quota: block }   # quota may only reference **its own provider's** models
                                              # under §4.6 the local counter is a warning, never the authority

  - name: kimi-cn-plan               # the same vendor, the other region, the subscription account (§4.8)
    region: cn                         # this entry's endpoint and price page are the CN ones
    currency: CNY                      # its price table is published in CNY and stays in CNY
    urls:
      chat: https://api.kimi.com/coding/v1/chat/completions
    api_key_env: KIMI_CN_CODING_API_KEY
    wire_api: chat
    supports: [chat]
    account: coding_plan
    models:
      - id: k3                         # the coding endpoint's own native id …
        family: kimi-k3                # … and the tag that pairs it with the metered route below (§4.8)
        context: 1m
        price: { input_miss: <…>, input_hit: <…>, cache_write: <…>, output: <…>, peak: { multiplier: 1.0, windows: [] } }
        #       the flat shape (§4.10): one price for every input length
        source: "<the CN region's official pricing page> @<fetch date>"
    # no `quota:` — a plan whose allowance the vendor does not publish as tokens is still a plan (§4.6);
    # a credit- or window-shaped allowance is not written as a token count (GAP-Q17, DESIGN §12.9)

  - name: kimi-cn                     # the CN metered account of the same vendor — the family's `overflow`
    region: cn
    currency: CNY
    urls:
      chat: https://api.moonshot.cn/v1/chat/completions
    api_key_env: KIMI_CN_API_KEY
    wire_api: chat
    supports: [chat]
    models:
      - id: kimi-k3
        family: kimi-k3                # the same tag: two different native ids, one family (§4.8)
        context: 1m
        price: { input_miss: <…>, input_hit: <…>, cache_write: <…>, output: <…>, peak: { multiplier: 1.0, windows: [] } }
        #       the flat shape (§4.10): one price for every input length
        source: "<the CN region's official pricing page> @<fetch date>"

# `providers_file: ./providers.yaml`     # §4.14: … **or** the roster is its own file, named here — the
                                         #   same `providers:` block, byte for byte. The value is resolved
                                         #   by §4.1's rule (absolute wins, else against the config file's
                                         #   own directory; `~` is not expanded), there is no default
                                         #   location and no discovery (§4.12 gains no candidate). A file
                                         #   whose top-level key is not `providers:` is refused

aliases:  { coding-fast: deepseek/deepseek-v4-pro }

plugins:
  - id: cache-guard
    kind: builtin/cache_guard          # tier-A: compiled into the product
    config: { strict_prefix: true }
  - id: tool-output-rules
    kind: builtin/transform_rules      # rules as data (TOML + inline tests)
    config: { rules_file: ./rules/tool_output.toml, on_failure: passthrough }
  - id: judge-experiment
    kind: process                      # tier-B: out-of-process plugin
    url: unix:///tmp/router-plugins/judge.sock
    inject: [cache_ledger, session_table]   # dependency declaration (coeffect): stays at load-waiting while unsatisfied
    isolate: false                     # separate realm: can coexist with another version for shadow comparison
    intercept: { sample: 0.05, shadow: true }
    disabled: true

fallback: [deepseek/deepseek-v4-pro, kimi/kimi-k3]   # ordered route list, global granularity (§8)

plan_policy:                          # optional; §4.6 — the subscription first, the metered account as the spill
  family: <the family tag both routes' model entries carry>   # §4.8; equal to the model id when the two
                                                              # routes' native ids agree
  primary: <provider>/<model>         # required; its provider must be `account: coding_plan`, and the model
                                      # entry it resolves to must carry `family`
  overflow: <provider>/<model>        # required; its provider must be `account: api`, and likewise
  on_primary_exhausted: spill          # spill | block   (default spill)
  recover: probe                       # probe | none    (default probe)
  cooldown: 15m                        # default 15m
  overflow_monthly_cap_usd: <float>    # optional; absent = no cap
```

**The account belongs to the provider entry; the plan policy is one top-level section.** `account` says whether
a provider entry is a subscription (`coding_plan`) or the pay-per-token API (`api`, the default when the key is
absent): the key, the endpoint and the allowance belong to the account and not to a model (ADR-011 item 4), so
the flag sits on the provider. `region` and `currency` are provider-entry properties for the same reason (the
deployment and the unit belong to the endpoint and the account), and `models[].family` is a **model**-entry tag
— §4.8 for all three. `plan_policy` names one **family** — the tag two routes (one per account) both carry —
and is optional; its semantics, its defaults and its hard rules are §4.6.

**Roster ids are provider-native.** `models[].id` is the exact string that provider's API expects, and it
is what the outbound request carries (§2). A client never needs to know it: it writes a `provider/model`
route or an alias (§3), and router substitutes the resolved id. Pasting a bare native id into a client is
therefore **not** a shortcut — the inbound grammar has no bare-model form, so it resolves to nothing and
the client gets `404 unknown_model`.

**The roster is one block, and exactly one of two keys names it.** `providers:` (inline, the block above)
and `providers_file:` (§4.14) are **mutually exclusive**: a root file that writes both is refused, and one
that writes neither is refused too — an empty roster is a decision (`providers: []`), never a default.
This is not a merge and not a precedence rule: there is still exactly **one** place a given key can be
written in any one effective config, and the *refusal* is what makes that boundary checkable. The block
itself is identical in either place, so the roster file is not a second schema and not a second parser.

*What is shipped today, and what is contract:* the shipped example is the **pair** — `config.example.yaml`
names the roster at its `providers_file:` line, and `providers.example.yaml` is that roster (the `providers:`
block that used to be inline, moved byte for byte, comments and citations included) — so the example's own
`# Usage:` line (`cp config.example.yaml config.yaml`, with `providers.example.yaml` kept beside it) is the
complete instruction. The wizard's embedded roster template and the section table's target-file column are
served with it: `router setup` writes **both** files from the templates embedded in its binary, and
`--check` validates the pair (DESIGN §12.14). The inline shape is not deprecated **on the reading side** — a
root that carries the roster itself loads exactly as it always did, and §4.14 is the contract for both
shapes. On the **writing** side there is one shape: since **ADR-038** a writing run never leaves the roster
inline. A root whose parsed shape is inline-with-`providers` is normalized by that run — the block's bytes
become `providers.example.yaml`'s, the embedded template's own `providers_file:` line takes the header
line's place, and a file already at that name is kept at `<roster>.bak` first (§4.11's shape step) — so the
wizard's output is the pair whatever the file's history, and the hand-written inline root stays a shape the
reader serves, not one this command produces.

### 4.0 Price convention (preventing two copies from drifting)

**This file copies no price figure** — the single source of truth for price figures is each model entry
**in the roster** (`providers.example.yaml`, the file the shipped root names at `providers_file:`; §4.14),
and each of them must carry `source` (official pricing page URL + fetch date).
This file defines only the schema and the convention:

- The four price tiers are **base prices**; the periods matched by `peak.windows` are **multiplied** by
  `peak.multiplier` (peak/off-peak is expressed by multiplication, not by writing two sets of prices).
- **A price table may be banded by input length.** Some vendors publish one rate while a request's input stays
  within a threshold and another rate above it; the flat four-tier block above is that same table with **one**
  band (its phrasing: one price for every input length). The banded form — its shape, which band prices a
  request, the load-time refusals and the citation rule — is **§4.10**. Nothing else in this section changes:
  the unit, the "no rates, no estimated figures" rule, the peak multiplication and the one-`cache_write`-tier
  rule apply to a band exactly as they apply to a flat table.
- **The unit is the entry's `currency`** (§4.8), and it is uniformly **the currency unit / 1K token**.
  Converting the official page's "per 1M" to 1K means **dividing by 1000**, in whatever currency the page
  publishes: a CNY table is transcribed as CNY, and **no exchange rate is ever applied** (an "equivalent"
  USD figure computed from a rate is a number nobody published, and it is also the reason this file's
  numbers can be recomputed byte for byte).
- **`currency` is never inferred from `region`**: a CN-region entry whose page prices in USD, or the reverse,
  is written as it is.
- `source` is the **entry's own region's** official page (URL + fetch date): a `cn` entry cites the CN page
  and an `intl` entry the international one. One region's price table is never the evidence for another
  region's figure.
- Where the official page prices **cache writes per TTL tier** (e.g. a 5-minute and a 1-hour tier), `cache_write`
  records the tier this router's requests fall into — the page's own stated default when a request carries no
  explicit TTL — and the model entry's comment names the other tier with its price. The schema has one
  `cache_write` tier; recording a tier the router's requests never enter would overprice them, and recording
  neither would underprice them.
- `cache_write: 0` means the upstream does not charge separately for cache writes (e.g. DeepSeek only
  distinguishes hit / miss).
- Reference instance (@2026-09-19 official page): DeepSeek's peak periods are **UTC 01:00–04:00 and
  06:00–10:00 (Monday to Friday, excluding Chinese public holidays)**, off-peak is half the peak price
  ⇒ the base price takes the **off-peak price**, `peak.multiplier: 2.0`. Holidays are not modeled
  (GAP-Q6; known deviation: holidays are billed at `peak`, which is on the high side).
- A plan's `quota` **may only reference models of its own provider**: a cross-provider reference would
  have unclear semantics when that provider is unavailable.
- `quota` states a **token** allowance on a monthly window, and nothing else. A plan whose vendor publishes
  a different shape — a credit allowance, a per-5-hour or weekly window, a usage-based limit — carries **no**
  `quota` block: §4.6's rules are reachable without one (§4.6: "a plan whose token allowance is not published
  is still a plan"), and writing a token count the vendor never published is fabrication (GAP-Q17, DESIGN §12.9).

Config-change semantics (aligned with Cordis's keyed diff, see ADR-002): a `config` change → handed to
the plugin to diff by itself and reload (the process is not rebuilt); `disabled: true` → unload that
fiber and fully roll back its effects; an `id`/`kind` change → rebuild that entry.
**This paragraph describes the contract, not today's binary** — the loader that would act on it is P9,
whose implementation is R41-2 (ADR-036; §4.3's honesty note above).

### 4.1 `trace` (the on-disk parameters of the observation medium)

| Key | Type / value | Semantics |
|---|---|---|
| `dir` | path | Directory where trace JSONL is written. A relative path is resolved against **the directory containing this config file** (not the CWD) |
| `rollover` | `hourly` | Rollover granularity; `hourly` → file name `YYYY-MM-DDTHH.jsonl` (UTC, hour start). v0.1 defines only this value |

- The path goes into config (so that state can live elsewhere); **retention does not go into config**:
  v0.1 does no automatic cleanup, files are append-only and archived by hand by the operator; adding
  `retention` in the future is a new key (it does not break existing configs).
- A write failure **does not block the request** (§8); trace contents are in §6.

### 4.2 `fallback` (failover chain, §8)

| Key | Type | Semantics |
|---|---|---|
| `fallback` | ordered `provider/model` list | **Global granularity** (v0.1 has no per-model / per-alias chain); an empty list = no failover |

On upstream 5xx / 429 / quota exhaustion, switch in order to "the next route **not yet attempted**";
switching loses the prefix cache, so the trace must record `failover_from` and the resulting re-prefill
cost (§6 "result"). Chain exhausted → `502 upstream_error` (upstream attempt timeout →
`504 upstream_timeout`). List entries must be routes that exist in the roster (aliases do not take part
in fallback). For a request covered by `plan_policy` (§4.6), the family's `overflow` route is tried **before**
this list's own entries: the family's designated spill target does not have to be repeated here.

**A candidate can serve only on its own wire.** Eligibility is one rule for both forwarding paths, and it
includes the protocol: a candidate is attempted only if its provider entry exists in the roster, this process
holds a key and a transport for it, and its `wire_api` equals the **inbound** protocol (validation already
requires `wire_api ∈ supports`, §4, so the wire condition is the whole of it). A candidate that fails the wire
condition is skipped in the same class as a keyless one — not attempted, no `failover_from`, no switch cost, no
trace event — and the chain continues; a skip is **not** a refusal of the request, and it is not the `501` of a
route the client named (§2). **The walk names a destination only if that candidate could serve this request**
(the narration predicate *is* the eligibility predicate, stated once for both paths): `failover_from` and the
`failover.triggered` event are written only when the request moves onto a candidate the walk will actually
attempt, so an ineligible tail is neither narrated as a displacement nor priced at its own table (§6). When no
candidate at all may serve — and **nothing was attempted** — the client gets the `502 upstream_error` refusal §8
freezes (`details.stage: "no_available_route"`), and the trace records the terminal failure. When a candidate
**was** attempted and the walk then had nothing left to try, the refusal is §8's attempt-exhausted shape
instead: the walk's refusal has **two conditions, one shape each**, and both paths produce each shape
identically (§8).

**The classification's evidence is the upstream's own answer, on both forwarding paths.** An upstream error is
classified from the answer's own material and from nothing else: its status, its headers (`Retry-After`) and
the answer's **body bytes** (ADR-011 item 10 — parsing the body is never a precondition, but a body that
arrived is evidence). The two forwarding paths feed the classifier those same three inputs, so one upstream
answer classifies one way on the buffered path and the same way on the streaming path. On a streaming attempt
whose head is an error status, the answer's body is therefore **read before the classifier runs** — the relay
has sent no byte to the client at that point (DESIGN §12.10.3 R6 column 1), so the request is still an ordinary
one and the §8 refusal answers it. The read is bounded by the streaming path's existing idle bound
(`server.upstream_attempt_timeout`, DESIGN §12.10.3 R4 — the same bound the relay's own reads use, no new bound
and no new knob) and it is **internal**: an error head is relayed to no one, so this changes no byte a client
sees. A read that ends short (the idle bound trips, or the connection closes) leaves the classifier with the
bytes that arrived — the status-and-headers verdict when none did. The read is an **input** to the
classification, never a fourth fact about it: a failure whose body could not be read is not a different outcome
class, and §4.6 rule 3's account-moving verdict is reachable on a streaming request for the same reason it is
reachable on a buffered one.

### 4.3 `inject` (plugin dependency declaration)

`inject: [<service>…]` declares the service slots this plugin depends on (Cordis's coeffect declaration,
ADR-002/DESIGN §4). While unsatisfied, that fiber stays at **load-waiting** (it does not error, and it
does not affect other plugins); it goes on loading once the service is ready. Service names are
product-defined typed slot names (e.g. `cache_ledger`, `session_table`), not arbitrary strings.

**Two of the four keys are acted on, and in different places; two are still inert.** `disabled` is
honoured at start-up (§4.4: the rule set is off, `/health` reports it). `inject` is honoured by the
launcher's assembly: the `plugins:` list **is** the assembly point, so a declared slot that nothing provides
leaves that plugin in a **named loading wait** rather than failing the boot, and every plugin the launcher
can mount has its declarations satisfied before the request path is built. `isolate` and `intercept` are
accepted by the parser and checked at load (the plugin validation loop, `router-core/src/config.rs:1744-1804`:
the `intercept.sample` range and the `inject` slot names) and **nothing acts on them**: realms and
interception are the rail features ADR-013 describes, and the surfaces that would use them (`Selector`,
`Guard`) are still blocked in the leak register (§13.3). Their meaning,
the lifecycle they belong to, and the boundary that decides which capabilities may ever be mounted this
way are frozen by **ADR-036** (`design/decisions/ADR-036-minimal-core-and-plugin-surface.md`; the map is
DESIGN §13.6): `inject` waits until its slots exist, `isolate` gives one key two realms of bindings, and
`intercept` changes how a binding is used and never the binding itself.

**`disabled` is honoured today, at start-up.** A `disabled: true` entry is not loaded — for
`builtin/transform_rules` that means its rule set is off, which is §4.4's own wording (*"turns a rule
set off"*; `router-cli/src/lib.rs:270`) — and `/health` reports the entry as disabled. What is **not**
implemented is this key's *designed runtime* semantics: unloading a live fiber and rolling back its
effects when the config changes (§4's config-change paragraph below). Until then a `disabled` entry
behaves as the start-up switch it has always been, not as a live unload.

**No plugin can reach the byte boundary.** Whatever the list mounts, the client's bytes and the two
permitted span mutations of §2 stay the core's (DESIGN §13.1's P1 row; ADR-015): a plugin may propose a
**path-addressed edit plan** for a payload and nothing else — the parsed view is never what reaches the
wire (§4.4, ADR-019) — and the primitives that carry the invariants a plugin must not own (admission,
the decision record, the state write path, the `verified`/`inferred` label, and the loader itself) are
in the core **by definition, not by configuration**; DESIGN §13.6 lists each one with the constraint
that forces it.

### 4.4 Rule files (`rules/*.toml`) and `tee`

The rule-file format of `builtin/transform_rules` is in `rules/tool_output.toml` (the declarative
pipeline of ADR-003: filter stage, `match_output`, line keep/drop, truncation, `on_empty`, inline tests).

- A rule may declare `tee = true`. On a hit that did drop lines, **one line is appended** at the **end of
  the output**:
  `[router:tee sha256=<first 16 hex chars of the original payload's sha256> lines_dropped=<n> bytes_original=<n>]`.
- **The original text's storage location and retrieval channel (retrieve) are not implemented in v0.1**
  — this version **provides no** retrieval endpoint and defines no on-disk directory. The trace records
  only `tee_id` (§6 "transform"; `null` when tee is not enabled). The implementation is a separate change
  following the P1 compression direction (D3); before it lands, do not use `tee` as a "retrievable
  original text" capability.
- **The mode is asked for by the client, never by this file** (§2.1): a rule set is configured here,
  and it edits nothing unless the request carries `X-Router-Transform: transform`. There is no config
  key that enables a content transform, and that absence is the guarantee the passthrough promise
  rests on (ADR-019) — `config.example.yaml`'s `disabled:` knob turns a rule *set* off; it cannot turn
  it *on* for a request that did not ask.
- **What a rule produces is a value-span edit, not a new body.** A rule consumes one addressed payload
  node's text and returns that node's new text; the payload is re-encoded as a JSON string and spliced
  in place of that node's value span (ADR-007's discipline, one level deeper), so every byte outside
  the edited span — including every other message, the tool schemas and the client's whitespace —
  survives byte for byte. A `json_compact` rule therefore compacts *the payload*, never the document.
- **Payloads only, by declaration.** A rule selects its targets by `match_tool` / `match_kind`, never
  by inspecting content for a guess: no rule may touch a user or assistant message, the system
  instruction or a tool schema (ADR-003's boundary; ADR-019 item 3).
- **Admission is a gate, not a preference.** A rule is adopted only when its inline tests are green,
  the cache regression of §6 (`prefix_continuity` after enabling it) does not fall below the baseline,
  and its net gain is `verified` — a paired measurement of the same content with the rule on and off,
  read from the upstream's `usage` (§7). Every rule admitted by less than that is a rule whose saving
  nobody has measured.
- **`on_failure: passthrough` is the rule set's contract, not a fallback of last resort**: a rule that
  cannot load, compile, pass its inline tests or splice its edit does not apply, the payload travels
  verbatim and the record carries `errors[].kind = "transform_error"` (§8). Partial edits are not a
  state this version can be in.
- **Rule parameters are the L1 envelope's own example** (ADR-012 item 3 pre-registers
  `plugins.<id>.config.rules_file.<rule>.max_lines` with a range): a rule's numeric knobs are what the
  loop may tune inside that envelope, and nothing about a rule's *shape* is auto-adoptable.
- **Open item, named so it is not improvised:** a rule's `match_kind` is a "payload category
  declaration" (`rules/tool_output.toml`), and nothing yet says where the category comes from — no wire
  field carries it and §4 defines no tool→kind table. `match_tool` is sufficient for the first rules;
  the resolution of `match_kind` is registered as GAP-Q18 and is the implementing change's to settle.

### 4.5 `state` (local persistence) — contract

The gateway keeps **one local store**: a SQLite database in WAL mode, by default
`<directory containing this config file>/state/router.db` (`state/` is gitignored). v0.1 has **no `state:`
config key** — the path is fixed; a section that moves it (the way `trace.dir` does) is an additive future
key (§4.1's precedent, ADR-009).

One request leaves two records, and they have different jobs:

| Record | Job | Shape | Who reads it |
|---|---|---|---|
| **event log** (`events`) | the **state truth**: every state transition (session binding, upstream intent, quota charge, config applied, …) | one row per event, ordered by `event_id` | the serving path and its projections; **internal** — autowork never reads it (ADR-005) |
| **trace** (`trace.dir` JSONL) | the **analysis truth**: one `DecisionRecord` per request (§6) | one JSON line per request | `router replay` / `router stats` / autowork: the only product → autowork channel |

- **Join key: `request_id` + `event_id`.** The trace's `event_id` (§6 "identity") is that request's
  `request.received` event, and the rest of that request's events share its `request_id`; the two records
  are paired exactly, never by timestamp.
- **Bodies are never persisted** — neither in the log nor in the trace. The log keeps `body_hash` (the
  first 16 hex chars of `sha256(router-visible body bytes)`, the same convention as the §6 block hash) plus
  a pointer to the trace line; a payload exists only if the operator captured it out of band.
- **Derived state is not state**: the sticky table, the cache ledger and the quota counters are
  **projections** of `events`; a lost projection is rebuilt from the log, and losing one regresses
  statistics, never correctness.
- **A binding's expiry is a microsecond value derived from a millisecond knob.** `session.ttl` is a
  `DurationVal` — **milliseconds** (the duration grammar of `DESIGN` §12.5) — while the sticky table
  stores `expires_at_us` in the store's own clock unit: the binding event's `ts_us` plus the
  `session.bound` payload's **`ttl_us`**, which is **microseconds**. The conversion is therefore
  **× 1 000**, it happens **once** — where the serving path resolves the configured value, never in a
  consumer — and a reader that subtracts an event's `ts_us` from `expires_at_us` recovers the configured
  TTL by dividing by 1 000. Every consumer of that value obeys the same rule, including the account-move
  handoff of §4.6 that re-points live bindings — and that handoff's rows are anchored on the
  `session.bound` row they write, **never** on the `plan.switched` row that caused the move: *the binding
  event* is the row that carries the binding, and the anchor is a function of that row's own `ts_us`
  (`R27-F1`, closed by R28; `DESIGN` §12.10.5 note R7). (Stated here because it is the sticky table's own state.
  The shipped v0.1 build converted × 1 000 000 instead, so a configured `12h` was honoured as ~12 000 h —
  the pre-existing `R21-F6`, frozen and corrected by R27; the store's own fixtures are the convention's
  witness, `43_200_000_000` µs for a 12 h binding.)
- **A binding's state is read off one row, and a move is a write to that row.** The sticky table is a
  projection of `session.bound` rows **alone**: a session's `provider`/`model` come from its **latest**
  such row, `requests_seen` is that session's `session.bound` **count**, and `expires_at_us` is that row's
  `ts_us` + its `ttl_us`. Whoever owns a move owns a write to this row — the request's own resolution
  (§6's `route_changed`) *and* the plan policy's account handoff (§4.6 rule 1) — so the handoff re-points a
  family's live bindings by writing **one `session.bound` row per re-pointed session**, never by writing
  the projection alone: a state transition the log does not carry is not state (ADR-010), and a projection
  written without its row is exactly the disagreement a rebuild repairs *away* (`R27-F1`, closed by R28;
  the landing is `DESIGN` §12.10.5 note R7). A move is not a client request, and `requests_seen` therefore
  counts **binding writes**, not requests: `turn_index` (§6) is this count + 1, and it advances on a move.
  (One consequence is worth stating because it looks like a defect and is not: a session can gain two rows
  in one request — its own resolution moved it onto the route the handoff is abandoning, and the handoff
  then moves it again. The count is still a pure function of the log and live and rebuild agree.)
- **Durability**: intent / accounting events commit (`synchronous=FULL`) **before** the effect they
  authorize; projections are `NORMAL` and batched (ADR-009).
- **Crash window**: an upstream intent with no response is an `unknown_outcome` — the quota is **not**
  charged again and no cost is invented for it; reconciliation is left to the operator against the
  provider's own bill (§8; ADR-010 is normative, including its honest boundary: whether the upstream is
  idempotent is unverified, so "was it billed?" can be undecidable at the protocol layer).
- **Migrations**: forward-only, tracked in the store (`schema_version`); the **event payload is versioned
  from day one** (`events.schema_version`) because it is the truth — old rows are never rewritten and
  readers upcast.
- **Failure behaviour**: the store is a startup prerequisite (`serve` exits if it cannot open or migrate
  it); a failed **intent** write rejects the request before anything reaches the upstream; a failed
  **projection** write does not affect the request (it is rebuilt); a failed **trace** write keeps the §8
  rule (it never blocks the request).

### 4.6 `account` and `plan_policy` (plan-first routing)

**What it is.** A subscription account and a metered account can serve the same model. `plan_policy` names that
pair and makes the router prefer the subscription: while the plan is usable the family is routed to `primary`;
when the upstream declares the plan exhausted the family continues on `overflow` (a real, metered spend) and
comes back when the upstream allows it again (ADR-014). The pair is named by a **family tag** (§4.8), not by a
model id: the two routes' provider-native ids may differ (a coding endpoint's `k3-256k` and the metered
platform's `kimi-k2.7-code` are one family when their model entries say so), and when they are equal the tag
defaults to that id, which is the behaviour every config written before §4.8 has.

**`account` (a provider-entry property).** `coding_plan` | `api`; **absent ⇒ `api`**. It states what the
provider entry *is* — the account its key, endpoint and allowance belong to. A provider that declares
`account: coding_plan` may carry a `quota` plan (spec §4) but is **not required to**: a plan whose token
allowance is not published is still a plan, and §4.6's rules must not be reachable only by operators who can
supply a number nobody has (GAP-Q1).

**`plan_policy` (one top-level section, optional).** At most one policy in v0.1; a second family is an additive
future key (the `state:` / `retention` precedent), never a reshaped section.

| Key | Type | Default | Semantics |
|---|---|---|---|
| `family` | string | required | The **family tag** both routes' model entries carry (§4.8) — and the key of the state: the account state, the probe deadline and the switch records are per family. Load error unless a model entry of `primary`'s provider and a model entry of `overflow`'s provider both carry that tag. A model entry with no `family` key carries its own `id` as its tag, so a policy written when the two routes shared one model id keeps working unchanged. |
| `primary` | `provider/model` | required | The subscription route. Load error unless it is a roster route whose provider is `account: coding_plan`, and — when that provider declares a `quota` — unless **the model id the route resolves to** is covered by `quota.models` (§4.0's own-provider rule; `quota.models` names ids, never tags). |
| `overflow` | `provider/model` | required | The metered route. Load error unless it is a roster route, distinct from `primary`, whose provider is `account: api`, whose model entry carries the same family tag (its native `id` may differ from `primary`'s — that is what the tag is for, §4.8). It need not appear in `fallback`: for a request inside a family it is the first candidate after `primary` (§4.2). |
| `on_primary_exhausted` | `spill` \| `block` | `spill` | `spill`: the family continues on `overflow` at its real price. `block`: the request is refused with a **readable** reason (§8 `quota_exceeded`, 429) instead of being served from the metered account — the mode for an operator who would rather fail than spend. |
| `recover` | `probe` \| `none` | `probe` | `probe`: after `cooldown`, the next session's first request is admitted as a probe on `primary`; success switches the family back and is recorded. `none`: no automatic probe — the family returns at the plan's own window boundary when one is declared, and otherwise only by an operator action. |
| `cooldown` | duration | `15m` | The minimum interval between the move away from `primary` and the first admitted probe. A floor, not a schedule: ADR-011's cooldown for that provider must also allow the attempt, and rule 3 below may defer the probe further. |
| `overflow_monthly_cap_usd` | f64 USD | absent = no cap | Optional guardrail on the family's **metered** spend in a UTC calendar month, compared against measured usage priced by the config table. Once the month's spend has reached it, the family's overflow requests are refused (`cost_cap_exceeded`, 403). The request that crosses the cap is served — its cost cannot be known beforehand — so the overshoot is bounded by one request. A negative or non-finite value is a load error, and so is setting it on a family whose `overflow` route's provider is of a currency other than USD: the cap is denominated in **USD** (its own name), and comparing a USD ceiling with a spend in another currency is the silent mixing §4.8 forbids — so the policy refuses to load instead (message names `plan_policy.overflow_monthly_cap_usd` and the currency it found). |

**Hard rules** (the parts an implementation may not improvise; ADR-014 items 1–11 are normative):

1. **The switch granularity is the session.** A session (§6 `session`) is pinned to an account while it lives;
   it changes account only when **forced** (the primary is exhausted) or when the primary has been **proven
   usable again**. A per-request choice between the two accounts is forbidden.
2. **A probe happens only at a session boundary** — a new session, or a session's first request
   (`turn_index == 1`) — and **never mid-session**. A session already on `overflow` is moved back only when the
   upstream has judged `primary` usable (for example because another session's probe succeeded), and that move
   **must be recorded** (`result.plan_switch`, §6; the `plan.switched` event). A request with no session never
   probes.
3. **The upstream is the authority on exhaustion; the local counter is a warning.** Only an upstream `403`
   classified `quota_exhausted` (ADR-011) may move the account. The verdict is the *classification*, and the
   classification is made from the upstream's own answer — its status, its headers and its error body — on
   **both** forwarding paths (§4.2): a streaming request reads its own error head's body before the classifier
   runs, so this rule reaches the clients that stream, which is every shipped one. The local `quota_counters`
   count may neither refuse a request nor force a spill on its own (GAP-Q1: its denominator may be a
   placeholder); it is recorded
   in `cost.quota_after`, surfaced by `/health` and `router stats`, and it may **defer** a probe until the
   plan's declared window boundary has passed.
4. **Costs.** In-plan requests are accounted at the plan's marginal cost **0** and record `quota_after`;
   overflow requests are accounted at the model's **real price**. The account a request was billed under is not
   a trace field of its own: it follows from `decision.provider`'s `account`.
5. **A switch is priced as a failover**, because it changes the account's upstream cache namespace: the trace
   records the re-prefill (`result.plan_switch.reprefill_tokens` / `switch_cost_nano`), `inferred` at decision
   time and `verified` once the switched attempt's usage has landed (§6; ADR-011 item 9's convention).

**The `plan.switched` payload (spec amendment).** Besides the fields ADR-014 item 8 lists (`family`,
`from_account` / `to_account`, `from_route` / `to_route`, `reason`, `probe`, `reprefill_tokens` +
`switch_cost_nano`, `session`), the event carries **`cooldown_ms`** — the policy's then-current cooldown in
milliseconds. It exists because the `plan_state` projection is derived state (§4.5): a rebuild must
determine the row's informational `until_us` (`since_us + cooldown_ms`) from the log alone, without the
config that produced the event — and events are never rewritten, so a later `cooldown` change must not
change what the row said when it was written. The serving path itself always recomputes the probe gate
from the *current* config against `since_us`.

**Illegal combinations: what each one fails as.** The load-time checks are refusals at startup — the process does
not come up and the message names the config path and the reason (§4.5: there is no partially-started process).
They are deliberately **not** `error.type` values of §8: those describe the outcome of a request, and a config that
never loaded serves none. The two refusals the policy itself produces at run time do use §8's vocabulary (the last
two rows).

| Combination | Fails as | Message names |
|---|---|---|
| `account` is not `coding_plan` or `api` | load error | `providers[i].account` |
| `primary` is not a roster route | load error | `plan_policy.primary` |
| `overflow` is not a roster route | load error | `plan_policy.overflow` |
| `primary` and `overflow` are the same route | load error | `plan_policy.overflow` |
| `primary`'s provider is `account: api` | load error | `plan_policy.primary` |
| `overflow`'s provider is `account: coding_plan` | load error | `plan_policy.overflow` |
| no model entry of `primary`'s provider carries `plan_policy.family` as its family tag — or none of `overflow`'s | load error | `plan_policy.family` |
| two model entries of one provider entry carry the same family tag | load error | `providers[i].models[j].family` |
| the primary provider declares a `quota` and the model id `primary` resolves to ∉ that `quota.models` | load error | `plan_policy.family` |
| `overflow_monthly_cap_usd` is negative or not finite | load error | `plan_policy.overflow_monthly_cap_usd` |
| `overflow_monthly_cap_usd` is set and `overflow`'s provider's `currency` (§4.8) is not `USD` | load error | `plan_policy.overflow_monthly_cap_usd` |
| `cooldown` is not a duration (`15m`, `1h30m`) | load error | `plan_policy.cooldown` |
| any key inside `plan_policy` that §4.6 does not define | load error | `plan_policy` |
| the state is `overflow` and `on_primary_exhausted: block` | `quota_exceeded` (429, §8) | the family, the account state and the reason |
| the month's metered spend has reached `overflow_monthly_cap_usd` | `cost_cap_exceeded` (403, §8) | the family and the cap |

The `block` refusal is a property of the **state**, not only of the spill trigger: any request inside a family
whose account state is `overflow` — including a request of a session that was already in flight when the
family spilled — is refused while `on_primary_exhausted: block` (the operator's mode is "fail rather than
spend"; serving a session's next turn from the metered account would defeat it exactly as a fresh request
would).

Nothing else in §4.6 is an error: a `spill` is served (that is the point of the mode), and an `overflow` that fails
too walks on into §4.2's chain, whose own exhaustion is §8's `upstream_error` / `upstream_timeout`.

**Relation to the Guard stage (§3).** This policy **is one guard rule** and adds no pipeline stage: the chain
already asks "may this request go on this route?", and the answer uses the existing vocabulary — `Pass` (this
route is allowed), `Downgrade(<the overflow route>)` (a different route must be taken, and it is recorded),
`Reject { code, message }` (`block`, or the overflow cap). Its rule order is fixed: **probe admission → overflow
cap → account state**. §3's sentence is unchanged ("on a guard hit it acts per the configured policy: reject and
explain why, or downgrade to the `fallback` chain"); for a request inside a family, the family's `overflow` route
is simply the first route the downgrade considers.

### 4.7 `server.auth_token_env` (inbound token auth)

| Key | Type | Semantics |
|---|---|---|
| `server.auth_token_env` | string \| absent | The **name of an environment variable of the router process** whose value is the token this process expects from every inbound request to the three protocol endpoints (its value is never written here — §4's secrets rule, and a token is a secret) |

- **The key absent ⇒ no inbound auth.** That is not a gap: "no key" *is* the local, single-user mode
  this document has always described, which is also what makes the key backward compatible — a config
  written before the key existed behaves exactly as it did.
- **The key present, and the environment variable unset *or empty* ⇒ the process refuses to start** —
  exit code **`4`**, the code the other unsatisfiable-environment startup prerequisites use (the state
  store, the trace directory), and the reason names both the variable and the config path. A missing secret
  must **not** be
  read as "so no auth is required": that reading silently drops the operator's only access control,
  while the strict reading costs one start-up. **This is a requirement, not a recommendation**, and it
  has no override — there is no "auth off" switch to fall back to.
- The token is read **once at startup** and is never re-read: rotating it means restarting the process
  (v0.1 has no config-reload path for it).
- The token value is a secret of the same class as `api_key_env`: it is never printed by `/health`,
  never logged, and never written into a trace record or an event payload. Only the **variable's name**
  may be reported (§9's `auth` member of `/health`).

**Accepted header forms.** A request presents the token in exactly one of two headers:

| Header | Form | Note |
|---|---|---|
| `Authorization` | `Bearer <token>` | what an OpenAI- or Anthropic-shaped client sends by default |
| `x-api-key` | `<token>` | what a client speaking the Anthropic convention sends |

Either one is enough. When both are present, **a match on either admits the request**: a client that
sets both to the same value is the normal case, and a mismatch between the two is not worth a code of
its own. A malformed `Authorization` (no `Bearer ` prefix, or a `Bearer` with an empty credential)
**offers no credential** — it cannot admit a request — but the request did carry that header, so it is
the one named in the error body's `details.header`; that key is `null` only when the request carried
neither header.

**Comparison is constant time.** The comparison must not return early on the first differing byte and
must not branch on secret bytes: this is a requirement on the implementation, not advice. v0.1
hand-rolls it — `subtle` is not on §12.1's dependency allowlist and this is not worth a new dependency
for a byte loop. Comparing the two lengths first is accepted and documented here, exactly as
`subtle`'s slice comparison does.

**Exemption.** `GET /health` **always** answers without a token: it is the liveness probe, and a probe
that needs a credential cannot be used by whatever supervises the process. Nothing else is exempt, and
the exemption is **structural** — the guard is applied to the three protocol endpoints' routes only,
never to `/health` — not a path comparison inside the guard that a later edit could get wrong.

**A refusal is not an upstream failure.**

| Fact | Value |
|---|---|
| Response | `401` with §8's unified error body, `error.type = "unauthorized"`; `details.header` names the header the guard read (`"authorization"` / `"x-api-key"`, or `null` when neither was present) |
| `X-Router-Request-Id` | present — §8's "always" has no exception, and a refusal is the case an operator most needs to correlate |
| Fallback / retry | **never**: §4.2's chain and ADR-011's re-attempt rules are statements about *upstream attempts*, and a request the boundary refused makes none |
| Upstream | never contacted — no provider request bytes exist for it |
| State store | **no row**: the guard runs before §4.5's `request.received` event, so the request does not enter the pipeline and unauthenticated traffic cannot write to the store |
| Trace | **one record**, from the same sink as every other record (§6's one-record-per-request contract — every terminal outcome leaves a line — applies unchanged, and "am I being scanned?" is answerable only from the trace). Its fields are §6's **pre-pipeline** class and it carries `errors[].kind = "unauthorized"` |
| `router stats` | the record lands in `failed`, split out as `unauthorized`, and in `usage missing`; it contributes to no sum, no rate and no gate (§9.2's provenance rows already say so — this key introduces **no new figure**) |

### 4.8 `region`, `currency` and the route family tag (one vendor, two regions, two currencies)

Three keys, one subject: a vendor is reachable through more than one regional deployment, those deployments
publish their prices in their own currency, and they do not always serve the same model **id**. Each key sits
on the entry that owns the fact — a provider entry for the first two, a model entry for the third — and none
of them is a routing dimension.

**`currency` (a provider-entry property).** `USD` | `CNY`; **absent ⇒ `USD`**. It is the unit of every `price`
tier in that entry's model table, and of nothing else.

- **Why the entry, and not the model or the tier.** The four tiers of one model are one invoice line from one
  account, so they cannot disagree: a per-tier unit would be a field whose only legal value is the one its
  siblings carry. And the price list belongs to the **account** — the same argument that already puts
  `account`, `urls`, `wire_api` and `api_key_env` on the provider entry (§4.6): the same model id can be
  billed in CNY through one deployment and in USD through another, and only the entry knows which.
- **No conversion, ever.** router applies no exchange rate, stores no rate and never converts one currency's
  figure into another's (§4.0). A CNY price page is transcribed **as CNY**; a request served by a CNY entry is
  accounted and reported in CNY. An estimated rate is a number nobody published — the same defect class as an
  estimated price (AGENTS constraint 5) — and it would also make a report depend on when the rate was read.
- **Not derived from `region`.** A CN-region entry billed in USD is a legal configuration; deriving one key
  from the other would manufacture a fact the vendor's pages do not state.
- **The ISO code is exact**: `USD` / `CNY`, uppercase. Any other value — including a lowercase spelling — is a
  load error naming `providers[i].currency`.
- **The region decides which page is evidence**: an entry whose `region` is `cn` cites the CN official page in
  its `source`, an `intl` entry cites the international one (§4.0).

**`region` (a provider-entry property).** `cn` | `intl`; **absent ⇒ `intl`**. It declares which regional
deployment of the vendor the entry's endpoint and key belong to — the fact an operator needs when two entries
of one vendor sit in one roster.

- **Not a routing dimension.** A client still writes `provider/model`, the entry's own name is what resolves,
  and the deployment behind it is invisible (and irrelevant) to the request. Two deployments are two provider
  entries under two operator-chosen names.
- **Not an accounting dimension either.** The unit comes from `currency` above.
- **Surfaced.** `/health`'s provider list carries each entry's `region` and `currency` beside its name, its key
  variable and its availability (§9.1's neighbourhood; DESIGN §12.10.2), so "which deployment am I actually
  spending on" is answerable from a reporting surface rather than from a comment.
- **It is a field, not a name suffix.** The provider name is the client-facing route grammar and the trace's
  `decision.provider`; encoding a facet into it would invalidate existing route strings, aliases, `fallback`
  entries and the meaning of already-written records, and would force a reader to parse a name to recover a
  fact the file can simply state. The naming *convention* over operator-chosen names is documentation
  (ADR-018): `<vendor>[-<region>][-<account>]`, with `<vendor>` matching the key variable's product name.
- **The router does not check `region` against a host in `urls`.** Vendors own their host lists, a
  built-in table of them would rot, and a wrong-yet-declared region is a documentation error, not a routing
  one. The check that matters is the `source` rule above.

**The route family tag (`models[].family`, a model-entry property).** Optional non-empty string;
**absent ⇒ the model's own `id`**, which is ADR-014's rule unchanged and therefore moves no existing config.
`plan_policy.family` (§4.6) matches this tag — that is what lets two routes with **different provider-native
ids** be one family. When the plan-side entry's native id differs from the metered one (a plan may expose a
different id shape than the metered catalogue — measured example: ZAI's coding plans serve the **bare** ids
`glm-5.3` / `glm-5.3-flash` on every wire; the `[1m]` form their docs mention is a Claude-Code client-side
switch, refused 400 `[1211]` by the upstream when sent as an id, R16-1), each side must state the same `family:`
tag explicitly to complete the pairing; the pairing is never inferred from id equality.

- **A name, not an address.** A client never writes a tag: it writes `provider/model` or an alias (§3), and a
  bare tag resolves to nothing (`404 unknown_model`). The tag is never on the wire and never reaches a provider.
- **§2's two mutations are unchanged.** The outbound `model` is still the resolved route's own native id, and
  `decision.requested_model` is still the client's own string verbatim. The tag moves neither; it exists so a
  policy can name a pair the ids cannot.
- **The equivalence is asserted, not verified.** Nothing checks that two tagged routes really serve one model —
  the router cannot know and the operator can. It is deliberately explicit for that reason: **no rule infers a
  family from ids that look alike** (prefix, suffix, substring, case), because an inferred equivalence would be
  a claim about a provider's catalogue that no page makes.
- **A tag resolves at most once per provider entry** (a duplicate within one entry is a load error naming
  `providers[i].models[j].family`), so a tag unambiguously names a model within a provider.
- **A tagged entry no policy names is legal and inert** — the same standing as a plan account no policy routes.
- **A family may span two currencies** (a CN plan with an international metered spill, or the reverse). Nothing
  breaks: each request is priced by one table, so each record states one currency, and money is never added
  across currencies (§6, §9.2). The one exception is a scalar comparison — `plan_policy.overflow_monthly_cap_usd`
  — which is why that key carries a currency rule (§4.6).

**One amount, one currency (the invariant behind all three keys).**

- A request is priced by **one** entry's table, so a record has **one** currency: `cost.currency` is the
  currency of `decision.provider`'s entry (§6).
- A record written before this key existed is **USD by definition** (no non-USD route could be configured), so
  the trace version moves once and a window holding both vintages stays unambiguous (§6, DESIGN §12.6).
- **Money is never summed, compared or printed across currencies.** Counts (`requests`, `switches`, records)
  and ratios (cache hit rate) are currency-free and may aggregate over a mixed window; every money figure is
  reported once per currency present, each labelled (§9.2). A mixed window states **no** combined money total:
  the figures are complete, the one thing that would be a lie is their sum.
- **Two amounts in different currencies are never equal, greater or lesser** either: a comparison that would
  silently pick a unit is refused at load time where the config can express it (the cap, §4.6) and does not
  exist anywhere else.

### 4.9 `urls` (one complete URL per wire protocol)

**`urls` (a provider-entry property, required).** A map from a wire protocol (`chat` | `responses` |
`anthropic`) to the **complete URL** the router POSTs to for that protocol. There is no `base_url` and no
path composition: the value is used verbatim — nothing is appended and nothing is trimmed (ADR-020).

```yaml
    supports: [chat, anthropic]
    urls:
      chat:      https://api.moonshot.ai/v1/chat/completions
      anthropic: https://api.moonshot.ai/anthropic/v1/messages
```

- **The keys are exactly the declared cells.** `set(urls) == set(supports)`, checked at load: a wire in
  `supports` with no URL, or a URL for a wire the entry does not declare, refuses the start (the message
  names `providers[i].urls`). `wire_api ∈ supports` stays as it is in §4. An unknown key — a typo, or a
  protocol v0.1 does not define — is refused by the key type itself.
- **A value must be an absolute `http(s)` URL containing no whitespace**; anything else is a load error
  naming the path and the value found.
- **Why the entry, and not the model or the tier.** The endpoint belongs to the **account**, exactly as
  `api_key_env`, `account` and `currency` do (§4.6, §4.8): every metered vendor in the shipped roster
  (`providers.example.yaml`, the file the shipped root names; §4.14)
  serves its OpenAI form and its Anthropic form at *different* bases (re-read 2026-09-21), so the fact
  cannot live on a field that does not name a wire.
- **The router does not verify that a URL is the vendor's URL.** A wrong-yet-absolute URL is a
  documentation error, and the only defences are the `source` rule (§4.0/§4.8) and the human re-read
  (ADR-020, "honest boundaries").
- **The URL is not body bytes.** Nothing in this key touches the passthrough promise, the prefix hash or
  the cache ledger (§2, §7; AGENTS constraint 1).

### 4.10 `price.tiers` (banded pricing: one price per input-length band)

**What it is.** Some official price tables are published **banded by input length**: the same model carries one
rate while a request's input stays within a threshold and another rate above it (occasionally a third band).
A model entry that can record only one rate forces such a vendor to be transcribed as one of its bands —
under-pricing a long-context request, or over-pricing a short one, and either way quoting a price the page did
not publish *for that request*. A **tier** is one published band; `price.tiers` records the bands exactly as
the page publishes them (ADR-021).

```yaml
        price:
          peak: { multiplier: 1.0, windows: [] }     # one peak table per entry, **outside** the tiers (rule 3)
          tiers:                                     # ascending; the last tier declares no ceiling (rule 1)
            - up_to: 200000                          # this band's own ceiling, inclusive (rule 2)
              # section: "<the page's own heading for this band>" @<fetch date>   # rule 5: this band's citation
              input_miss: <…>
              input_hit: <…>
              cache_write: <…>
              output: <…>
            - input_miss: <…>                        # no `up_to` ⇒ no ceiling: everything above 200000.
              input_hit: <…>                         #   It MUST be the last tier.
              cache_write: <…>
              output: <…>
        source: "<the one official page carrying every band> @<fetch date>"
```

**Rule 1 — one shape, and the flat block is its degenerate case.** `price` carries either the four flat tiers
(§4, the **flat shape**) or `tiers:` (the **banded shape**) — never both, never neither; `peak` is required by
both. The flat shape means exactly what a one-tier list means: **one price for every input length**, i.e.
`tiers: [<the same four prices, no ceiling>]`. The two spellings must produce **identical money** for identical
usage, and an entry already written flat needs no migration: it is a one-band table, which is what it always
was.

**Rule 2 — the ceiling, and which band prices a request.** `up_to` is a band's **upper bound on input tokens,
inclusive**: a request whose tier-selecting input size `n` satisfies `n <= up_to` is priced by that band.
**The last band omits `up_to`** and has no ceiling — it covers every input above the previous band's ceiling.
The key's absence is the only spelling of "no ceiling", and `up_to` is a plain **integer number of tokens**: no
`k`/`m` suffix (unlike `context`, whose suffix multiplies by 1024 — a band's ceiling is compared against a
*measured* token count, and a reader should never have to guess which of 1000 or 1024 a page's "K" meant; the
page's own wording belongs in the citation comment, rule 5).

`n` is the request's **measured total input tokens**, `usage.input_total`: the whole prompt as the upstream
counted it, **cached tokens included** (`input_cached` is a part of `input_total`; the uncached remainder is
the token count that the `input_miss` bucket prices, §6). It is **not** `input_total + output` (that is the
quota convention, §4.6/GAP-Q1) and **not** the estimated prefix figure: a page's bands are about the size of
the prompt that was sent, so the band a request falls in never depends on how much of it was cached.

**The selected band prices the whole request** — a *banded* table, not a progressive one: its four prices price
*all* of this request's tokens, exactly as the flat table's four prices price all of them today. A one-token
change across a ceiling therefore re-prices the entire request. That discontinuity is the vendor's own, and it
is why rule 5 exists.

**Rule 3 — peak/off-peak is orthogonal to the band.** A model entry carries **one** `peak` table, at the price
level and *outside* the tiers (`peak` inside a tier is a load error). A request inside a `peak.windows` period
is multiplied by `peak.multiplier` **whichever band priced it**: the band chooses the four prices, the window
multiplies their sum once — the existing arithmetic, unchanged (sum the four buckets, then apply the
multiplier). No band can carry its own multiplier, and the multiplier is never re-read per band.

**Rule 4 — banding changes no bookkeeping.** The trace's `cost` group stays four money buckets + `total` +
`cost.currency`, computed by the same code; only *which* prices were used changed. **Which band priced a
request is not a trace field**: it is recoverable from the record itself (`usage.input_total` against the
priced config of that era), and the record carries the money, not the prices (ADR-021 states the trade-off and
the trigger for revisiting it). Banding introduces **no** saving, no delta and no new figure of any kind: a band
is a *price*, not a change to the request.

**Rule 5 — every band's price must be traceable to the page's own band.** The entry keeps one `source`
(`<URL> @<fetch date>`, §4.0), and it must be the page carrying **all** of that entry's bands. On top of it, a
banded entry carries **one comment line per tier**, immediately above that tier's first price, naming the
page's own heading for that band as the page words it — plus that section's own URL when the citation spans
more than one section or page — and the fetch date:

```yaml
              # section: "输入长度 0-200K" @2026-09-21
```

The loader does not parse prose and cannot check this line; a **review** does, and the check is one click: open
the cited page, find the band's heading, compare the four numbers. A bare URL with no band named fails that
check — which is the entire reason this rule is written down: the numbers of two neighbouring bands sit one
heading apart on one page and look alike.

**Rule 6 — what the loader refuses.** Each of these is a start-up refusal (the process does not serve), and each
message names the offending path `providers[i].models[j].price…`, the tier's index and the value found:

| Refused | Because |
|---|---|
| both shapes written, or neither | a price block is exactly one shape (rule 1) |
| an empty `tiers:` list, or more than **8** bands | a price block with no band states no price; 8 is the cap — no vendor publishes more than three bands, and a hand-written block stays reviewable |
| a band's `up_to` is `0`, negative, fractional or not a number | a ceiling is a positive integer: `n = 0` is priced by the first band, so a ceiling below 1 covers nothing |
| no band omits `up_to`, or more than one does | exactly one band has no ceiling (rule 2) |
| the band that omits `up_to` is not the last one | bands ascend and the unceiled band is the top one |
| two `up_to` values equal, or descending | the bands would overlap, so one input size would carry two prices and the request's price would depend on which one the reader meant |
| a band missing any of its four prices | every band states all four: a band that inherits a price from another band is a number the page did not publish for it |
| a band whose `input_miss`, `input_hit` or `output` converts to **0** | the same refusal as §4.0's flat block — a silently free band. `cache_write: 0` stays legal (the upstream charges no separate write price), stated per band |
| a `peak` (or any unknown key) inside a band | rule 3: the multiplier lives at the price level, once |

**The only zero a band may carry is `cache_write`.** §4.0 permits a zero cache-write price (the upstream charges
no separate write price) and refuses a zero in the other three; banding inherits that rule **per band** and adds
no exception — a page that publishes `0` for a band's `input_miss`, `input_hit` or `output` is not a table this
schema records, exactly as the flat block refuses it today. (If a page ever really publishes such a band, reading
it is a new decision with its own ADR, not an implementation choice.)

A **hole** between bands is not refusable because it is not expressible: a band's floor is the previous band's
ceiling + 1, so the list covers `[0, ∞)` by construction, and the only ways to break coverage are the two rows
above (a duplicated ceiling, a missing or detached ceiling).

**Rule 7 — the loader does not check the page, and the router does not guess.** A band's ceiling is a fact the
vendor published. The router never infers bands from a model's `context` window, from a tokenizer estimate or
from a sibling model's table, and never interpolates between bands. Where an official page publishes no bands,
the entry keeps the flat shape: writing a banded table nobody published is fabrication (§4.0, AGENTS
constraint 5).

**Rule 7.1 — what this schema deliberately does not express** (each line is a boundary, not an oversight; the
reasoning is in ADR-021's alternatives):

| Not expressed | Because |
|---|---|
| bands on **output** length | the output does not exist when the request is priced, and the published tables band on input; pricing a request on its answer would make the price depend on the answer |
| **progressive / marginal** brackets (the first N tokens at one rate, the excess at another) | no vendor publishes the bracket formula for these tables, so the arithmetic would be the router's invention and would silently disagree with the invoice |
| a **`peak` per band** | one multiplier for the entry; N copies of one table would let a band silently lack the multiplier and under-price peak traffic |
| a **per-band currency** | the unit belongs to the entry (§4.8/ADR-018), and the record already carries one `cost.currency` |
| a per-band **`source`** field | the entry has one source and it must carry all of its bands; the band's section is prose, so it belongs in the citation comment (rule 5), where a human reads it |
| **promotional / holiday calendars**, coupons, per-account rates | a calendar is a second time dimension with its own truth source; peak windows (rule 3) are the only time multiplier this schema models, the same stance as GAP-Q6 for holidays |
| any **hit-rate-dependent or conversation-dependent** rate (`cache_hit` discounts that vary with length, volume rebates, tiered plans) | the published tables don't carry them; a rate that depends on measured history is a reporting statistic, not a price, and nothing in this schema may depend on the router's own past |
| a **trace field naming the band** | rule 4: the band is recoverable from `usage.input_total` + the priced config of its era, and the record carries money, not prices |
| **inferring** a ceiling from `context`, from a sibling entry or by interpolation | constraint 5: a ceiling is a published fact, and a guess here corrupts the `verified` cost that every downstream gate rests on |
| a **new saving/session figure** of any kind | constraint 4: a band is a price, not a change to the request, so there is nothing to save and nothing to label |

**Rule 8 — figures the router computes before the upstream answers.** The band is selected from **measured**
usage, which exists only after the response. A figure the router must compute earlier — the switch's re-prefill
cost (`result.plan_switch`, a failover) and the cache-aware breakeven's two unit prices — has no measured `n`:
it is computed from the **first band** (the lowest ceiling's prices), the one band every entry is guaranteed to
have and a choice that invents no estimate. Every such figure keeps its existing `inferred` label (§7) and may
not be read as a band-faithful quote of the real cost; a band-faithful variant would need a request-size
estimate the router does not have (GAP-Q14/Q20, DESIGN §12.9 and §12.13).

**Selection semantics, worked at the boundaries** (`n` = `usage.input_total`; the figures below are *ceilings*,
not prices — the boundary rows are the unit tests' source, DESIGN §12.13):

| `price` | `n` | Band that prices the request | Why |
|---|---|---|---|
| flat (one price for every length) | `0` | the single band | there is only one, and it has no ceiling |
| flat | `200000` | the single band | same |
| flat | `10000000` | the single band | same — a flat table never changes band |
| `tiers: [{up_to: 200000}, {no ceiling}]` | `0` | 1st (ceiling 200000) | the first band starts at 0 |
| `tiers: [{up_to: 200000}, {no ceiling}]` | `200000` | 1st | the boundary belongs to the band that declares it |
| `tiers: [{up_to: 200000}, {no ceiling}]` | `200001` | 2nd (no ceiling) | one token above the ceiling |
| `tiers: [{up_to: 200000}, {no ceiling}]` | `10000000` | 2nd | beyond every declared ceiling ⇒ the unceiled band, which is always last |
| `tiers: [{up_to: 200000}, {up_to: 1000000}, {no ceiling}]` | `200000` | 1st | boundary, inclusive |
| `tiers: [{up_to: 200000}, {up_to: 1000000}, {no ceiling}]` | `200001` | 2nd | |
| `tiers: [{up_to: 200000}, {up_to: 1000000}, {no ceiling}]` | `1000000` | 2nd | boundary, inclusive |
| `tiers: [{up_to: 200000}, {up_to: 1000000}, {no ceiling}]` | `1000001` | 3rd | |

### 4.11 `router setup` — the guided first configuration, and the boundary of what it may write

*Status: shipped, and this section is its contract. `router setup` is a served subcommand — one of the three
the binary has (README's CLI block; CONF-43 keeps the two in step). The path that needs no command is the one
§4 and §5 already document and it is unchanged: copy `config.example.yaml` with `providers.example.yaml` kept
beside it, edit the roster, export the keys — and leave the copy in the repository root, where §4.12's third
candidate finds it for every command below. The rationale and the rejected alternatives are ADR-025; the
landing is DESIGN §12.14; the location rule this command's default obeys is **§4.12**.*

`router setup` is a **file-writing command that is not in the serving path**: it handles no request, and nothing
on the request path reads anything it wrote other than the config file itself. Its whole job is to turn the
template (the shipped example, §4) into **your** config, with the answers you give and **only** those.

**Command face.**

| Verb / flag | Semantics | Reason |
|---|---|---|
| `router setup [<section>]` | the guided wizard over one section, or over all of them when the argument is absent (or `all`) | hermes-agent's per-section granularity: someone who wants to change one thing is not dragged through the other six (the comparison and its sources are the survey DESIGN §12.14 names) |
| `--config <path>` | the file to write; when it is absent the file is resolved by **§4.12's discovery order** (explicit path > the XDG location > `./config.yaml` > the XDG location, created). The absolute path written is **printed**, with the rule that chose it | one rule for the file the gateway reads and the file this command writes: two rules is how "setup wrote it and serve reads something else" begins. The flag keeps the name `serve` / `stats` use; a card proposed `--out` and it is declined — the path is the **same file**, and two names for one path is how a CLI starts contradicting itself |
| `--from <path>` | the **template** to start from; default = the `config.example.yaml` **embedded in this binary**, plus the embedded **roster** template (`providers.example.yaml`) for the roster target below | the example is the file an implementation reads directly (§4, CONF-25's counterpart), so the default template must be the one of **this build's own commit**; an installed binary with no example beside it must still work, and a developer trying an edited template passes `--from`. It costs the binary the example's bytes. Since the example splits (§4.14), `--from` names the **root** template in every run **except** the roster swap of the `--force` row below: the `providers`-section run over an existing split root, where it names the replacement **roster** |
| `--non-interactive` | no prompt at all: every question takes its **default** | the CI / container path. On a fresh target with nothing overridden the result is byte-identical to the template (G1) |
| `--quick` | ask only about the items `--check` reports unsatisfied (the named environment variables that are missing); nothing missing ⇒ `nothing to do`, exit 0 | hermes-agent's *only ask what is missing*, with router's own baseline: under `deny_unknown_fields` and a complete example there are **no missing config keys** (§12.5's defaults row) — the only thing that can be missing at a site is an environment value |
| `--print [--json]` | print each section's keys with the value the file carries (and the state of a key the template ships commented out); no prompt, no write. A target that does not exist prints the **template's** values, labelled as such. A file that carries the roster **inline** is reported as such, naming the file a writing run would move it to (the shape step below; ADR-038) | a read-only surface is what answers "I changed it but it did not take effect" — the most expensive silent failure this repository knows (§12.5) |
| `--check [--json]` | load the file with the **same loader** `serve` runs, then check every environment variable the file **names** and print them; no prompt, no write. A file that carries the roster **inline** is reported as such — one line above the names (a `roster` member in `--json`), naming the file a writing run would move it to (the shape step below; ADR-038 D9) — and it is the *only* thing the inline shape adds: the names, the export snippets and the exit codes are the same over both shapes. No target ⇒ exit 2 | the read-only surface a script calls |
| `--dry-run` | print the edits the run would make (`<anchor>: <old> → <new>`, with the edit kind), in application order, plus the shape step's span when the base root carries the roster inline (`split: providers: lines 100-1058 (959 lines, 71069 bytes) → <roster>`); no write | the write strategy's safety story: a change is inspectable **before** it lands |
| `--force` | the **base** becomes the template instead of the file that is there: the target is replaced by the template plus your answers | this is both the escape hatch and the recovery from an unusable file. It is **not** `hermes setup --reset`: router has no in-code default set to reset to (§12.5: only the three defaults §4 states are defaults), so the shipped example **is** the default set and "reset" and "start from the template" are one operation. What it discards is any note **you** wrote into your own file, since the base becomes the template again; the `source:` provenance comments survive, because the base is a **file** and never a serializer (ADR-025). It is therefore the explicit hatch, not the routine path — a routine reconfigure is a bare `router setup`, and `--dry-run` prints the replacement first |
| `--backup` | before a write, copy the target to `<target>.bak` (one fixed name, replaced each run) | rollback for the anchored-edit path. `--force` **implies** it: the wholesale replace is the operation that can lose content, while an anchored edit's edits are bounded and printable with `--dry-run` |

**There is no `--reconfigure`.** On an existing file a bare `router setup` *is* the reconfigure: every question
shows the **current value** as its default. hermes-agent's `--reconfigure` is a compatibility no-op with exactly
that meaning, and a flag that merely restates the default is a lie in a help text.

**There is no environment-override layer.** A CI run that must differ passes `--from` / `--config` or writes the
file; there is no `ROUTER_SETUP_ADDR`-style channel. The only env-shaped facts here are environment *variable
names*, and a value read from the ambient environment would make the written config a function of the shell that
ran the command — the opposite of "the file is the single source of truth" (§4's usage note; §12.5's defaults row).

**Sections and the keys they may write.** Seven sections. Each is a group of keys a user answers in one sitting;
where a group coincides with a file block it takes that block's name.

| Section | Keys it may write | Kind | Default source | Target file |
|---|---|---|---|---|
| `server` | `server.addr`, `server.upstream_attempt_timeout`, `server.request_timeout` | value | the file's current value, else the template's | the root config file |
| `auth` | `server.auth_token_env` — the **variable name** only, plus whether the key is enabled at all | value / enabled | the file's state (the template ships it commented out) | the root config file |
| `session` | `session.ttl`, `cache.sticky`, `cache.breakeven.enabled`, `cache.breakeven.min_remaining_turns`, `cache.breakeven.safety_factor` | value | the file's current value, else the template's | the root config file |
| `paths` | `trace.dir`, `trace.rollover` (`hourly` is the only value §4.1 defines) | value | same | the root config file |
| `providers` | `providers[name=<entry>].api_key_env`, for every provider entry | value | same | the **roster file** — one the root names with `providers_file:` (§4.14), or one this run creates by moving the root's inline block (the *shape step* below, ADR-038) |
| `routing` | `plan_policy.family`, `.primary`, `.overflow`, `.on_primary_exhausted`, `.recover`, `.cooldown`, `.overflow_monthly_cap_usd` | value / enabled | same | the root config file |
| `plugins` | `plugins[id=<entry>].config.rules_file`, `plugins[id=<entry>].disabled` | value / enabled | same | the root config file |

**The target file, and why exactly one section has two of them.** The command writes a key **in the file
that owns it**. Six of the seven sections own keys of the root config, so they edit the root — the file
§4.12 finds. `providers` is the exception after §4.14: when the root names a roster, the provider entry (and
therefore its `api_key_env`) lives in the roster file, and the edit lands **there**; when the root carries
the roster inline, the run first **moves** that block into the roster file (the *shape step* below) and then
edits it there — so the column resolves to the roster for `providers` in every run, and to the root for the
other six. Nothing else about the section changes: the same key, the same question, the same refusal when
the anchor does not resolve.

**The shape step: a writing run never leaves the roster inline (ADR-038).** A root whose **parsed** shape is
inline-with-`providers` is normalized **before anything is planned** — the normalization is part of the
run's candidate like every other change, and it happens for a bare run, an `all` run and a one-section run
alike:

- the moved span is the `providers:` **header line through the last line the block owns** (the last
  non-blank line before the next top-level key; comment lines never terminate a block, and trailing blank
  lines stay in the root, where they separate its remaining keys), and that span's bytes **are** the roster
  file's bytes verbatim — entries, comments and their `source:` citations included;
- in the root, the header line's place is taken by the **embedded template's own `providers_file:` line**,
  terminator and all, and every other byte of the root is untouched: the wizard moves the file's own bytes
  and composes no prose (ADR-025's write strategy, ADR-038 D3);
- the roster file's **name** is the embedded template's own value — `providers.example.yaml`, the file the
  shipped root names — so a fresh run and a normalized run name one file. The wizard never invents a name;
- a file already present at that name is copied to `<roster>.bak` **unconditionally** and then overwritten:
  the operator's bytes are kept by name, and the run does not stop for a condition it can make safe;
- the root takes **no** automatic backup from this step (its block *is* the new file's content);
  `--backup` and `--force` are unchanged;
- the move is **reported**, never silent: the run's report, `--dry-run` and `--print` each state the span
  and the file it goes to (ADR-038 D9). **Since R45 (2026-09-26)** the same fact is stated on the fourth
  surface, the one D9 did not name: over an inline root `--check` carries it too — one line before its
  names, a `roster` member in `--json` (the row above) — and states nothing of the kind for a root that
  already names its roster.

Three consequences of the target column and the shape step, all part of this contract:

- **The candidate is the pair.** `--check` / `--print` and the write path load and validate the **root and
  the roster together** (`providers_file` is resolved first, by §4.1's rule, because it decides which file
  the `providers` anchors are resolved against). A run whose plan edits both files validates the candidate
  **pair** before either file lands, and a failure lands neither — G4's rule ("any refusal ⇒ the target is
  byte-identical to what it was") read for two targets rather than for one.
- **`--from <path> --force` replaces the file it starts from.** For the `providers` section under the split
  form that is how a roster is replaced **as a unit** — the operator hands the command a whole file they
  wrote, no anchor is created, no position is chosen and no style is reproduced (ADR-037 D9; §4.14). It is
  a replacement, **not** an insertion: the wizard still cannot add a provider entry to a roster, and the
  section still says so (DESIGN §12.9's Q21).
- **Only the inline-with-`providers` shape is normalized.** A root that writes **both** keys, one that
  writes **neither**, and one that does not parse are left exactly as they are, and the loader's refusal is
  the run's outcome (exit 2, nothing written). The wizard configures a file; it does not repair one that
  does not load (§4.14's ladder).

*Shipped since the example's split (§4.14):* the target-file column above is the behaviour — the shipped
example is the pair (`config.example.yaml` naming `providers.example.yaml`), the wizard embeds **both**
templates, and a fresh run writes both files. Since **ADR-038** an existing inline root is normalized by the
same run: a bare `router setup`, or any section-scoped one, moves the block into the roster file and writes
`providers_file:` in its place, so the shape a run produces is the pair whatever the file's history. The
inline root stays a legal shape **for the reader** — `serve`, `stats` and `--check` load it exactly as they
always did (§4.14) — but no run of this command produces one. **The load is the whole of what is
unchanged:** an inline root adds the one roster-fact line the `--check` row names (and a `roster` member
to its `--json`), and moves nothing else on that surface — not a name, not an export snippet, not an exit
code.

What is deliberately **not** a section:

- **`plans`** — a plan is not a file block: it *is* a provider entry (`account: coding_plan`, §4.6) plus the
  top-level `plan_policy`. Its answers live in `providers` (the key's name) and `routing` (the policy).
- **`aliases`** and **`fallback`** — file blocks whose members are single-line scalars, but what a user does to
  them is a **membership** edit (add an entry, drop an entry, reorder). This command has no insert and no
  delete, so they are **shown** (current value, one line per entry) and edited by hand. A section that could
  only retarget an existing entry at a fixed position is worse than the file.
- **`session` merges `cache`** — the two answer one question (how a conversation is identified, and whether it
  stays on one route), and the example's own comments bind them.

**The vendor facts are never asked for, only shown.** A price, a `source` URL, a `context` window, an endpoint, a
`models[].id` — every value whose authority is an official page (§4.0, AGENTS constraint 5) — is **displayed** by
the section that owns it and never prompted. The wizard does not invent a price, and it does not ask a user to
recall one: it shows what the file carries and what the file cites. A list- or flow-valued key
(`session.key_sources`, `aliases`, `fallback`) is display-only for the same reason.

**The write strategy: a verbatim template plus anchored single-line edits.**

1. **Base bytes.** The target's own bytes when it exists; the template's when it does not (or under `--force`).
   The consequence that matters: hand-written keys, a reordering, and every comment survive verbatim, because
   the file is **edited**, never reproduced.
2. **A plan.** Each answer that differs from the value at its anchor becomes one edit. Two edit kinds, and only
   these two: **`set-value`** — replace the value's byte extent on a **single line** whose key anchor resolves
   **uniquely**; **`set-enabled`** — add or remove the leading comment marker on the key's **own line** (the
   template ships `server.auth_token_env` and `plan_policy.overflow_monthly_cap_usd` commented out). A plan
   entry is `(line, byte range, replacement)`, and the replacement is encoded in the file's **own style** at that
   key (quoted iff the value there is quoted; bare booleans and numbers; the §12.5 duration grammar). The wizard
   changes a value, never a style.
3. **The failure boundary — refusal, never best effort.** The run **refuses and writes nothing** (exit 2, naming
   the key, the anchor and the reason) when an anchor resolves to no line or to more than one; when the value is
   not a single-line scalar (a flow mapping, a block scalar, a multi-line string — a hand-rewritten file rather
   than the shipped shape); when two edits' ranges overlap; or when the candidate does not pass the loader
   below. Anchors are never searched heuristically and never approximated: a near-miss here is a corrupted price
   table, and the whole point of the strategy is that the bytes which are not the answer are not touched.
   An anchor that does not resolve with **no** requested change to that key is a **warning** (`not settable in
   this file; left alone`) — nothing was going to be written there anyway.
4. **No re-serialization, at any verbosity, behind any flag.** A YAML writer round trip (`serde_yaml` on
   `RouterConfig`) emits a document **without** this file's `source:` citations and `TODO verify against official
   source` markers, and those comments are the price authority's only carrier (§4.0, ADR-018 / ADR-020). The way
   to *replace* a file wholesale is to hand the command a **different template** (`--from <path> --force`) — a
   file the operator authored and can read — never to regenerate one from memory. **ADR-025 records this
   trade-off, the rejected alternative, and its cost.**

**The secret boundary: names only.**

- The two keys this command may write are `api_key_env` (per provider entry) and `server.auth_token_env`. Both
  carry **the name of an environment variable**, which is exactly what §4 says the file may carry, and exactly
  what §4.7 / §12.11 say the token's *value* may never be (never a struct, a log line, a trace field or an event
  payload).
- The command **never reads a key value**: presence is probed against the environment as a set of names
  (`var_os(name).is_some()` — the same probe `serve` makes for a provider key). It never prints a value, never
  writes one, and never echoes what it read.
- Nothing is created or fetched: no key is generated, no endpoint is called, no pricing page is opened.
- **There is no `.env`.** The command introduces no second secret location: nothing in this product reads a
  `.env` (§4.7 reads the process environment and the CLI has no dotenv path), so a `.env` it wrote would be a
  file whose presence does not make a key present. It prints the **export** snippet for each missing name and
  stops there — `export DEEPSEEK_API_KEY='<paste the key here>'`.
- `--check` lists the variables the file names: for a provider key "not present"; for the token, **"absent" and
  "empty" as distinct states**, the distinction `serve` refuses the start on (§4.7, §12.10.2).

**Validation and atomicity, before anything lands.**

- **The same loader, on the candidate, before the write.** The candidate text is parsed by the same `RouterConfig`
  deserializer and the same `validate()` the `serve` startup runs — one shared entry point, so the two cannot
  drift — `deny_unknown_fields` included. **A candidate that does not load is never written**, and the message is
  the loader's own, naming the key.
- **Landing.** The candidate goes to a temporary file **in the target's directory**, is flushed, and is then
  `rename`d over the target (one atomic replace on one filesystem); the temporary file is removed on any failure.
  Nothing lands partially, and no other process can observe a half-written config.
  **The mode of a target that is already there is the operator's, and the landing keeps it.** The temporary
  file is created `0600` (below) — that is a **creation** mode, not the landed file's. When the run replaces a
  file that exists, the file that is there afterwards carries **the mode that file had**: `0644` stays `0644`,
  and a target the operator made **read-only** (`0444`) stays `0444` — the `rename` needs write permission on
  the target's **directory** and never on the file, so a read-only config is written rather than refused, and
  it is read-only again when the run returns. A landing therefore changes no attribute the operator set, and
  the run's own report (`0 edits applied`) is true of the mode as well as of the bytes. What this rule covers
  is the **mode**; the replace itself is a new inode, so the file is owned by the user who ran the command and
  a hard link, an ACL or an extended attribute attached to the old inode does not travel with it. A
  `<target>.bak` copy keeps the mode of the file it copied.
- **The target's directory is created when it is missing** (`mkdir -p`), because the default location is
  `~/.config/router/`, which does not exist on a fresh machine. The file this command creates is mode **`0600`**
  and every directory **it** creates is **`0700`** — set explicitly rather than left to the umask, and a no-op
  on a platform without Unix modes. Restricting both costs nothing even though no key *value* is ever written to
  the file, and a directory that already exists is never re-moded. The absolute path printed at the end is what
  makes a typo'd path visible instead of silent.
- **Which file gets written is printed, with the rule that chose it.** `--print --json` / `--check --json` carry
  the selection (`"selected_by": "flag" | "xdg" | "cwd" | "xdg-created"`), so "did my `--config` matter?" and
  "where did that file come from?" are answered by the command instead of by guessing (§4.12).
- **An existing target is the normal case, not a conflict.** No `--force` is needed to reconfigure the file that
  is there — the base is its own bytes and only the answered keys move. `--force` is the operation that
  *replaces* it (base = the template), and it keeps the previous file as `<target>.bak`.
- **Nothing to change ⇒ nothing is written — over a target that loads.** With an empty plan the command prints
  `no change: <path> left as it is` and exits 0. That is what makes a second run a no-op rather than a rewrite.
- **The loader is the gate on every run that reaches a plan, and an empty plan does not skip it.** The candidate
  — and when nothing was planned the candidate **is** the base — is parsed and validated before the run may
  report a no-op, so a file the loader **refuses** is refused by this command too: `exit 2`, naming the loader's
  reason, nothing written. The plan's emptiness is a statement about the operator's answers, never about the
  file. The shapes §4.14's ladder refuses are therefore refused here too, rather than told `left as it is`
  while `--check` (the row above) refuses them: **shape 1** (both keys written), **shape 2** (neither
  written), **shape 4** (a roster that is present and is not the roster block) and **shape 5** (a reference
  the roster does not resolve) — plus the coarser case that precedes the ladder, a root that does not parse
  at all. `--dry-run` reports the refusal rather than printing a plan of nothing. A no-op run is a no-op
  over a config that works. **Shape 3 is deliberately not this bullet's**: a root whose `providers_file`
  names a path that is **not there** is the arm a writing run *creates* the roster for (measured
  2026-09-26: the run writes 71 070 B at mode `0600` — the shipped roster's own bytes, sha16
  `2dbb9d6a5f80f4c9` — and leaves the root's bytes untouched), so what that arm **is** — a repair this
  command may perform, or a refusal, as its ladder row reads on its own — stays open, and this bullet
  decides nothing about it.

**Determinism — the frozen, assertable properties.** The written bytes are a function of (base bytes, answers,
template) **only** — never of the clock, the CWD, the answer order or the environment's contents (the environment
affects the *check* output and the exit code, never a byte of the file).

| # | Property |
|---|---|
| G1 | a fresh target, every answer at its default ⇒ the file's bytes are **identical to the template's**, and it loads with the same loader |
| G2 | with *k* answered changes ⇒ every byte outside those *k* lines' own extents is identical to the base, and the file's counts of `source:` and `TODO verify against official source` occurrences equal the base's (a relation over the run's own base, never a snapshot of a number) |
| G3 | the same answers twice ⇒ the file's bytes are unchanged and the second run **writes nothing** (content and mtime unchanged, `no change` printed) |
| G4 | any refusal ⇒ the target is byte-identical to what it was, and no temporary file is left behind |
| G5 | no environment **value** appears in the command's stdout, its stderr, its `--json`, or any file it wrote — only names, statuses and paths |
| G6 | the target is modified only after the candidate has passed the loader |
| G7 | the run reproduces under a different CWD — the **bytes written** are a function of (base bytes, answers, template) only — and no file it writes carries a timestamp. The one CWD-dependent step is §4.12's `./config.yaml` candidate, and it is reported rather than silent (G8) |
| G8 | the path the command reports and writes is exactly the one §4.12's discovery order selects from (`--config`, `$XDG_CONFIG_HOME`, `$HOME`, the CWD); the `--json` selection member names the rule that chose it; a file or directory the run **created** carries mode `0600` / `0700`; an existing directory is not re-moded **and neither is an existing file** — a target that is already there keeps the mode it had across the landing (`0444` stays `0444`), so no run of this command changes a mode the operator set |

**Exit codes** (the vocabulary `serve` already uses).

| Code | Meaning |
|---|---|
| `0` | the file was written; or `no change`; or `--print` / `--dry-run` printed; or `--check` found every named variable present |
| `2` | **refused, nothing written**: an unknown section; stdin not a terminal without `--non-interactive`; an unresolvable or ambiguous anchor; a requested change on a key that is not settable here; a candidate that does not load — **including the base itself, on a run whose plan is empty**; a missing template; a target that is a directory, or a target whose directory cannot be created because a path component is not a directory |
| `4` | `--check`: the file loads, but a variable it names is missing — or, for the token, empty — the same class `serve` refuses the start on (§12.10.2) |
| `1` | an unexpected I/O failure, with the reason printed |

**No terminal on stdin.** With stdin not a terminal and `--non-interactive` absent, the command prompts nothing:
it prints the exact working command line for that environment plus the export snippets for the names the file
carries, and exits **2**. `docker init`'s hard TTY error leaves a script with nothing to do; hermes-agent's
identical branch returns 0 while writing nothing, which reads as success to a script. Router takes the two
together — refuse (a run that did not do the work is not a success) **and** hand over the command that does
work. Answers are read as **lines**, an empty line taking the default, and a terminal-less *stdout* is not a
refusal, so `router setup | tee setup.log` works.

**The boundary of what `setup` does not do.**

| It does not | Because |
|---|---|
| make any network request, probe an upstream, list a provider's models, or fetch a pricing page | the command must be a pure function of local input: networked onboarding would make the config depend on an instant (AGENTS constraint 2), and "verify the price against the official page" is the operator's read, never the router's guess (constraint 5) |
| read, write or echo any key value; create a key; write a `.env` | §4.7 / §12.11: the value never enters a struct, a log line, a trace field or an event payload — and a wizard is none of those |
| write any vendor fact (price, `source`, `context`, endpoint, model id) | constraint 5: those values are transcriptions of an official page, and a copy of one in code is exactly what the example's comments exist to prevent |
| add, remove, reorder or reformat anything in the file | ADR-025: the edits are value replacements on existing lines; the comment-destroying rewrite is not offered |
| change anything on the decision path or the byte boundary | AGENTS constraint 1: this command is not in the serving path, and it writes no trace, no event and no store row |
| enable a transform | ADR-019 I3: transform mode is a **request** fact, the client's own opt-in. Switching a plugin entry off or on is a config change; it never edits a request that did not ask |
| introduce a config key | `deny_unknown_fields`: a key only the wizard understands is an unservable file. There is **no `_config_version`** and no `setup`-owned key — "what is missing" is measured against the shipped example, which §4 already makes the contract, not against a second table |
| publish a `$schema` | the editor-lint trick needs a schema *generator* and a second definition of the field set; the single source here is the parser's `deny_unknown_fields` plus the example (CONF-25), and two definitions drift |
| grow a subcommand family (`config get/set`, `doctor`) | one file, one guided command. The read-only needs are `--print` / `--check` / `--dry-run`; a later `router config` family can grow around them without becoming a second writer |
| add a crate dependency | zero new dependencies: stdin lines, stdout, and `std::io::IsTerminal` from std. No prompt/TUI crate, no dotenv, no schema generator |
| touch an existing conformance assertion, a gate or the corpus | AGENTS constraint 9 / ADR-012 |

**What must be asserted when this lands** (the shape the implementing round's rig takes; IDs in DESIGN §12.8):
the non-interactive file is byte-identical to the template and loads (G1); one overridden key moves only its own
line (G2); the refusal ladder — an unresolvable anchor, an ambiguous anchor, a not-settable key with a requested
change, a candidate that fails the loader — leaves the target untouched (G4); idempotence (G3); the secret canary
(G5); `--check`'s three exit codes and the token's absent-versus-empty distinction; and the interactive path
driven by a **PTY** script rather than by a Rust test harness.

### 4.12 The config file's location, and the paths inside it

Two separate questions, two rules: **which file** the gateway reads (this section), and **where a path written
inside that file lands** (§4.1's rule, restated here because the answer to the first question now moves the
second's outcome).

**The discovery order.** One rule, obeyed by `serve`, `stats` and `setup` alike; the first candidate that
applies wins.

| # | Candidate | Applies when |
|---|---|---|
| 1 | `--config <path>` | the flag is given. A path that does not exist (or does not load) is an error — it never falls through to a later candidate |
| 2 | `${XDG_CONFIG_HOME:-$HOME/.config}/router/config.yaml` | the file exists |
| 3 | `./config.yaml` (relative to the CWD) | the file exists |
| 4 | the XDG location, **created** (`mkdir -p`, file mode `0600`, a created directory `0700`) | `setup` only, and only when 1–3 found nothing. `serve` / `stats` **refuse** (exit 2) naming `router setup` and `--config` |

- **Reading** (`serve`, `stats`, and `setup`'s `--print` / `--check`): candidates 1–3, else refuse.
- **Writing** (`setup`): candidates 1–3, else candidate 4. The absolute path chosen, and the rule that chose it,
  are printed (`"selected_by": "flag" | "xdg" | "cwd" | "xdg-created"` in `--json`).
- **This rule decides *which file* is read, never what the file says.** The listen address, the plugin set and
  the roster still come only from the file (CONF-25), and the resolution lives in `router-cli`'s argument layer:
  the `serve` / `stats` entry points keep taking a resolved path, so the existing rigs drive them unchanged.
- **The table above gains no row for the roster, and that is the amendment §4.14 requires.** The four
  candidates are ways of *finding the config file*; the roster is **named** by a key inside the file they find
  (§4.14), so it is a reference and not a candidate. "Exactly one file is read" is
  true of the root, and — from the split on — the process reads the roster the root *names*, which is the
  one place in this section where "one file" must be read as "one **root**, found by one order, plus at
  most one roster it names": no merge, no precedence, no second search. The shipped example's own `# Usage:`
  line (§4's usage note) states it: `cp config.example.yaml config.yaml` with `providers.example.yaml` kept
  beside it is the complete instruction, because the shipped example is the pair (§4.14; DESIGN §12.14). A
  root that carries the roster inline is copied on its own and is unchanged — that shape reads one file, and
  always did.
- **Why the XDG location is the default *write* site**: it is outside the repository — a config there cannot be
  committed by accident nor removed by a `git clean` — it is the convention the user's other tools already
  agree on, and it makes the file `--config`-free for every later command.
- **The repository's own path stays supported**: `cp config.example.yaml config.yaml` (§4's usage line, the
  README) is candidate 3, and it is what keeps a repository-local development config working.

**What the file's own relative paths mean** (§4.1's rule, unchanged, restated where a user meets it): every
relative path in the file — `trace.dir`, `providers_file` (§4.14), every
`plugins[*].config.rules_file`, and the fixed `state/router.db` —
resolves against **the directory containing the config file**, never the CWD; an absolute value wins
(CONF-25 asserts both halves). So a config at the XDG location keeps its traces and its store beside itself,
under `~/.config/router/`. That is deliberate — one anchor, and one backup story: the config and its state
travel together (book/operations.md) — and the way to put the traces elsewhere today is an **absolute**
`trace.dir` (the `paths` section of `router setup` sets it; `~` is **not** expanded, the value is used as
written). Moving the *store* is not a second resolution rule: it is the additive `state:` key §12.5 already
anticipates, registered as **GAP-Q22** (DESIGN §12.9).

**What this is not: a configuration layer.** There is no merge across locations, no per-project override, no
remote or managed file. Exactly one file is **discovered** — and, from the split on, at most one more is
**named** by the file that was found (§4.14) — while the four candidates are ways of *finding* the discovered
file, never sources that combine. Nothing here reads files of equal standing and adjudicates between them: a
reference is not a layer, and there is no precedence order to get wrong. The layered model (opencode's eight
layers, codex's project/`--profile`/managed stack) is
the alternative ADR-025 records and rejects for v0.1, with the reason each layer would need — and with what it
would cost the "one file, one template, one backup" story this section's second rule depends on. That story
becomes "the root and the roster it names, plus its state" (§4.14; book/operations.md) — still a rule, still
one anchor, and not a search.

### 4.13 `server.max_body_bytes` (the inbound request-body limit)

| Key | Type | Semantics |
|---|---|---|
| `server.max_body_bytes` | integer (bytes), default **`2097152`** (2 MiB) | The largest inbound request body this process will **read** on the three protocol endpoints. A body larger than it is refused at the boundary, before the pipeline, with §8's `413 request_too_large` |

- **What the bound is for, stated plainly.** Every inbound request body is buffered in full before the router
  decides anything about it (the path split reads `stream`, and the byte boundary needs the bytes), and a body
  is what the store's event log and the trace are *about*. An unbounded inbound body is therefore a
  resource-exhaustion surface: one client's request size is multiplied by the number of concurrent clients, and
  the multiplier is chosen by the client, not by the operator. The key makes that multiplier the operator's.
- **Defaults are the behaviour that already shipped.** The value is a **declared** form of the cap this build
  has always had in effect — an implicit **2 MiB** one, imposed by the HTTP layer rather than by the router,
  with a refusal that is not this document's shape (see *The cap has exactly one owner* above). So the key is
  backward compatible in the same sense §4.7's is: a config written before it behaves exactly as it did. No
  client that works today stops working because this key exists.
- **The value is a byte count, and a value that disables the bound is refused.** The key takes a plain integer
  of bytes — no unit suffixes (there is no byte-suffix grammar anywhere in this file, and inventing one to save
  three digits is not worth a second grammar). A value **below `1024`** (including `0` and negatives) is a
  **load error**: exit code `2` with the key named, the code §9.2 already uses for an unusable config. There is
  deliberately **no "unlimited" spelling**: a bound that can be switched off is the surface this clause exists
  to close, and an operator who wants a very large bound writes a very large number — explicitly, where a
  reader can see it.
- **The cap has exactly one owner, and it is the router.** The bound is enforced by the router, at the
  boundary, *above* the pipeline — not by the HTTP framework's own default body limit. The framework's cap must
  therefore be **disabled** (the router's own check replaces it; DESIGN §12.15 names the mechanism), so that
  exactly one cap exists, its value is the configured one, and its refusal is this document's. Two caps would
  be two owners of one invariant, and the invisible one would win on the request whose size sits between them.

**A refusal is not an upstream failure** (the same fact table §4.7 gives its guard, with this clause's values):

| Fact | Value |
|---|---|
| Response | `413` with §8's unified error body, `error.type = "request_too_large"`; `details.limit_bytes` names the configured bound and `details.content_length` the length the request declared (`null` when it declared none) |
| `X-Router-Request-Id` | present — §8's "always" has no exception, and this is a refusal an operator correlates like any other |
| Fallback / retry | **never**: no upstream is contacted and nothing is attempted, so §4.2's chain has nothing to walk and no `skipped[]` exists (that array belongs to a walk) |
| Upstream | never contacted — no provider request bytes exist for it |
| State store | **no row**: the boundary runs before §4.5's `request.received` event, exactly as §4.7's guard does |
| Trace | **one record**, §6's pre-pipeline class (`event_id: 0`, `usage_missing: true`, nothing priced), carrying `errors[].kind = "request_too_large"` — a refusal that left no trace would be the one failure an operator could not count |
| `router stats` | the record lands in `failed`, split out as `request_too_large`, and in `usage missing`; it contributes to no sum, no rate and no gate (§9.2's provenance rows) |
| The connection | **closed after the response.** The refusal may be answered without draining the whole body (that is the point of a bound), so the connection may hold unread request bytes and cannot be reused for another request |

**How a request above the bound is answered, and in which order.** The check is header-first: when the request
declares a `Content-Length` above the bound, the answer is produced **without reading a byte of the body**;
when it declares none (a chunked body), the read itself is **bounded** and the same refusal is produced the
moment the bound is passed. Both arms produce one response and one record — a request cannot be refused twice,
and the two arms cannot disagree about which rule answered them.

- **Order against the other boundary checks.** The limit sits **above** the transform-mode resolution (§2.1)
  and **above** the path split, and it is evaluated **after** §4.7's auth guard — an unauthenticated request is
  still told `401` and never learns anything about this bound. Within the boundary, the body limit answers
  before the mode header: a request whose body cannot be read at all is refused for that reason, and the
  `X-Router-Transform` value it carried is not additionally adjudicated.
- **A streaming request is not an exception, and it cannot be cut mid-stream.** The router reads the whole
  request body *before* it opens any response, on both media — the `stream` member that selects the SSE relay
  is a fact of the body (spec §2.1's mode channel is a header, this one is not), so a request that asks for
  streaming is refused with the same complete, non-SSE `413` body every other refusal has, and no SSE head is
  ever sent for a request the router has not finished reading. The relay streams the *answer*, never the
  request: there is no state in which the bound could truncate a response that has already begun.
- **`details.stream` is absent, and that is not an omission.** The two forwarding media differ by that member
  in the walk's refusals (a fact the router knows by then). Here it does not — the member is a fact of a body
  this refusal never read, and inventing it would be a claim about content the router did not see (§7's
  discipline: an absent measurement is absent, never a value).

**The bound is not a rate limit, and not a content policy.** It says how much of one request will be read; it
says nothing about how many requests a client may make (§8's refusal vocabulary is unconditional and this
clause adds no counter, no window and no per-client state). A rate limit is a second admission rule with its
own contract, and this clause does not create it.

### 4.14 `providers_file` (the roster as its own file)

**One key, no default, no discovery.** A root config may carry `providers_file: <path>` **instead of** its
inline `providers:` block — exactly one of the two is written (§4's rule above; both, or neither, is a load
refusal). The value is a **required** path, resolved by §4.1's existing in-file rule, unchanged:

| Value | Resolves to |
|---|---|
| an absolute path | itself (lexically normalized) |
| anything else | `<the directory containing this config file>/<value>` — **never** the CWD |
| a leading `~` | **not expanded** — it is a literal path component, exactly as `trace.dir` and `rules_file` treat it |

There is **no** default location and **no** discovery: `<config dir>/providers.yaml` is not a candidate,
and §4.12's four-candidate table gains **no** row. The roster is *named* by the file that is read, never
*found* — a typo'd reference names itself in the refusal, whereas a hidden default would name nothing
(the stance §12.5's defaults row takes from the other side: only the three defaults §4 states explicitly
are defaults).

**What the roster file may contain.** Exactly **one** top-level key — `providers:` — carrying the same
block the inline form carries, under the same unknown-key strictness and the same per-entry rules (§4.8,
§4.9, §4.10). It is **not** a config file: it has no `server`, `session`, `cache`, `trace`, `plugins`,
`aliases`, `fallback` or `plan_policy` — every one of those is a key of the **root**, and a roster file
that carries one is refused by the same rule that would refuse it inline. A file whose top-level key is
not `providers:` — a `server:` block, a bare sequence, an empty document — is **refused**, and so is an
entry that breaks an existing per-entry rule; in both cases the message names the **roster's own path**.

**The refusals, and the key each one names.** Five shapes. The exact wording of each message belongs to
the implementation; the shapes and the keys below are this contract.

| # | Shape | Refused by a message naming |
|---|---|---|
| 1 | **both** `providers:` and `providers_file:` written in the root | **both keys**, and the root path |
| 2 | **neither** written (an empty roster is a decision: `providers: []`) | **both keys**, and the root path |
| 3 | `providers_file` naming a path that cannot be read — missing, unreadable, not UTF-8 | **`providers_file`**, the value as written, and its **resolved** path |
| 4 | a roster file that is not the roster block — top-level key not `providers:`, a second top-level key, an entry breaking a per-entry rule | the **roster's resolved path**, and the offending key or entry |
| 5 | a root key that references the roster and does not resolve there: `aliases.*`, `fallback[i]`, `plan_policy.primary` / `.overflow`, `quota.models` | the key path, the value found, **and the roster file** the reference failed to resolve in |

A written `providers_file:` whose value is not a path — a null, or the empty string — is **not** shape 3: it never reaches resolution. It is refused at parse time by `providers_file` itself, naming the key and the type found (a null coerced into a path would be a *search* for a file named `null`, the one thing this section forbids). The refusal is about the value's **shape**, not about the name it spells: a string is still a string, so `./null` names and loads a roster file called `null`.

**What does not change.** `RouterConfig::providers` stays **the** representation the serving path reads;
the join of the root with its roster happens **once, at load**, and nothing on a request path learns that
a second file exists (no second resolution rule, no second accessor, no roster service key). Every
cross-key rule — `aliases`, `fallback`, `plan_policy`, `quota` — is still checked in one pass, in the
existing order, by the existing validator.

**The roster is part of "the config" when you back it up.** Where the split form is used the config is a
**pair**: the root and the roster must travel together, because restoring one without the other refuses to
start (shape 3, naming `providers_file`). A backup that copies only `config.yaml` is therefore incomplete
— and the process says so at startup rather than serving without a roster. The state store remains the
third file, resolved against the config file's own directory as §4.1 and §4.12 already say
(book/operations.md).

**The identity of the effective configuration.** Which configuration a process is serving is identified by
a **byte digest** of the two files, not by a canonical form of their parsed values:

```
root_sha16    = the first 16 hex chars of sha256(the root file's bytes)
roster_sha16  = the first 16 hex chars of sha256(the roster file's bytes); the empty string when the roster is inline
config_digest = the first 16 hex chars of sha256(root_sha16 + ":" + roster_sha16)
```

`sha16` is §6's one hash convention (the same one the prefix block hash uses), so an outsider can recompute
the value from the two files alone — `printf '%s:%s' <root_sha16> <roster_sha16> | shasum -a 256 | cut -c1-16`
— without parsing anything. It is reported by `GET /health` (§9.1), written into the `config.applied`
event at startup, and carried by every trace record as `config_digest` (§6). A comment edit moves it, and
that is deliberate: the roster's comments are where the price citations live (§4.0). It is **attribution,
never a score** — it is not a config key, it enters no gate, and no report total is derived from it.

**Shipped.** `providers_file` is parsed and resolved at load, and the shipped example is the split form:
`config.example.yaml` names `providers.example.yaml`. `router setup` writes both files from its embedded
templates, `--check` validates the pair — a root naming a roster that is not there exits 2, naming the
resolved path — and `GET /health` reports the identity above (§9.1). Nothing in §4.14 changed a single byte
of the inline form, which stays legal, unchanged and loadable: exactly one of `providers:` and
`providers_file:` is written, and a root that writes both or neither is refused (§4's rule).

### 4.15 The reload — a configuration change takes effect without a restart

*Status: **specified, not served.** The mechanism is decided (ADR-039) and the semantics below are
ADR-040; the watcher, the publish and the keyed diff land with the reload's own implementation cards.
Until they do, a process still serves the configuration it loaded — nothing on this page is observable
yet. Nothing here adds a flag, a signal or a key: the reload is default behaviour, like `setup`'s pair
write.*

A running process **notices that the pair changed and serves the new configuration**, with no restart and
nothing for the operator to remember. The change is noticed by a file watcher and *decided* by the
identity of §4.14: the process re-reads the root and the roster it names, recomputes
`config_digest = sha16(root_sha16 + ":" + roster_sha16)`, and **only a different digest is a revision**.
A file touched without changing a byte is therefore nothing at all, and neither is a rewrite that leaves
both files' bytes identical.

Five rules, each one something a user may rely on:

| # | Rule |
|---|---|
| 1 | **The candidate is the pair, and the same loader is the gate.** The root and the roster are read together and parsed and validated by exactly the loader `serve` starts with (§4.11's *the loader is the gate*), `deny_unknown_fields` and §4.14's refusal ladder included. |
| 2 | **A refusal is not applied, and the process does not stop.** A candidate the loader refuses leaves the running process serving the revision it already had, reports the loader's own reason, and writes **nothing** to the state store. This is the deliberate opposite of startup: at startup a bad config is `exit 2` because nothing is serving yet; a live process that stopped serving because a file was mistyped would be a worse failure than the mistake. |
| 3 | **One revision serves a request, from arrival to answer.** A request takes the revision in force when it arrives and is answered by that revision — there is no half-applied configuration and no request that sees two. A request already in flight when the change lands finishes on the revision it started on. |
| 4 | **Only what changed takes effect, and a change that reaches nothing on the wire leaves the prompt cache alone.** The outbound bytes of a request remain a pure function of the client's own bytes and the revision in force; the reload adds no normalization, no re-ordering and no rewrite of what you wrote. A revision that changes only keys the upstream never sees — `session.ttl`, the cache and breakeven defaults, prices, quotas, `currency`, `region`, a path or a default — produces byte-identical outbound requests, so a live conversation's cached prefix is untouched. A revision that *does* move a wire-visible fact (the resolved model a request selects, or the transform rules that are mounted) invalidates the upstream prefix for conversations in flight — the cost of the change is then **attributable**, not hidden: the records after the switch name the new `config_digest`, and the prefix break shows up in `prefix.continuity` and `cache_control_breaks` (§6). |
| 5 | **The state store is continuous across a switch.** Sessions, the cache ledger, quota counters and cooldowns all survive it unchanged; the switch adds **one** row to the event log — a `config.applied` (§4.5), naming the revision it applied. There is no second ledger, no migration and no revision column: the identity stays what §4.14 says it is, *attribution*, and it never becomes a key. |

**Keys the reload refuses, because the process already holds what they name.** `server.addr` (the listener
is bound once) and `trace.dir` (resolved at load and held by the trace writer) — and the keys whose only
consumer is something the process built once and keeps, such as the upstream-attempt timeout behind the
provider clients or the inbound body bound (`server.max_body_bytes`, §4.13). A revision that changes one
of them is refused with that key named, the process keeps serving, and the remedy is a restart — a reload
that re-bound a listener would be a different capability (drain and rebind), not a configuration change.
The rule behind the list is the **read site**: a key the serving path reads per request or per revision
travels with the revision and may change under a running process; a key read once into a long-lived object
is refused until the implementation rebuilds that object with the revision. The direction is deliberate —
widening this later is a smaller change than narrowing it.

**What a process keeps when it forgets a revision.** Nothing durable: the revision in force lives in the
process's memory and is re-derived from the files at the next start, where one `config.applied` row is
written exactly as it always was. That is what keeps the stateless-client boundary of §4.5 intact — a
restarted process serves one revision to every client, and no stored state has to be reconciled.

**Pending (an owner's ruling, not a card's): what a conversation that spans a switch sees.** Rule 3
settles the *request*: one revision, arrival to answer. Whether a **conversation** keeps the revision it
began on until its binding expires, or whether its next request is served by (or refused under) the new
one, is a product decision. ADR-040 drafts both arms with their consequence for a client mid-conversation
and picks neither; **no implementation of the session-level policy may land before that ruling.** The
recommendation recorded there is the pin arm, held in memory only and bounded by `session.ttl`.

The landing is DESIGN §12.20; the reasoning, the rejected alternatives and the state the process ends in
after each kind of failed swap are ADR-040; the file-watch crate and the dependency row it spends are
ADR-039. The observable facts a reader can check are named there as `RV-1`…`RV-7`.

## 5. Onboarding prerequisite (mandatory)

The client must bypass any local system proxy, otherwise **the request does not reach router at all**:

```bash
export NO_PROXY=127.0.0.1,localhost
```

Measured (2026-09-19): the macOS system proxy is configured as `127.0.0.1:8080` with `127.0.0.1` in its
exception list, but `reqwest` (codex) does not honor the exception list, so localhost requests are sent
into the proxy all the same and the client finally reports `503 Service Unavailable`. The same holds for
`httpx` (hermes).

### 5.1 The token, when inbound auth is on

`server.auth_token_env` (§4.7) is off unless the operator writes it. When it is written, **every
request to the three protocol endpoints must carry the token**, in `Authorization: Bearer <token>` or
`x-api-key: <token>`; `GET /health` never does. A client whose token is missing or does not match gets
`401 unauthorized` — which, unlike every other failure in §8, is not a symptom of anything upstream:
nothing was sent anywhere. It is the same class of prerequisite as the proxy line above (the client is
set up wrong, not router), which is why it is stated here as well as in §5's neighbourhood.

## 6. Observation contract (one DecisionRecord per request)

The trace is the **analysis truth**: one JSON line per request, and the only product → autowork channel
(ADR-005). State has its own truth — the event log of §4.5 — and the two records are paired by
`request_id` + `event_id`.

| Field group | Contents |
|---|---|
| config | `config_digest` — a **top-level** field beside `schema_version`, not a nested group (the byte digest of the root file and the roster, §4.14; see below) |
| identity | `request_id`, `event_id` (the state-truth anchor, §4.5), `client` (UA-normalized), `session` (from §4 `key_sources`, preferring `prompt_cache_key`), `thread_id`, `turn_index` |
| protocol | `protocol_in`, `protocol_out`, `translated` (bool), `lossy[]` |
| decision | `provider`, `model` (the resolved **provider-native** model id, §2 — the string the upstream received), `requested_model` (the client's own `model` string, verbatim; `null` when the request carried none), `selection_source` (explicit/alias/plugin), `plugin_chain[]`, `decision_ms` |
| state | `stateful_inbound` (`store != false` or non-empty `previous_response_id`), `sticky_hit`, `cache_control_breaks` |
| prefix | `prefix_blocks[]` (token count + hash per block; **block granularity and hash definition below**), `prefix_continuity` (longest common block ratio relative to the previous request in the same session) |
| transform | `transform_mode` (`passthrough` \| `transform`; always present — the mode in effect for **this request's outbound body**, §2.1), and `transforms[]`: one entry per step **that changed the payload**, each `plugin` (the rule id), `edited_paths[]` (`{ path, bytes_in, bytes_out }`, where the two byte counts are the **payload text's** length before and after the step — the length of the spliced JSON string span differs by its escaping and is not claimed here), `added_input_tokens`, `saved_input_tokens`, `saved_output_tokens`, `cache_impact`, `verdict` (verified/inferred), `tee_id` (optional; `null` when tee is not enabled in v0.1). **Every step must report its token delta**: a step that cannot is not admissible as one |
| usage | normalized `Usage { input_total, input_cached, cache_write, output, reasoning }` |
| cost | `cost.input_miss`, `cost.input_hit`, `cost.cache_write`, `cost.output`, `cost.total`, **`cost.currency`** (§4.8: the unit every amount in this group is denominated in), `quota_after` |
| result | `status`, `upstream_status`, `failover_from`, `plan_switch` (present only when the plan policy of §4.6 displaced the request's account), `overhead_ms`, `upstream_ms` |
| failure details | `errors[]` (an array; **no failure = empty array, do not omit**), each `{ kind, message, plugin?, details? }`; `kind ∈ {transform_error, upstream_error, trace_write_failed, internal, unauthorized, request_too_large}` (§8 — the vocabulary is shared with §8's `error.type` table, and the two lists move together; `unauthorized` is §4.7's guard, `request_too_large` is §4.13's bound) |

**`protocol` records what left the process outbound, and `translated` records an event — never a comparison.**
`protocol_out` names the wire the request's bytes actually went out on. Because only a candidate whose
`wire_api` equals the inbound protocol may be attempted (§2, §4.2), it equals `protocol_in` on every record of
an attempt and **may never name another protocol**; it is `null` only in the classes where no route was
selected (the pre-route rows below, the resolved route's `400`/`501`). `translated` is true only when the
attempt that carried the request re-encoded the body through a mapper; v0.1 ships no mapper, so it is **`false`
on every record this build writes** and a record of this vintage may not be read as "a translation happened"
(the R11-F1 record — `protocol_out` a foreign wire with `translated: true` over an untranslated body — is the
defect this sentence retires). `lossy[]` stays `[]` for the same reason.

**A record's money is in one currency, and the record says which** (§4.8). One request is priced by one
route's price table, so the whole `cost` group is denominated in that entry's `currency`, and `cost.currency`
states it — a trace field that said only `184000 nano` would be ambiguous the moment a CNY route exists, and
the trace is read without the config (ADR-005). The rule is exact: `cost.currency` equals the `currency` of
the provider entry `decision.provider` names. A record **without** the field is a v1 record and is **USD by
definition** (no non-USD route was configurable when the format had no place to say so); this field's
introduction is what moved `DecisionRecord.schema_version` to **2** (DESIGN §12.6), and a window may hold both
vintages without ambiguity.

**`usage_missing: true` means "no usage was measured for this request"** — it is not only "the upstream
answered without a `usage` member". A request that never reached an upstream is in the same class: a
pre-route rejection, a connect failure, a request the inbound auth guard refused (§4.7), and a request the
body bound refused (§4.13). The flag
is what keeps such a record out of every rate, every sum and every gate — `router stats` counts it on
its own line and prices it nowhere (§9.2) — and a record carrying it is never read as 0 usage.

**`state.sticky_hit` is one predicate about the moment a request arrived, and it is not a constant.**
*Did this session already have a binding row?* — did the `sessions` projection of §4.5 hold, for
`identity.session`, a row that had not expired at the moment this request arrived and **before this request
wrote anything of its own** (§4.5's row 4 is where a request writes one). The value is therefore fixed by
what the request finds in the store, never by the wire it travels: a **new** session's first request is
`false`, a later request of the same session is `true` while that binding is still live (and `false` again
once it has lapsed — that expiry is the projection's own), and `session: null` is always `false`. It is
**one value per request, and the same value on both forwarding media**: the value that decides whether a
`session.bound` event is written (§4.5) is the value the record carries — read **once**, before any binding
write, and never recomputed within the request. Both halves of that rule were broken until R21 (on the
streaming path the read sat *after* its own binding write, so a fresh session's first request reported
`true`); that finding is R11's `R11-F2`, with its three measurements and its read sites — the two reads
inside `bind_session` at `stream_forward.rs:499`/`:506` and the record's own read at `:831-834` → `:1519`,
against the buffered path's single read before its write, `forward.rs:896`. The **shared failure record** is
the same class, registered structurally by R21-1 (which does not measure it): `record_failure_trace`
evaluates the predicate at record time — `forward.rs:628`, reached from the buffered path's failure branch
at `:594` and the streaming one at `stream_forward.rs:228` — which is *after* that same request's binding
write. DESIGN §12.6 carries the symbol-level landing and §12.8 the case that pins it (`CONF-66`).

**The binding's write rule has a second arm, and it is one measured value: `route_changed`.** The
`session.bound` row is the record of the route a session is on, so its writer's rule (`DESIGN` §12.10.5
row 4; §4.5's sticky table) is *write it when the binding is **created or moved**; write nothing on a
sticky hit whose route is unchanged*. `route_changed` is that rule's second input, and it is **a value,
not a constant**: true exactly when **the binding that existed before this request differed in provider or
model from the route this request resolved to** — the route **after** the guard chain (§3, §4.6) and
**before** this request's own attempt — and false when there was **no prior binding** (nothing existed to
have moved) or when the prior binding already named that same provider and model. Both inputs come from
**one read**, taken at session resolution on **both** forwarding media — the same read that gives
`state.sticky_hit` its value, before anything this request writes — so a session's streamed ask and its
buffered ask cannot disagree about whether the binding moved, and the arm stays a pure function of
(content, stable config): it adds a store row, never a byte on the wire. One shape follows directly: a
session whose client `model` string changes from `p1/m-x` to `p2/m-x` writes exactly one `session.bound`
naming `p2/m-x`, and that write is what moves the `sessions` projection and advances `turn_index`; a
request resolving to the route its session is already on writes nothing, which is the arm that must not
regress. A move the plan policy's account handoff makes **later in the same request** is that handoff's own
accounting (§4.6 rule 1) and **not** this row's edge — it is not one of the two inputs above (it happens
*after* the request's own bind, and it compares nothing against the pre-request binding) — but it **is**
this row: the handoff writes one `session.bound` per re-pointed session, of the same payload shape, and the
`sessions` row rides on that row's own `ts_us` exactly as any other bind's does (`DESIGN` §12.10.5 note R7).
So a move advances `turn_index` whoever owns it, and `requests_seen` counts **binding writes** rather than
client requests; where that matters — the probe's `turn_index == 1` boundary (§4.6 rule 2) — a move is inert
by construction, because a re-point can only reach a session that already has a live binding (both writers
insert `1` on create), so its count was already ≥ 1 and the boundary is not crossed. The row is not
optional: without it the log does not contain the move, so a rebuild repairs the projection *back* onto the
abandoned route, which is what `R27-F1` measured (twice: `requests_seen` live 3 vs rebuild 1, and, after a
spill with no recovery, a **provider-level** disagreement — live `api` vs rebuild `coding_plan`) and what
R28 closes. All three writers of this row (the create arm, the request's own move arm, the handoff) share one
shape and one projection rule, which is what makes `requests_seen` a pure function of the log. (Both halves were unmeasured until R27: the shipped build
passed a literal `false`, so a moved binding wrote **no** row and left the `sessions` projection on the
stale route — the pre-existing `R21-F5`, frozen here and pinned by `CONF-80`, `DESIGN` §12.8.)

**The rest of the group is a constant in v0.1** (known gap G-F): a record has one writer
(`Accountant::commit`) and it writes `stateful_inbound: false` and `cache_control_breaks: 0` on every
request — no serving path writes any other value. The definition above stays the contract to land: `store`
and `previous_response_id` are never read (they are forwarded as the client's own bytes, §2), and the
sticky-binding read that gives `sticky_hit` its value is the projection read of §4.5, which now answers for
both the `session.bound` event and the record's field. A reader must therefore not take
`stateful_inbound: false` as "the client sent no server-side state", and the `stateful_unsupported` row of
§8 cannot fire. ADR-004 item 3 is the ruling this part of the group lands.

**This restores a definition, it does not change one.** `sticky_hit`'s meaning was stated above from the
start; R11-F2 measured a serving path departing from it, and R21 aligns the paths to it — so no field is
added, removed or retyped, no `errors[]` / refusal shape and no price moves (DESIGN §12.8).

**The `transform` group is a mode plus a ledger, and both are needed** (ADR-019). `transform_mode` is
the word in effect for *this request's outbound body*: `passthrough` on every request that did not ask
for a transform (§2.1), and on every request refused before the transform chain ran (a 401, a parse
rejection) — no step was even planned, so nothing is claimed. `transforms[]` carries one entry **per
step that changed the payload**, written with the step's own report (the event row of the same name,
`transform.applied`, ADR-010), so an empty array is ambiguous between *no mode* and *mode on, nothing
matched*; that ambiguity is the reason the mode is its own field instead of being inferred from the
array. Each entry names the rule that fired, the payload path it touched with its byte counts, its
token delta and that delta's label — and **an entry describes the plan, not what reached the wire**: a
request the guard chain then refused keeps its mode word and its entries, and whether those bytes went
upstream is `result.status`'s answer (a refusal after the chain ran is a different fact from one before
it). A step whose delta cannot be stated is not admissible as a step: an unaccounted content edit is the
exact failure the mode exists to make impossible.

**`config_digest` identifies the configuration that priced the record, and `schema_version` does not move
for it.** The value is a **byte digest** of the two files the effective config came from (§4.14):

```
root_sha16    = the first 16 hex chars of sha256(the root file's bytes)
roster_sha16  = the first 16 hex chars of sha256(the roster file's bytes); the empty string when the roster is inline
config_digest = the first 16 hex chars of sha256(root_sha16 + ":" + roster_sha16)
```

`sha16` is this section's own convention (the prefix block hash is its other user). The value is recomputable
from the two files by one shell line — `printf '%s:%s' <root_sha16> <roster_sha16> | shasum -a 256 | cut -c1-16`
— with no parser, no `Serialize` and no canonical form of the parsed document anywhere in the path.

- **It is an additive field, which is why the version stays `2`.** Every record this build writes carries it;
  a record **without** it was written before the field existed, and a reader must take that as \"the
  configuration is not recorded for this request\" — never as an empty digest. No existing field changes
  meaning, and none changes *how it is read*, which is the distinction that moved the format to 2
  (`cost.currency`: a consumer that ignores it sums a CNY amount into a USD one). An added key a reader can
  ignore is exactly the case DESIGN §12.6's additive rule already covers.
- **Attribution, not a score.** The digest identifies the **inputs**, not a behaviour: identical bytes are one
  identity (the point), a one-character comment change is two. It is not a config key, it feeds no gate and
  no report total, and no saving may be attributed to a config revision without the same-run baseline the
  gates already require (§7; AGENTS constraint 4). A comment edit moving it is the honest reading here —
  the roster's comments carry the price provenance (§4.0).
- **One value, three surfaces.** The trace field here; `/health`'s config member (§9.1); and the
  `config.applied` event written once at startup, beside both absolute paths and both file digests.
- **The empty string is not a value any served surface reports.** The digest identifies the configuration a
  process **loaded**, and a served process has one — it found a config file, and the load is what the process
  is — so the value it writes on every record, reports on `/health` (§9.1) and writes into `config.applied` is
  **non-empty**, over both shapes of §4.14: `root_sha16` is never empty, and an inline roster contributes the
  empty `roster_sha16` *as an input* which the recipe hashes together with the root's half, so the composed
  value is non-empty there too. The empty string exists only where there is **no configuration behind a
  record** — a test or a tool that builds one directly — and such a record is not a served record: it may not
  appear in a served trace. **A writer wired into the serving path that cannot produce a non-empty digest is a
  defect, and forgetting it is loud, not silent**: it fails at construction or at its first write, and what it
  may never do is stamp a record with an identity of `""` (DESIGN §12.6 states the same invariant at the
  writer seam). The **reader** half is unchanged and is what that phrase keeps true: a record that carries no
  digest was written before the field existed, and a reader takes it — and the empty string with it — as *"the
  configuration is not recorded for this request"*, never as an identity.

**A request refused at the boundary** — by §4.7's auth guard, or by §4.13's body bound — is in a class of its
own: it never entered the pipeline, so its record carries what is observable at the boundary and invents
nothing. The table below is that class, instantiated for the guard's `401`; §4.13's `413` is the same class
field for field, with exactly three differences, named under the table.

| Field | Value | Why |
|---|---|---|
| `config_digest` | the identity of the **loaded** configuration — the same value every record of this process carries | it is a fact of the config, not of the request: the boundary runs before any decision but *after* the config was loaded, and a record with no configuration behind it would have no creator. Written by the same writer, on this class as on every other |
| `identity.request_id` | allocated, from the same counter as every request | §8's error body and `X-Router-Request-Id` carry it |
| `identity.event_id` | **`0`** | no `request.received` row exists: the guard runs before §4.5's row 1, which is *"once per request that entered the pipeline"*. `0` is never a real id (`events.event_id` starts at 1), and it is the same sentinel the parse / route / capability rejections already write |
| `identity.session` / `thread_id` / `turn_index` | `null` / `null` / `0` | session resolution reads the body and the sticky projection; a refused request has neither, and its body is **never parsed** |
| `protocol.protocol_in` | the endpoint's own protocol | the route is known before the body is read |
| `protocol.protocol_out` / `translated` / `lossy` | `null` / `false` / `[]` (omitted on the wire when empty, as on every record) | no route was selected |
| `decision.provider` / `model` | `""` / `""` | no route was attempted — the empty-string convention the pre-route failure path already uses |
| `decision.requested_model` | `null` | nothing was read from the body, so the client's own string is unknown rather than invented |
| `decision.selection_source` | `"explicit"` | the field is non-optional and this is the default word the pre-route path writes. **It names no act**: nothing was selected, and a reader must take the class from `errors[].kind` / `result.status`, never from this field |
| `decision.plugin_chain` / `decision_ms` | `[]` / `0` | no chain ran, no decision was made |
| `state.*` | `false`, `false`, `0` | v0.1's constants (above) |
| `prefix.blocks` / `continuity` | `[]` / `null` | no prefix was read or measured |
| `transform_mode` / `transforms` | `"passthrough"` / `[]` (the array is omitted on the wire when empty) | no body was composed, so no edit exists to claim (ADR-019) |
| `usage` | zeroed | an absent measurement is absent, never 0-with-a-meaning |
| `usage_missing` | **`true`** | the definition above; this is what keeps the record out of every rate, sum and gate while still counting it |
| `cost.*` (four tiers, `total`) / `quota_after` | `0` / `null` | nothing was priced and no quota was charged |
| `result.status` | `401` | the client-visible status |
| `result.upstream_status` / `failover_from` / `plan_switch` | `null` / `null` / `null` | no upstream, no abandoned attempt (fallback is about *attempts*, §4.7), and `plan_switch` stays present-and-null as always |
| `result.overhead_ms` / `upstream_ms` | measured / `null` | router's own work against no upstream latency |
| `errors[]` | one entry: `kind: "unauthorized"`, `details.header = "authorization"` \| `"x-api-key"` \| `null` | the vocabulary above; `null` means the request presented neither header |

**The body bound's refusal (§4.13) is that class with three differences, and no fourth.** `result.status` is
`413`; `errors[]` is one entry `{kind: "request_too_large", details: {limit_bytes: <the configured bound>,
content_length: <the length the request declared, or null>}}`; and `identity.session` / `thread_id` stay
`null` / `null` for the same reason the guard's do — the body is never read, so nothing about it is claimed
(`decision.requested_model` is `null` on both, and **no `details.stream` member is added**: see §4.13 on why a
fact of an unread body is not invented). Everything else in the table — `event_id: 0`, `usage_missing: true`,
zeroed usage, `0` cost, `null` `plan_switch`, `result.overhead_ms` measured against `upstream_ms: null`, the
`decision.provider` / `model` empty strings — is identical, because it follows from the same single fact: no
pipeline stage ran.

**`decision.model` vs `decision.requested_model`** (one field would lose information, so both are kept):
`model` is the id the request was actually billed under at the upstream — the resolved roster entry's own
id, which is also the string the outbound body carries (§2); `requested_model` is what the client wrote,
verbatim. An alias and a direct route that resolve to the same route therefore share `decision.model` and
differ in `requested_model` and `selection_source`. Neither field is ever re-derived from the other: the
first is read from the route, the second from the inbound bytes.

**`result.plan_switch`** (an object, or `null` when the plan policy of §4.6 did not displace the request's
account — the field is always present). Shape:
`{ from, to, reason, probe, reprefill_tokens, switch_cost_nano, cost_currency }`: `from` / `to` are
`provider/model` routes; `reason ∈ {primary_exhausted, primary_cooling_down, primary_recovered}`; `probe` is a
boolean (this switch was the return trip of an admitted probe); `reprefill_tokens` and `switch_cost_nano` are the
switch's cache price under §7's convention, and `cost_currency` is the unit `switch_cost_nano` is denominated in
— the **destination** route's currency (§4.8), present whenever `plan_switch` is not null. The two money
figures of one record are therefore each self-describing, and this is why `switch_cost_nano` needs its own key
rather than borrowing `cost.currency`: a family may span two currencies, and when the destination's attempt
fails and the chain serves a route of another currency, the switch's price and the record's own cost are in
different units. It is the plan policy's own record and is **not** a second name for `failover_from` —
the two answer different questions and are set by different facts:

| what moved the request | `failover_from` | `plan_switch` |
|---|---|---|
| a failed attempt in this request (§4.2's walk) **with a candidate the walk can attempt next** | set — the route the attempt abandoned | set, `reason: primary_exhausted`, when the abandoned route was the family's `primary` |
| a failed attempt in this request and **no** candidate the walk can attempt after it (the attempt-exhausted condition, §4.2/§8) | `null` — nothing moved: the refusal carries that attempt's own evidence (`upstream_status` / `error_class`), and a displacement with no destination would be a fact that never happened (ADR-010) | `null` |
| ADR-011's cooldown refusing the resolved route **before** any attempt | set — the route the skip abandoned (ADR-011 item 4) — and a refusal does not undo it: the field is written whether the walk then serves, fails, or ends with nothing served, so a condition-N refusal whose chain opened on a cooling route carries it, on **both** media (ADR-024; `conf_42_non_primary_abandon_is_failover_only`, CONF-64) | set, `reason: primary_cooling_down`, **when the abandoned route was the family's `primary`**; `null` when it was not (the cooldown displaced the client's own choice, not the family's account) |
| the family's account state, with no failure-class fact in this request | `null` (nothing failed here — saying otherwise would make the field mean "the route changed", which is this field's job) | set, `reason: primary_exhausted` |
| the family's account state returning to `primary` | `null` | set, `reason: primary_recovered` |

**Each of the three `reason` values has a producing path.** A documented value no implementation can emit is a
defect, not a reserved word, so the producers are named here and witnessed by conformance cases (DESIGN §12.8):

| `reason` | Produced when | The state write |
|---|---|---|
| `primary_exhausted` | (i) an attempt on the family's `primary` failed with an upstream `403 quota_exhausted` — the one signal that may move the account (§4.6 rule 3); (ii) the family's account state was already `overflow` and the policy displaced the request (`spill`) | (i) writes the `plan.switched` event (the family's account moves); (ii) writes nothing — the state already is the one the request is moved to |
| `primary_cooling_down` | ADR-011's cooldown projection refused the family's `primary` route **before any attempt**, so the request was served by the family's `overflow` route while the account state stayed `primary` | writes **nothing**, and does **not** move the account: a cooldown is route availability, not a verdict on the plan (§4.6 rule 3 — only an upstream `403 quota_exhausted` may move it). The next request is decided by the state as usual, so once the cooldown expires the primary serves again with no probe needed |
| `primary_recovered` | an admitted probe on the primary succeeded (a probe, or another session's probe pulling a spilled session back) | writes the `plan.switched` event back to `primary` |

A **displacement** and a **state transition** are therefore not the same thing (DESIGN §12.10.8): the
`primary_cooling_down` row is the pair that shows it — a `plan_switch` with no `plan.switched` behind it.

`reprefill_tokens` is the session's prefix token count (the §6 block-token attribution, so it is `inferred`);
`switch_cost_nano = reprefill_tokens × p_miss(destination account)`, in the integer fixed-point unit of the
**destination** entry's currency (ADR-006 + ADR-018; `plan_switch.cost_currency` states which, §4.8), where an
**in-plan** destination's marginal miss price is 0 (spec §4.6 rule 4, DESIGN §5) — so a return trip records the
work in `reprefill_tokens` and a money cost of 0, by the same price table the whole accounting uses. Both
figures become `verified` through the switched request's own measured usage and `cost.*` fields (the in-plan
marginal price is 0, so the first post-spill request's measured total *is* the switch's verified cost); a switch
with no following request keeps the `inferred` label and says so (§7's reporting rule).

**`prefix.continuity` is a predictor; the upstream's usage is the fact.** Two different numbers describe
the same request and they are not rivals:

- `prefix.continuity` is an **inferred** figure (spec §7): it is computed locally from the block hashes
  of this request and the previous request of the same session, and its job is to *predict* whether the
  upstream's prefix cache will hit. It is a diagnostic, not a measurement of what the upstream did.
- The cache **hit rate as a fact** comes only from the upstream's own normalized `usage`:
  `usage.input_cached / usage.input_total` (§6 `cache_hit_rate` is its aggregate form), and is therefore
  **verified**. When `usage_missing: true` the ratio is **unknown** — it is not 0, and it must never be
  treated as 0 in a verdict or a report; an absent measurement is absent.
- A gate or an external saving claim adjudicates on the **verified** number only (AGENTS constraint 4);
  `prefix.continuity` falling below 1.0 is a trigger to go look, not a verdict that cache was lost.

**Definition of `prefix_blocks[]`** (the precondition for comparability; two implementations must not
each improvise):

- A block = a **structural unit** in the upstream-visible prefix region (`messages` / `input` / the
  system instruction position): one message / one tool definition / one input item. It is **not** a
  fixed-length token bucket (the structure aligns with cache breakpoints, see ADR-007).
- Each block records `tokens` (that block's token count) and `hash`; `hash` = the **first 16 hex chars**
  of `sha256(block raw bytes)`.
- That domain **does not include** router-owned fields, so "deleting router-owned fields" changes no
  block hash (§2 byte boundary).
- **Blocks are enumerated in the provider's effective prompt (template) order** — the
  system-instruction position first, then `tools`, then the `messages` / `input` items — **not** in
  body byte order. A client may serialize `input` before `tools` (the measured codex shape does);
  the provider's template places tools before the conversation, so a client's tail append in
  template order is a true tail append upstream and must measure `prefix_continuity == 1.0`. This
  enumeration order is a **measurement-definition change by user decision on 2026-09-20 (Plan A)**
  under AGENTS constraint 9 / ADR-012; the decision and its evidence are recorded under
  `autowork/progress/` (2026-09-20). The block **domain** is unchanged by it.
- The on-disk path / rollover is specified by `trace` in §4.1; a write failure does not block the request
  (§8), and records `errors[].kind = trace_write_failed`.

**Metric definitions** (exposed by `router stats`, referenced by autowork gates):

- `cache_hit_rate` = Σ`input_cached` / Σ`input_total`
- `stateful_inbound_rate` = stateful requests / total requests (used to keep confirming "whether it
  depends on stateful"). **Always 0 in v0.1**: inbound state is not detected, so no request counts as
  stateful (gap G-F)
- `prefix_continuity_p50` = median of the longest common prefix-block ratio over adjacent requests in the
  same session (**the fidelity metric**: when it drops, some transform is breaking the cache)
- `verified_savings_tokens` = counts only the transform gains with `verdict=verified`
- `overhead_ms_p99` = the p99 of (`result.overhead_ms` − `result.upstream_ms`) — **router's own overhead,
  excluding the upstream** — over the records of the window **whose `upstream_ms` is present**. A record with
  no upstream latency (`upstream_ms: null`: a boundary refusal, a pre-route rejection) carries the router's
  work but has nothing to subtract from it, so it is **excluded from the sample** rather than read as a 0 ms
  observation (the absent-measurement rule of §7, which §9.2's provenance row states for this figure)
- `unknown_outcome_requests` = the number of `upstream.submitted` events in the window with no
  `upstream.responded` for the same `request_id` — an intent with no response is a **known unknown**
  (ADR-010 item 4). It is counted and reported, and it is never priced: there is no `usage` to read and the
  accounting convention (§7) forbids inventing one

The surfaces that print these (the invocation, each figure's provenance and its §7 label) are §9.

## 7. Accounting convention (must not be ambiguous)

| Convention | Definition | Use |
|---|---|---|
| `verified` | the **measured delta** from upstream `usage`: a control turn with the transform on/off within the same session, or an attributable change in `cached_tokens` | **only it can enter a gate or be reported externally** |
| `inferred` | local tokenizer estimate, no control | only for debugging and direction judgment; must be labeled |

Reporting requirement: any statement of "how much was saved" must state the convention, the sample size
and the time window; mixing conventions counts as an error.

**A saving is a difference between two worlds, and the trace holds one of them.** This is why the
convention is binary rather than a confidence scale: the request that was served is measured by the
upstream's own `usage`, but the counterfactual — the same content served *without* the edit — does not
appear anywhere in the record. A single observation therefore measures **the request**, never the
saving, and any figure derived from it by arithmetic on local byte lengths stays `inferred` (the
dependency allowlist has no tokenizer, GAP-Q14). A `verified` figure needs the pair to exist: a control
turn with the transform on and off over the same content in the same session, read from the upstream's
`usage` — or the replay path computing the counterfactual with the same code, which is designed and
**not served** (§9.3). Three consequences follow, and all three are rules rather than advice:

- **A transform's figure starts `inferred` and stays there until a pair exists.** In a run with no
  control turn and no replay, `verified_savings_tokens` (§6) is 0, and that is the honest reading, not
  a gap in the report.
- **The ledger's arithmetic is `net = saved − added`** (ADR-019): the marker line a `tee` rule appends,
  a payload's re-encoding and any replaced text all count on the added side. A rule whose *verified*
  net is ≤ 0 is not adopted; its `inferred` net may be positive and still tells you nothing.
- **A label travels with the figure, and an absent measurement is not a zero.** A step whose delta was
  never measured is `usage_missing`-class (§6): it is counted, labelled and priced nowhere.

## 8. Degradation and error behavior

**Unified error body** (every non-2xx, including the stub endpoints; clients parse this and should not
rely on each upstream's own error shape):

```json
{"error": {"type": "quota_exceeded", "message": "monthly quota exhausted for zai/glm-5.3",
           "request_id": "req-7", "details": {"provider": "zai", "over_quota": "block"}}}
```

| `error.type` | HTTP | Trigger |
|---|---|---|
| `invalid_request` | 400 | request body unparsable / missing `model` / wrong field type; also an unusable `X-Router-Transform` value (§2.1 — a client asking for a mode that does not exist is told, never silently served the other way) |
| `request_too_large` | 413 | the inbound body exceeds `server.max_body_bytes` (§4.13) — refused at the boundary, above the pipeline: no upstream is contacted, nothing is attempted, and no store row is written |
| `auto_not_supported` | 400 | `model: auto` (v0.1, §3) |
| `capability_unsupported` | 400 | inbound protocol ∉ that provider's `supports` (an undeclared cell = 400, no "best effort" translation) |
| `stateful_unsupported` | 400 | stateful inbound and stickiness cannot keep fidelity (ADR-004; **cannot fire in v0.1** — no request is ever judged stateful, gap G-F) |
| `unauthorized` | 401 | the request carried no token, or a token that does not match the one `server.auth_token_env` names (§4.7). Decided at the boundary: no upstream is contacted, so it is **not** the start of a failover walk |
| `cost_cap_exceeded` | 403 | guard cost cap hit (including a `plan_policy.overflow_monthly_cap_usd` cap, §4.6) |
| `unknown_provider` / `unknown_model` | 404 | `provider/model` or an alias does not resolve |
| `quota_exceeded` | 429 | `quota.over_quota = block` and the allowance is exhausted, or `plan_policy.on_primary_exhausted: block` and the primary account is exhausted (§4.6) |
| `upstream_error` | 502 | an upstream attempt errored and the chain is exhausted — the **attempt-exhausted** shape: the class-based sentence with `details.upstream_status` / `details.error_class`, and **neither `stage` nor `skipped[]`** — **or** nothing in the chain may serve this request at all, in which case **nothing was attempted**: every candidate demoted, keyless or **not native for the inbound protocol** (`details.stage: "no_available_route"`, shape frozen in the clauses below) |
| `upstream_timeout` | 504 | upstream attempt timed out and the chain is exhausted |
| `not_implemented` | 501 | a capability declared in the roadmap but not implemented in this build (currently: cross-protocol **translation** cells — native passthrough of all three protocols is implemented; the cell's message names what is missing). It answers the cell the **client named**; it is not the failover walk's answer to a candidate the client did not name (§2, §4.2 — that is the `502` below) |
| `internal` | 500 | everything else |

Response headers: `X-Router-Request-Id` (always), `X-Router-Session` (when a session was resolved),
`X-Router-Lossy` (when a lossy translation happened). On the SSE path all three headers must already have
been sent before the first event.

Behavior clauses:

- inbound auth failure → `401 unauthorized` (§4.7), decided **before** the pipeline: it never enters the
  fallback chain, is never retried, never reaches an upstream and writes no state-store row — and it
  **does** write its trace line (§6). It is the one failure a client can fix without looking at a provider.
- inbound body above `server.max_body_bytes` → `413 request_too_large` (§4.13), decided at the same boundary,
  by the same rules: before the pipeline, no fallback walk, no retry, no upstream contact, no state-store row,
  and **one** trace line (§6) — a body the router will not read is refused before it is buffered, and the
  client is told which rule refused it rather than being handed the HTTP layer's own bare `413`.
- transform failure → **fall back to the original** (fail-safe), the trace records
  `errors[].kind = transform_error`, and the request is forwarded as usual.
- upstream 5xx / 429 / quota exhaustion → switch per config's `fallback` chain (§4.2; switching loses the
  cache, so `failover_from` and the resulting re-prefill cost must be recorded).
- **no candidate may serve this request at all** (every entry of the chain demoted, keyless or not native for
  the inbound protocol, §4.2) → the same `502 upstream_error`, taken **before any upstream is contacted**: the
  record is a terminal failure (`usage_missing`, nothing charged) and the client gets the shape frozen at the
  end of these clauses. What decides this branch is **whether the walk attempted anything**, not how long the
  chain was: the moment one candidate is submitted, the request is no longer "nothing may serve", and its
  refusal is the attempt-exhausted shape — the class-based sentence with `details.upstream_status` /
  `details.error_class`, and neither `stage` nor `skipped[]` — on **both** forwarding paths. A client can
  therefore read `details.stage: "no_available_route"` as a statement about its own request: *no upstream was
  contacted*.
- prefix discontinuity (detected by cache-guard) → handled per `strict_prefix`: default is warn +
  continue; strict mode rejects that transform.
- trace write failure → **does not affect the request**, records `errors[].kind = trace_write_failed`
  (a missing observation must be explicit, never silent).
- crash window (an upstream intent was recorded and no response arrived) → the request is reported as
  `unknown_outcome`, the quota is **not** charged again and no cost is invented for it; reconciliation is
  left to the operator against the provider's bill (§4.5; ADR-010 is normative).
- unknown fields: **must be passed through verbatim** (friendly to protocol evolution), and must not be
  silently dropped.

**The walk's refusal: two conditions, one shape each, on both forwarding paths.** The walk ends with nothing
served in exactly two conditions, and each has one body that both media produce (the streaming path adds only
its pre-existing `"stream": true` member to `details`; nothing else differs, so a client cannot tell which
medium produced the refusal):

- **nothing was attempted** — every candidate of the chain failed eligibility (unknown provider / no key or
  transport / wire mismatch / demoted, the resolved route included). This is the shape frozen below:
  `details.stage: "no_available_route"` with `skipped[]`, and `upstream_status` / `error_class` both **`null`**.
  The condition and the behaviour clause above are one condition, which is why this sentence is only ever
  emitted over a request no upstream was contacted for. The **record** of such a request carries no displacement
  beyond the one ADR-011's cooldown may itself have made: a cooling skip in this chain writes
  `result.failover_from` — the route it abandoned — and still no `failover.triggered` row, while a chain with no
  cooling candidate leaves both clear (ADR-024; CONF-57 (b), CONF-64);
- **an attempt was classified and nothing served after it** — the attempt-exhausted condition, the
  `upstream_error` row's first limb. The shape is the one the in-loop exhaustion sites already emit: the
  class-based sentence (`upstream error ({class}) and the fallback chain is exhausted`; the deterministic
  variant for `format_error` / `content_policy_blocked`; the connect variant when the last attempt never
  reached the upstream), `details{upstream_status, error_class}`, and **neither `stage` nor `skipped[]`**.

In both conditions the record is a terminal failure (`usage_missing`, nothing charged) and `errors[0]` carries
the same `details` object the client received.

The `no_available_route` shape, frozen:

```json
{"error": {"type": "upstream_error",
           "message": "no available route: every candidate provider is demoted, keyless or unavailable",
           "request_id": "req-9",
           "details": {"stage": "no_available_route",
                       "skipped": [{"route": "zai-plan/glm-5.3", "reason": "keyless"},
                                   {"route": "deepseek/deepseek-v4-pro", "reason": "wire_mismatch"}],
                       "upstream_status": null,
                       "error_class": null}}}
```

`skipped[]` lists **every candidate the chain offered**, in the chain's own order — the order the walk itself
iterates: the resolved route, the family's `overflow` where §4.6's policy inserts it, then `fallback` in config
order (§4.2) — each with exactly one of
`unknown_provider` / `keyless` / `wire_mismatch` / `demoted` — one entry per candidate, so a second model on a
provider the walk already refused as keyless is listed with the same reason as the first, and in this shape
`|skipped[]|` equals the number of candidates the chain offered (nothing was attempted, so "every candidate the
walk refused without attempting" **is** "every candidate"). **The array and its order are a property of the
chain, not of the medium**: for one chain and one request the two forwarding paths produce the same `skipped[]`
element for element, so the only difference between their bodies is the streaming path's `"stream": true`
(ADR-024 — a path that collects its skips in two passes must still place each entry at its chain position, and
an entry appended when the walk reaches its candidate is in the wrong one). A candidate that *was* attempted
appears in no `skipped[]` — such a request's refusal is the attempt-exhausted shape, and `skipped[]` is a
member of this one only. The message is frozen verbatim — "unavailable" is its umbrella for the wire reason,
and the machine-readable truth is `skipped[]`, not the sentence. A candidate skipped on the wire writes **no**
`failover_from`, no `plan_switch`, no event row and no `errors[]` member: nothing failed and nothing moved, so
no switch may be narrated (§6) — and the walk never narrates a displacement **onto** a candidate it cannot
serve either, so the `failover.triggered` event is written only when the request moves to a candidate the walk
will actually attempt (§4.2). `failover_from` has exactly two producers, and both name a fact of the request
rather than a property of its outcome (§6's table): a failed attempt the walk moves on from, and ADR-011's
pre-attempt cooldown skip — which writes it **whether or not** anything was attempted after it, so a refusal
whose chain opened on a cooling route carries that route's name while one whose chain offered no cooling route
carries `null` (ADR-024). A refusal writes no `failover.triggered` row in either case.

## 9. Reporting surfaces (the operator's read-out)

Two surfaces read the records of §4.5 and §6 back out: the `plan` section of `GET /health`, and the `router
stats` subcommand. Both are **read-only**; neither is a second source of truth, and neither prices, estimates
or extrapolates anything. The metric *formulas* stay in §6 ("Metric definitions"); this section says what is
printed, from which record, and under which §7 label — it defines no new metric and no new gate.

### 9.1 `GET /health`'s `config`, `auth` and `plan` members

`/health` reports what this process loaded (§4.5, `store`). The members below carry what an operator
reasons about most: **`config`** (the configuration that was loaded — §4.14), **`auth`** (always present,
§4.7) and **`plan`** (present when the loaded config declares a `plan_policy`, §4.6).

**`config`** — which configuration this process loaded. It carries the two files and the digests of §4.14, so
"which revision is this process serving?" is answered by the surface rather than by the process's argv:

```json
"config": {
  "root_path": "/Users/me/.config/router/config.yaml",
  "roster_path": "/Users/me/.config/router/providers.yaml",
  "root_sha16": "<16 hex chars>",
  "roster_sha16": "<16 hex chars>",
  "config_digest": "<16 hex chars>"
}
```

| Key | Type | Semantics |
|---|---|---|
| `root_path` | string | the absolute path of the config file this process read — the one §4.12's order selected, resolved the way `trace_dir` and `state_db` already are |
| `roster_path` | string \| null | the **resolved** absolute path of the roster file when the root names one (§4.14); **`null` when the roster is inline** |
| `root_sha16` | string | the first 16 hex chars of `sha256` over the root file's bytes |
| `roster_sha16` | string | the same over the roster file's bytes; **the empty string when the roster is inline** — the value §4.14's recipe hashes, so an outsider recomputing the digest from the two files and one reading this member compute the same bytes |
| `config_digest` | string | `sha16(root_sha16 + ":" + roster_sha16)` — the same value the trace carries (§6) and the `config.applied` event is written with; **never the empty string** (a served process always loaded a configuration, and the writer side of the same invariant is §6's — the paragraph below) |

Two spellings of one fact, stated so the member is not read ambiguously: `roster_path: null` means **there is
no second file**, and the empty string is what that same situation contributes to the digest — a path that
does not exist is `null`, and a hash input that is not there is the empty string, each in its own type. The
member is **not** a request fact: it is part of "/health reports what was loaded", it states what the process
read at startup, and it changes only when the config does.

**The member is never the empty string, over either shape — and the writer side is the same rule.** A served
process loaded a configuration, so it always has an identity to report: `config_digest` is **non-empty** on
every boot, and so is `root_sha16`. `roster_sha16` is the one member that may legitimately be `""` — the
roster is inline, there is no second file to hash — and the composed `config_digest` is non-empty even then,
because the recipe hashes the root's half beside it (§4.14). The identical value is written into
`config.applied` and carried by every trace record (§6), and §6 states the rule that keeps the three surfaces
in step from the writing side: a writer wired into the serving path that cannot produce a non-empty digest
**fails loudly** rather than stamping a record with an empty identity. Read from the other direction, a record
that carries no digest — or the empty string — is taken as *"the configuration is not recorded for this
request"*: that is the vintage reading the field's additive rule gives it, and it is why an empty value is
never evidence that a process served with no configuration.

*Shipped, and measurable.* A running binary reports this member. Over the shipped pair `/health` answers all
five keys — `roster_path` the resolved roster, the two `sha16` halves, and the `config_digest` built from
them — and over an inline root it answers `roster_path: null` with `roster_sha16` the empty string, so both
spellings above are observable on a live boot. The same digest is written into the `config.applied` event at
startup and carried by every trace record (§6), which is what makes "which revision is this process serving?"
answerable from the records as well as from this member.

**`auth`** — whether the process demands a token, and which environment variable holds it. The token
value itself is a secret and is never here:

```json
"auth": { "required": true, "env": "ROUTER_TOKEN" }
```

| Key | Type | Semantics |
|---|---|---|
| `required` | bool | whether `server.auth_token_env` is set (§4.7). `false` ⇒ inbound auth is off and the three protocol endpoints accept requests with no token at all |
| `env` | string | **present only when `required` is true** — the *name* of the environment variable holding the expected token, never its value (§4.7) |

`required: false` ⇒ **no other key is present** (`"auth": { "required": false }`): the same stance as
`plan.configured: false` below — with no key configured there is no variable to name, and naming one would
be the invented-state error in reverse. The member is **not** a record of what happened (a `401` is
observable in the trace, §6): it is part of "/health reports what was loaded", and it changes only when the
config does.

**`plan`** — when the loaded config declares a `plan_policy` (§4.6), the response carries one further
member:

```json
"plan": {
  "configured": true,
  "family": "glm-5.3",
  "primary": "zai-plan/glm-5.3",
  "overflow": "zai/glm-5.3",
  "recover": "probe",
  "account": "primary",
  "since": null,
  "probe": null
}
```

and while the family is on the metered account:

```json
"plan": {
  "configured": true,
  "family": "glm-5.3",
  "primary": "zai-plan/glm-5.3",
  "overflow": "zai/glm-5.3",
  "recover": "probe",
  "account": "overflow",
  "since": "2026-09-20T08:14:02.113Z",
  "probe": { "deadline": "2026-09-20T08:29:02.113Z", "admitted": false, "blocked_by": "window_not_reset" }
}
```

| Key | Type | Semantics |
|---|---|---|
| `configured` | bool | whether the loaded config declares a `plan_policy` at all. `false` ⇒ **no other key is present** (`"plan": {"configured": false}`): with no policy there is no family to name, and inventing one would be the "a state nobody can see" error in reverse |
| `family` | string | the policy's `family` — the family tag both routes' model entries carry (§4.8; for a same-id pair it is that id) |
| `primary` / `overflow` | string | the policy's two routes, verbatim, in the `provider/model` wire form `failover_from` and `plan_switch` use |
| `recover` | `probe` \| `none` | the policy's value. Printed because it decides whether a probe exists at all (§4.6) |
| `account` | `primary` \| `overflow` | the family's current account state. Read from the `plan_state` projection; **an absent row means `primary`** (a family that has never switched, §4.6) |
| `since` | string \| null | the last transition's instant (RFC3339 UTC, millisecond precision) — the `ts` of the `plan.switched` row that produced the current state. `null` for a family that never switched |
| `probe` | object \| null | **`null` unless `account` is `overflow`**: a family on its primary has nothing to probe back to (§4.6 rule 2 — the probe *is* the way back from `overflow`) |
| `probe.deadline` | string | `since + cooldown`, with `cooldown` from the **currently loaded config** — the same recomputation the serving path does (ADR-014 item 10's mid-flight clause), so a knob change moves a future deadline rather than rewriting history. It is not the stored informational column, which a rebuild may have derived from a cooldown that has since changed. **Always computable when `probe` is present** |
| `probe.admitted` | bool | whether a probe would be admitted **right now, given the current state and config** — judged for a session boundary, the only place a probe is admitted |
| `probe.blocked_by` | string \| null | the **first** failing condition, in the guard's own evaluation order with the two request-shaped arms left out (§4.6 rule 2), one stable word: `recovery_disabled` → `cooldown` → `primary_cooling_down` → `window_not_reset`; `null` when `admitted` is true |

The situations in which a probe is **not** admitted, and where each appears in the response:

| Situation | In the response |
|---|---|
| the config declares no `plan_policy` | `"plan": {"configured": false}` — no family, no account, no deadline |
| `recover: none` (no automatic probe, ever) | `probe.blocked_by: "recovery_disabled"` |
| `now < deadline` | `probe.blocked_by: "cooldown"` |
| ADR-011's cooldown refuses the primary route | `probe.blocked_by: "primary_cooling_down"` |
| the plan's declared window has not reset (the local counter's only influence, §4.6 rule 3) | `probe.blocked_by: "window_not_reset"` |
| the request has **no session**, or is not a session's first request | **nothing.** `/health` reports state; "this request carries no session" is a property of a request that does not exist yet, so the response carries no word for it and must not imply that a sessionless request would probe — for every such request the claim would be false (§4.6 rule 2) |

**No figure in this section is an estimate** (AGENTS constraint 5): every key is a read (the config, the
`plan_state` row) or an arithmetic on two known instants (`since + cooldown`). There is no case in which
`deadline` is unknown, and no inferred number is printed here.

### 9.2 `router stats`

```
router stats [--config <config path>] --window <duration> [--json]
```

- **`--config`** names the config file; absent, it is found by **§4.12's** discovery order (as it is for `serve`).
  It resolves `trace.dir` by §4.1's rule (a relative path is resolved against the config file's directory). There
  is no way to point the command at a trace directory that the config does not describe.
  When the loaded config declares a `plan_policy`, it also names the family the report's `plan family` section
  describes; with no policy the report has **no** plan section — a family nobody configured is not reported,
  never fabricated.
- **`--window`** is **required** and uses the config duration grammar (`300ms`, `90s`, `15m`, `1h30m`; there is
  no `d` unit — a day is `24h`). A default would let a report be printed without stating the window it covers,
  which is exactly what §7's reporting requirement forbids.
- The window is `[now - window, now]` in UTC. A trace file is read when its hourly range (§4.1) intersects the
  window; a record is counted when **its own `ts`** falls inside it — never by which file it happens to be in.
- **Read-only by construction.** `stats` never writes and never migrates: it reads the trace files directly and
  opens the state store **read-only** (so it cannot create a state file, and it does not take the writer role
  that `serve` holds — a read-only connection is not the exclusive writer, §4.5). When the store cannot be opened
  read-only (no state directory yet, an unreadable file, a schema newer than this binary), the one figure that
  only the log holds — `unknown outcome requests` — is **omitted** with a one-line note on stderr, and the rest
  of the report is printed unchanged.
- **Exit codes**: `0` a report was produced; `2` the invocation itself is unusable (unreadable or invalid
  config, an unparsable window, a trace directory that does not exist). A partial report is never presented as
  a complete one.

stdout, exact shape (the values are illustrative; every figure carries its §7 label, and every **money** figure
carries its currency — §4.8):

```
window:      24h (2026-09-19T12:00:00.000Z .. 2026-09-20T12:00:00.000Z)
trace:       /home/u/router/state/traces (5 files, 61 records)
currencies:  USD                  (money is reported per currency and never summed across them)

requests                         61
  succeeded                      58
  failed                          3   (upstream_error 2, upstream_timeout 1)
  usage missing                   2   (excluded from every rate and every sum below — never read as 0)

cost (verified, USD)       184000 nano
  input_miss 160000 | input_hit 4000 | cache_write 0 | output 20000

cache
  hit rate (verified)        0.9898   (17536 / 17716 input tokens)
  continuity p50 (inferred)   1.000   (a predictor, not a measurement of what the provider did)

transforms
  verified savings tokens         0
  inferred savings tokens         0   (labeled; never counted with the line above)

plan family 'glm-5.3'
  switches                        1   (requests whose result.plan_switch is present)
  switch cost (verified, USD)     184000 nano
  switch re-prefill (inferred)  10000 tokens | 20000 nano | USD
  switches without usage          0   (they keep the inferred label and say so)

state
  stateful inbound rate       0.0000   (always 0 in v0.1 — gap G-F; printed so the constant is visible)
  unknown outcome requests         0   (event log)
overhead p99                     6 ms
```

When the window holds more than one currency, the same lines appear once per currency, each labelled, and the
header names both — the report states every figure it has and deliberately states no combined money total:

```
currencies:  USD, CNY             (money is reported per currency and never summed across them)
...
cost (verified, USD)       184000 nano
  input_miss 160000 | input_hit 4000 | cache_write 0 | output 20000
cost (verified, CNY)         8400 nano
  input_miss 8000 | input_hit 400 | cache_write 0 | output 0
```

The counts stay single (`requests`, `switches`, `usage missing`) because they are not money: a request is a
request whichever account served it. A mixed-currency window is **not** an error — exit code `0`, a report was
produced — and `--json` marks the case by **omitting** the single-currency scalar keys rather than adding them
up (below).

Provenance of every figure:

| Line in the report | Value | Source (which record) | §7 label | Excluded |
|---|---|---|---|---|
| `requests` | count | one per trace line whose `ts` is in the window | count | — |
| `succeeded` / `failed` | count by `result.status` (2xx / non-2xx), split by `errors[].kind` | trace `result.status`, `errors[]` | count | — |
| `usage missing` | count of records with `usage_missing: true` | trace `usage_missing` | count | those records contribute to nothing else in the report |
| `cost` and its four tiers | Σ per tier, **per currency** | trace `cost.*`, **grouped by `cost.currency`** (§4.8) — computed by the serving path from measured `usage` priced by the config table | **verified** | records with `usage_missing: true`; an uncomputed cost is never 0; a currency's figures never enter another currency's line |
| `hit rate` | §6's `cache_hit_rate` | trace `usage` | **verified** | records with `usage_missing: true` |
| `continuity p50` | §6's `prefix_continuity_p50` | trace `prefix.continuity` | **inferred** | records whose `continuity` is null (a session's first request — an absent measurement is absent, not 1.0) |
| `verified savings tokens` | §6's `verified_savings_tokens` | trace `transforms[]` where `verdict == verified` | **verified** | every `inferred` record |
| `inferred savings tokens` | Σ saved tokens over `verdict == inferred` | trace `transforms[]` | **inferred** | never added to the line above |
| `switches` | count of records with `result.plan_switch` present | trace `result.plan_switch` | count | — |
| `switch cost (verified, <currency>)` | Σ, over those records, of the **record's own `cost.total`**, per currency | the trace, as above | **verified** | records with `usage_missing: true`, counted on the next line instead |
| `switch re-prefill (inferred)` | Σ `reprefill_tokens` and Σ `switch_cost_nano` per currency | trace `result.plan_switch.reprefill_tokens` / `.switch_cost_nano`, grouped by `.cost_currency` | **inferred** | never added into `switch cost (verified)` — see below; a currency's figures never enter another's line |
| `switches without usage` | count of displaced records excluded from `switch cost (verified)` | trace | count | — |
| `stateful inbound rate` | §6's `stateful_inbound_rate` | trace `state.stateful_inbound` | count | always 0 in v0.1 (gap G-F), which is why it is printed at all |
| `unknown outcome requests` | §6's `unknown_outcome_requests` | the **event log**, read-only (§9.2 above) | count | nothing — the ambiguity *is* the number |
| `overhead p99` | §6's `overhead_ms_p99` | trace `result.overhead_ms` **− `result.upstream_ms`** (the derivation §6 defines) | measured | records whose `upstream_ms` is `null` — §6's definition excludes them from the sample, so an unreadable denominator is never read as 0 ms |

Four conventions this report obeys, each of which has bitten someone:

- **A money line is printed once per currency, and never summed across them** (§4.8). The four tiers, the total,
  the plan section's two money lines: each is reported per currency present in the window, the header names the
  currencies it saw, and there is no combined figure — not even when a reader would find one convenient. Counts
  (`requests`, `switches`, `usage missing`, `switches without usage`, `unknown outcome requests`, `stateful
  inbound rate`) and ratios (`hit rate`, `continuity p50`) are currency-free and stay single. In `--json` the
  same rule is expressed by **omission**: with one currency present the report carries today's scalar keys plus a
  `"currency"` string; with several, the scalar keys are **absent** and the figures live under a per-currency
  map, so a consumer that assumes one total fails loudly instead of adding silently. A record whose
  `cost.currency` is absent is a v1 record and is USD (§6), which is why an old trace file and a new one can sit
  in one window.

- **`switch_cost_nano` is the *inferred* column, never the verified one.** §6 fixes what verifies a switch: the
  switched request's own measured usage and `cost.*`, whose measured `cost.total` **is** the switch's verified
  cost (the destination account's re-prefill is inside it). Adding the decision-time `switch_cost_nano` on top
  would count the same money twice — once as an estimate, once as a measurement. A switch whose destination is
  in-plan verifies at 0 by the same rule (§4.6 rule 4: an in-plan destination's marginal price is 0), so one sum
  over the displaced records' `cost.total` is right in both directions.
- **What is not counted.** In-plan requests are 0 in every cost bucket (§4.6 rule 4) and therefore contribute 0:
  the difference between "the plan's marginal price" and "the metered account's real price" shows up as the plan
  section's two lines, not as a correction to the total. A `usage_missing` record contributes to nothing — an
  absent measurement is never read as 0 (§6, §7). An `unknown_outcome` request is counted and never priced
  (ADR-010 item 4).
- **A figure that cannot be computed is omitted, with a note — never estimated** (AGENTS constraints 4 and 5).
  The only figure that can be unavailable is `unknown outcome requests` (the read-only store open above); every
  other figure is a sum or a count over records that exist.

### 9.3 Surfaces that are **not** served in v0.1

`router replay --trace … --config …` (DESIGN §9's same-code-path replay), `router trace tail`, and
`GET /metrics` (Prometheus) are **planned, not served**:

- `router replay` and `router trace tail` are not subcommands of this binary: the CLI accepts `serve` and
  `stats`, and any other subcommand is refused by the argument parser with a usage error and a **non-zero
  exit** — never a silently ignored flag.
- `GET /metrics` is not registered: the route answers a bare `404` (an unrouted path does not go through
  §8's error body), and its metric names, labels and units are frozen by the change that implements it
  (the figures are integer fixed-point nano amounts there too — a Prometheus surface must not invent
  decimals, and a window holding two currencies would need the currency as a label on every money series,
  §4.8; that is the implementing change's ruling to make, not this section's).

The reason to name them here at all is the rule this section exists to keep: **a surface's shape is frozen by
the change that implements it, and a documented-but-unreachable surface is a defect** — the same class of defect
as a documented `plan_switch` reason no code can produce (DESIGN §12.8's case registry, R4-G2). Until they land,
the trace record of §6 **is** the interface: append-only JSONL, one decision record per request, readable with
any JSON tool. `router replay` in particular is the autowork harness's prerequisite (DESIGN §9, ADR-005 item 3)
and needs a simulation seam in the serving path plus the plugin-config surface; it is not part of v0.1's
promise, and no round has yet taken it.
