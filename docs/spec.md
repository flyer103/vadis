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
| Server-side session state (`store:true`, `previous_response_id`) | Measured: clients do not use it (§7); stickiness keeps fidelity when it is absent |
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
server:   { addr: "127.0.0.1:8790", upstream_attempt_timeout: 60s, request_timeout: 10m }
session:  { key_sources: ["prompt_cache_key", "header:session-id", "header:thread-id"], ttl: 12h }
cache:    { sticky: true, breakeven: { enabled: true, min_remaining_turns: 3, safety_factor: 1.2 } }
trace:    { dir: "./state/traces", rollover: hourly }

providers:
  - name: deepseek
    base_url: https://api.deepseek.com/v1
    api_key_env: DEEPSEEK_API_KEY      # secrets are read from env only
    wire_api: chat                     # chat | responses | anthropic
    supports: [chat, responses]        # inbound protocols this provider can be translated to
    account: api                       # coding_plan | api (absent ⇒ api): the metered account (§4.6)
    models:
      - id: <unique model id within the provider>   # the provider-native id; this is what goes upstream (§2)
        context: <context limit>
        price:                         # five-tier price, USD / 1K token: this file fixes only the **schema and convention**, it copies no values
          input_miss: <base price: cache miss>
          input_hit: <cache-hit price>
          cache_write: <cache-write price; 0 = upstream does not charge separately>
          output: <output price>
          peak: { multiplier: 2.0, windows: [{ days: [mon,tue,wed,thu,fri], start: "01:00", end: "04:00", tz: UTC }] }
        source: "<official pricing page URL> @<fetch date>"   # required, traceable (see "Price convention" below)
  - name: coding-plan                # a second entry for the same family: the **subscription** account
    base_url: <the plan endpoint>
    api_key_env: CODING_PLAN_KEY
    wire_api: anthropic                # chat | responses | anthropic
    supports: [chat, responses, anthropic]
    account: coding_plan               # coding_plan | api (absent ⇒ api): the subscription account (§4.6)
    models:
      - id: <the model id this account serves; it must equal plan_policy.family>
        context: <context limit>
        price: { input_miss: <…>, input_hit: <…>, cache_write: <…>, output: <…>, peak: { multiplier: 2.0, windows: [<…>] } }
        source: "<official pricing page URL> @<fetch date>"
    quota:                             # subscription plans (coding plan etc.), optional
      - { models: ["<this provider's model id>"], window: monthly, tokens: <plan allowance>,
          reset_day: 1, over_quota: block }   # quota may only reference **its own provider's** models
                                              # under §4.6 the local counter is a warning, never the authority

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

fallback: [deepseek/deepseek-v4-pro, moonshot/kimi-k3]   # ordered route list, global granularity (§8)

plan_policy:                          # optional; §4.6 — the subscription first, the metered account as the spill
  family: <the model id both routes carry>
  primary: <provider>/<model>         # required; its provider must be `account: coding_plan`
  overflow: <provider>/<model>        # required; its provider must be `account: api`
  on_primary_exhausted: spill          # spill | block   (default spill)
  recover: probe                       # probe | none    (default probe)
  cooldown: 15m                        # default 15m
  overflow_monthly_cap_usd: <float>    # optional; absent = no cap
