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
            auth_token_env: ROUTER_TOKEN }   # optional (§4.7): name of the env var whose value every
                                             # inbound request must present; absent ⇒ no inbound auth
session:  { key_sources: ["prompt_cache_key", "header:session-id", "header:thread-id"], ttl: 12h }
cache:    { sticky: true, breakeven: { enabled: true, min_remaining_turns: 3, safety_factor: 1.2 } }
trace:    { dir: "./state/traces", rollover: hourly }

providers:
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

### 4.0 Price convention (preventing two copies from drifting)

**This file copies no price figure** — the single source of truth for price figures is each model entry
in `config.example.yaml`, and each of them must carry `source` (official pricing page URL + fetch date).
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

### 4.3 `inject` (plugin dependency declaration)

`inject: [<service>…]` declares the service slots this plugin depends on (Cordis's coeffect declaration,
ADR-002/DESIGN §4). While unsatisfied, that fiber stays at **load-waiting** (it does not error, and it
does not affect other plugins); it goes on loading once the service is ready. Service names are
product-defined typed slot names (e.g. `cache_ledger`, `session_table`), not arbitrary strings.

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
   classified `quota_exhausted` (ADR-011) may move the account. The local `quota_counters` count may neither
   refuse a request nor force a spill on its own (GAP-Q1: its denominator may be a placeholder); it is recorded
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
  `api_key_env`, `account` and `currency` do (§4.6, §4.8): every metered vendor in `config.example.yaml`
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
| identity | `request_id`, `event_id` (the state-truth anchor, §4.5), `client` (UA-normalized), `session` (from §4 `key_sources`, preferring `prompt_cache_key`), `thread_id`, `turn_index` |
| protocol | `protocol_in`, `protocol_out`, `translated` (bool), `lossy[]` |
| decision | `provider`, `model` (the resolved **provider-native** model id, §2 — the string the upstream received), `requested_model` (the client's own `model` string, verbatim; `null` when the request carried none), `selection_source` (explicit/alias/plugin), `plugin_chain[]`, `decision_ms` |
| state | `stateful_inbound` (`store != false` or non-empty `previous_response_id`), `sticky_hit`, `cache_control_breaks` |
| prefix | `prefix_blocks[]` (token count + hash per block; **block granularity and hash definition below**), `prefix_continuity` (longest common block ratio relative to the previous request in the same session) |
| transform | `transform_mode` (`passthrough` \| `transform`; always present — the mode in effect for **this request's outbound body**, §2.1), and `transforms[]`: one entry per step **that changed the payload**, each `plugin` (the rule id), `edited_paths[]` (`{ path, bytes_in, bytes_out }`, where the two byte counts are the **payload text's** length before and after the step — the length of the spliced JSON string span differs by its escaping and is not claimed here), `added_input_tokens`, `saved_input_tokens`, `saved_output_tokens`, `cache_impact`, `verdict` (verified/inferred), `tee_id` (optional; `null` when tee is not enabled in v0.1). **Every step must report its token delta**: a step that cannot is not admissible as one |
| usage | normalized `Usage { input_total, input_cached, cache_write, output, reasoning }` |
| cost | `cost.input_miss`, `cost.input_hit`, `cost.cache_write`, `cost.output`, `cost.total`, **`cost.currency`** (§4.8: the unit every amount in this group is denominated in), `quota_after` |
| result | `status`, `upstream_status`, `failover_from`, `plan_switch` (present only when the plan policy of §4.6 displaced the request's account), `overhead_ms`, `upstream_ms` |
| failure details | `errors[]` (an array; **no failure = empty array, do not omit**), each `{ kind, message, plugin?, details? }`; `kind ∈ {transform_error, upstream_error, trace_write_failed, internal, unauthorized}` (§8 — the vocabulary is shared with §8's `error.type` table, and the two lists move together; `unauthorized` is §4.7's guard) |

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
pre-route rejection, a connect failure, and a request the inbound auth guard refused (§4.7). The flag
is what keeps such a record out of every rate, every sum and every gate — `router stats` counts it on
its own line and prices it nowhere (§9.2) — and a record carrying it is never read as 0 usage.

**`state` is written as a constant in v0.1** (known gap G-F): a record has one writer
(`Accountant::commit`) and it writes `stateful_inbound: false`, `sticky_hit: false`,
`cache_control_breaks: 0` on every request — no serving path writes any other value. The definitions above
stay the contract to land: `store` and `previous_response_id` are never read (they are forwarded as the
client's own bytes, §2), and the sticky-binding read that would give `sticky_hit` its value decides only
whether a `session.bound` event is written (§4.5). A reader must therefore not take `false` as "the client
sent no server-side state" or "no binding was found", and the `stateful_unsupported` row of §8 cannot fire.
ADR-004 item 3 is the ruling this group lands.

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

**A request refused by the inbound auth guard** (§4.7) is in a class of its own: it never entered the
pipeline, so its record carries what is observable at the boundary and invents nothing.

| Field | Value | Why |
|---|---|---|
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
| a failed attempt in this request (§4.2's walk) | set — the route the attempt abandoned | set, `reason: primary_exhausted`, when the abandoned route was the family's `primary` |
| ADR-011's cooldown refusing the resolved route **before** any attempt | set — the route the skip abandoned (ADR-011 item 4) | set, `reason: primary_cooling_down`, **when the abandoned route was the family's `primary`**; `null` when it was not (the cooldown displaced the client's own choice, not the family's account) |
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
- `overhead_ms_p99` = router's own overhead (excluding upstream)
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
| `auto_not_supported` | 400 | `model: auto` (v0.1, §3) |
| `capability_unsupported` | 400 | inbound protocol ∉ that provider's `supports` (an undeclared cell = 400, no "best effort" translation) |
| `stateful_unsupported` | 400 | stateful inbound and stickiness cannot keep fidelity (ADR-004; **cannot fire in v0.1** — no request is ever judged stateful, gap G-F) |
| `unauthorized` | 401 | the request carried no token, or a token that does not match the one `server.auth_token_env` names (§4.7). Decided at the boundary: no upstream is contacted, so it is **not** the start of a failover walk |
| `cost_cap_exceeded` | 403 | guard cost cap hit (including a `plan_policy.overflow_monthly_cap_usd` cap, §4.6) |
| `unknown_provider` / `unknown_model` | 404 | `provider/model` or an alias does not resolve |
| `quota_exceeded` | 429 | `quota.over_quota = block` and the allowance is exhausted, or `plan_policy.on_primary_exhausted: block` and the primary account is exhausted (§4.6) |
| `upstream_error` | 502 | upstream error and the fallback chain is exhausted (`details.upstream_status`) |
| `upstream_timeout` | 504 | upstream attempt timed out and the chain is exhausted |
| `not_implemented` | 501 | a capability declared in the roadmap but not implemented in this build (currently: cross-protocol **translation** cells — native passthrough of all three protocols is implemented; the cell's message names what is missing) |
| `internal` | 500 | everything else |

Response headers: `X-Router-Request-Id` (always), `X-Router-Session` (when a session was resolved),
`X-Router-Lossy` (when a lossy translation happened). On the SSE path all three headers must already have
been sent before the first event.

Behavior clauses:

- inbound auth failure → `401 unauthorized` (§4.7), decided **before** the pipeline: it never enters the
  fallback chain, is never retried, never reaches an upstream and writes no state-store row — and it
  **does** write its trace line (§6). It is the one failure a client can fix without looking at a provider.
- transform failure → **fall back to the original** (fail-safe), the trace records
  `errors[].kind = transform_error`, and the request is forwarded as usual.
- upstream 5xx / 429 / quota exhaustion → switch per config's `fallback` chain (§4.2; switching loses the
  cache, so `failover_from` and the resulting re-prefill cost must be recorded).
- prefix discontinuity (detected by cache-guard) → handled per `strict_prefix`: default is warn +
  continue; strict mode rejects that transform.
- trace write failure → **does not affect the request**, records `errors[].kind = trace_write_failed`
  (a missing observation must be explicit, never silent).
- crash window (an upstream intent was recorded and no response arrived) → the request is reported as
  `unknown_outcome`, the quota is **not** charged again and no cost is invented for it; reconciliation is
  left to the operator against the provider's bill (§4.5; ADR-010 is normative).
- unknown fields: **must be passed through verbatim** (friendly to protocol evolution), and must not be
  silently dropped.

## 9. Reporting surfaces (the operator's read-out)

Two surfaces read the records of §4.5 and §6 back out: the `plan` section of `GET /health`, and the `router
stats` subcommand. Both are **read-only**; neither is a second source of truth, and neither prices, estimates
or extrapolates anything. The metric *formulas* stay in §6 ("Metric definitions"); this section says what is
printed, from which record, and under which §7 label — it defines no new metric and no new gate.

### 9.1 `GET /health`'s `auth` and `plan` members

`/health` reports what this process loaded (§4.5, `store`). Two of its members carry what an operator
reasons about most: **`auth`** (always present, §4.7) and **`plan`** (present when the loaded config
declares a `plan_policy`, §4.6).

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
router stats --config <config path> --window <duration> [--json]
```

- **`--config`** resolves `trace.dir` by §4.1's rule (a relative path is resolved against the config file's
  directory). There is no way to point the command at a trace directory that the config does not describe.
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
| `overhead p99` | §6's `overhead_ms_p99` | trace `result.overhead_ms` | measured | `upstream_ms` (ADR-009's latency budget is router's own work) |

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