```

**The account belongs to the provider entry; the plan policy is one top-level section.** `account` says whether
a provider entry is a subscription (`coding_plan`) or the pay-per-token API (`api`, the default when the key is
absent): the key, the endpoint and the allowance belong to the account and not to a model (ADR-011 item 4), so
the flag sits on the provider. `plan_policy` names one **family** — the model id that two routes (one per
account) both serve — and is optional; its semantics, its defaults and its hard rules are §4.6.

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
- `cache_write: 0` means the upstream does not charge separately for cache writes (e.g. DeepSeek only
  distinguishes hit / miss).
- Currency is uniformly **USD / 1K token**; converting the official page's "per 1M" to 1K means
  **dividing by 1000** (an exchange-rate approximation must not stand in for the official USD price).
- Reference instance (@2026-09-19 official page): DeepSeek's peak periods are **UTC 01:00–04:00 and
  06:00–10:00 (Monday to Friday, excluding Chinese public holidays)**, off-peak is half the peak price
  ⇒ the base price takes the **off-peak price**, `peak.multiplier: 2.0`. Holidays are not modeled
  (GAP-Q6; known deviation: holidays are billed at `peak`, which is on the high side).
- A plan's `quota` **may only reference models of its own provider**: a cross-provider reference would
  have unclear semantics when that provider is unavailable.

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
comes back when the upstream allows it again (ADR-014).

**`account` (a provider-entry property).** `coding_plan` | `api`; **absent ⇒ `api`**. It states what the
provider entry *is* — the account its key, endpoint and allowance belong to. A provider that declares
`account: coding_plan` may carry a `quota` plan (spec §4) but is **not required to**: a plan whose token
allowance is not published is still a plan, and §4.6's rules must not be reachable only by operators who can
supply a number nobody has (GAP-Q1).

**`plan_policy` (one top-level section, optional).** At most one policy in v0.1; a second family is an additive
future key (the `state:` / `retention` precedent), never a reshaped section.

| Key | Type | Default | Semantics |
|---|---|---|---|
| `family` | string | required | The model id both routes carry — and the key of the state: the account state, the probe deadline and the switch records are per family. Load error unless both routes' model id equals it. |
| `primary` | `provider/model` | required | The subscription route. Load error unless it is a roster route whose provider is `account: coding_plan`, and — when that provider declares a `quota` — unless the family's model is covered by `quota.models` (§4.0's own-provider rule). |
| `overflow` | `provider/model` | required | The metered route. Load error unless it is a roster route, distinct from `primary`, whose provider is `account: api`, carrying the same model id. It need not appear in `fallback`: for a request inside a family it is the first candidate after `primary` (§4.2). |
| `on_primary_exhausted` | `spill` \| `block` | `spill` | `spill`: the family continues on `overflow` at its real price. `block`: the request is refused with a **readable** reason (§8 `quota_exceeded`, 429) instead of being served from the metered account — the mode for an operator who would rather fail than spend. |
| `recover` | `probe` \| `none` | `probe` | `probe`: after `cooldown`, the next session's first request is admitted as a probe on `primary`; success switches the family back and is recorded. `none`: no automatic probe — the family returns at the plan's own window boundary when one is declared, and otherwise only by an operator action. |
| `cooldown` | duration | `15m` | The minimum interval between the move away from `primary` and the first admitted probe. A floor, not a schedule: ADR-011's cooldown for that provider must also allow the attempt, and rule 3 below may defer the probe further. |
| `overflow_monthly_cap_usd` | f64 USD | absent = no cap | Optional guardrail on the family's **metered** spend in a UTC calendar month, compared against measured usage priced by the config table. Once the month's spend has reached it, the family's overflow requests are refused (`cost_cap_exceeded`, 403). The request that crosses the cap is served — its cost cannot be known beforehand — so the overshoot is bounded by one request. A negative value is a load error. |

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

**Relation to the Guard stage (§3).** This policy **is one guard rule** and adds no pipeline stage: the chain
already asks "may this request go on this route?", and the answer uses the existing vocabulary — `Pass` (this
route is allowed), `Downgrade(<the overflow route>)` (a different route must be taken, and it is recorded),
`Reject { code, message }` (`block`, or the overflow cap). Its rule order is fixed: **probe admission → overflow
cap → account state**. §3's sentence is unchanged ("on a guard hit it acts per the configured policy: reject and
explain why, or downgrade to the `fallback` chain"); for a request inside a family, the family's `overflow` route
is simply the first route the downgrade considers.

## 5. Onboarding prerequisite (mandatory)

The client must bypass any local system proxy, otherwise **the request does not reach router at all**:

```bash
export NO_PROXY=127.0.0.1,localhost
```

Measured (2026-09-19): the macOS system proxy is configured as `127.0.0.1:8080` with `127.0.0.1` in its
exception list, but `reqwest` (codex) does not honor the exception list, so localhost requests are sent
into the proxy all the same and the client finally reports `503 Service Unavailable`. The same holds for
`httpx` (hermes).

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
| transform | per step: `plugin`, `added_input_tokens`, `saved_input_tokens`, `saved_output_tokens`, `cache_impact`, `verdict` (verified/inferred), `tee_id` (optional; `null` when tee is not enabled in v0.1) |
| usage | normalized `Usage { input_total, input_cached, cache_write, output, reasoning }` |
| cost | `cost.input_miss`, `cost.input_hit`, `cost.cache_write`, `cost.output`, `cost.total`, `quota_after` |
| result | `status`, `upstream_status`, `failover_from`, `plan_switch` (present only when the plan policy of §4.6 displaced the request's account), `overhead_ms`, `upstream_ms` |
| failure details | `errors[]` (an array; **no failure = empty array, do not omit**), each `{ kind, message, plugin?, details? }`; `kind ∈ {transform_error, upstream_error, trace_write_failed, internal}` (§8) |

**`decision.model` vs `decision.requested_model`** (one field would lose information, so both are kept):
`model` is the id the request was actually billed under at the upstream — the resolved roster entry's own
id, which is also the string the outbound body carries (§2); `requested_model` is what the client wrote,
verbatim. An alias and a direct route that resolve to the same route therefore share `decision.model` and
differ in `requested_model` and `selection_source`. Neither field is ever re-derived from the other: the
first is read from the route, the second from the inbound bytes.

**`result.plan_switch`** (an object, or `null` when the plan policy of §4.6 did not displace the request's
account — the field is always present). Shape:
`{ from, to, reason, probe, reprefill_tokens, switch_cost_nano }`: `from` / `to` are `provider/model` routes;
`reason ∈ {primary_exhausted, primary_cooling_down, primary_recovered}`; `probe` is a boolean (this switch was
the return trip of an admitted probe); `reprefill_tokens` and `switch_cost_nano` are the switch's cache price
under §7's convention. It is the plan policy's own record and is **not** a second name for `failover_from` —
the two answer different questions and are set by different facts:

| what moved the request | `failover_from` | `plan_switch` |
|---|---|---|
| a failed attempt in this request (§4.2's walk) | set — the route the attempt abandoned | set, `reason: primary_exhausted`, when the abandoned route was the family's `primary` |
| ADR-011's cooldown refusing the resolved route **before** any attempt | set (ADR-011 item 4) | set, `reason: primary_cooling_down` |
| the family's account state, with no failure-class fact in this request | `null` (nothing failed here — saying otherwise would make the field mean "the route changed", which is this field's job) | set, `reason: primary_exhausted` |
| the family's account state returning to `primary` | `null` | set, `reason: primary_recovered` |

`reprefill_tokens` is the session's prefix token count (the §6 block-token attribution, so it is `inferred`);
`switch_cost_nano = reprefill_tokens × p_miss(destination account)`, in integer NanoUsd (ADR-006), where an
**in-plan** destination's marginal miss price is 0 (spec §4.6 rule 4, DESIGN §5) — so a return trip records the
work in `reprefill_tokens` and a money cost of 0, by the same price table the whole accounting uses. Both
figures become `verified` through the switched request's own measured usage and `cost.*` fields (the in-plan
marginal price is 0, so the first post-spill request's measured total *is* the switch's verified cost); a switch
with no following request keeps the `inferred` label and says so (§7's reporting rule).

**Definition of `prefix_blocks[]`** (the precondition for comparability; two implementations must not
each improvise):

- A block = a **structural unit** in the upstream-visible prefix region (`messages` / `input` / the
  system instruction position): one message / one tool definition / one input item. It is **not** a
  fixed-length token bucket (the structure aligns with cache breakpoints, see ADR-007).
- Each block records `tokens` (that block's token count) and `hash`; `hash` = the **first 16 hex chars**
  of `sha256(block raw bytes)`.
- That domain **does not include** router-owned fields, so "deleting router-owned fields" changes no
  block hash (§2 byte boundary).
- The on-disk path / rollover is specified by `trace` in §4.1; a write failure does not block the request
  (§8), and records `errors[].kind = trace_write_failed`.

**Metric definitions** (exposed by `router stats`, referenced by autowork gates):

- `cache_hit_rate` = Σ`input_cached` / Σ`input_total`
- `stateful_inbound_rate` = stateful requests / total requests (used to keep confirming "whether it
  depends on stateful")
- `prefix_continuity_p50` = median of the longest common prefix-block ratio over adjacent requests in the
  same session (**the fidelity metric**: when it drops, some transform is breaking the cache)
- `verified_savings_tokens` = counts only the transform gains with `verdict=verified`
- `overhead_ms_p99` = router's own overhead (excluding upstream)

## 7. Accounting convention (must not be ambiguous)

| Convention | Definition | Use |
|---|---|---|
| `verified` | the **measured delta** from upstream `usage`: a control turn with the transform on/off within the same session, or an attributable change in `cached_tokens` | **only it can enter a gate or be reported externally** |
| `inferred` | local tokenizer estimate, no control | only for debugging and direction judgment; must be labeled |

Reporting requirement: any statement of "how much was saved" must state the convention, the sample size
and the time window; mixing conventions counts as an error.

## 8. Degradation and error behavior

**Unified error body** (every non-2xx, including the stub endpoints; clients parse this and should not
rely on each upstream's own error shape):

```json
{"error": {"type": "quota_exceeded", "message": "monthly quota exhausted for zai/glm-5.3",
           "request_id": "req-7", "details": {"provider": "zai", "over_quota": "block"}}}
```

| `error.type` | HTTP | Trigger |
|---|---|---|
| `invalid_request` | 400 | request body unparsable / missing `model` / wrong field type |
| `auto_not_supported` | 400 | `model: auto` (v0.1, §3) |
| `capability_unsupported` | 400 | inbound protocol ∉ that provider's `supports` (an undeclared cell = 400, no "best effort" translation) |
| `stateful_unsupported` | 400 | stateful inbound and stickiness cannot keep fidelity (ADR-004) |
| `cost_cap_exceeded` | 403 | guard cost cap hit (including a `plan_policy.overflow_monthly_cap_usd` cap, §4.6) |
| `unknown_provider` / `unknown_model` | 404 | `provider/model` or an alias does not resolve |
| `quota_exceeded` | 429 | `quota.over_quota = block` and the allowance is exhausted, or `plan_policy.on_primary_exhausted: block` and the primary account is exhausted (§4.6) |
| `upstream_error` | 502 | upstream error and the fallback chain is exhausted (`details.upstream_status`) |
| `upstream_timeout` | 504 | upstream attempt timed out and the chain is exhausted |
| `not_implemented` | 501 | the v0.1 stubs of the three protocol endpoints (forwarding lands in Round 2) |
| `internal` | 500 | everything else |

Response headers: `X-Router-Request-Id` (always), `X-Router-Session` (when a session was resolved),
`X-Router-Lossy` (when a lossy translation happened). On the SSE path all three headers must already have
been sent before the first event.

Behavior clauses:

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
