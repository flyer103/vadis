# Getting started

Status: outline only. Commands below are the ones the repository already documents; this
chapter does not restate config keys or price tables, it points at their definitions. The
copy-paste quick start itself — including the verified curl and codex examples — lives in the
[README](../README.md#quick-start); this chapter stays the map of how the pieces fit together.

Getting from a clone to a first request is four steps: build, copy the example config,
put provider keys in the environment, start the gateway.

## Outline

- **Build** the workspace and run the test suite that guards the protocol contract.
- **Configure**: copy `config.example.yaml` to `config.yaml` and edit the roster —
  providers, models, prices, quotas, aliases. The example file is the single source of
  truth for every price number, and each entry carries a `source` URL plus capture date.
- **Secrets stay in the environment**: the config references environment variable names
  only; API keys are never written into YAML. That covers the inbound token too: the key
  `server.auth_token_env` names a variable, and its value is never in the file
  ([`docs/spec.md` §4.7](../docs/spec.md)).
- **Start** `router serve --config config.yaml` and confirm liveness on `GET /health` —
  which answers without a token even when inbound auth is on, and reports which auth mode
  the process is in (`"auth":{"required":true,"env":"ROUTER_TOKEN"}` when the key is set).
  One refusal to know about before it surprises you: with `auth_token_env` naming a
  variable that is unset or empty, `router serve` exits with code 4 instead of starting
  unauthenticated.
- **Point a client** at the gateway base URL — but do the local-proxy prerequisite from
  [Connecting clients](connecting-clients.md) first, or nothing will reach router. If you
  turned inbound auth on, that chapter also has the token step.
- **First request**: send one request in any of the three inbound protocols, natively —
  v0.1 serves only routes whose inbound protocol equals the provider's own `wire_api`
  (a mismatch answers `501 not_implemented`; the README's curl example explains the fork
  for the stock roster). The `router_meta` response block (plugin chain, per-transform
  accounting, session and cache state) is planned design intent, not served in v0.1 —
  today those same facts are read from the trace record instead.
- **Know where state lands**: traces are appended under the `trace.dir` configured for
  the run, and the gateway's own state (session stickiness, cache ledger, quota counters) is
  a local store beside the config file, so the two can be backed up together
  ([`docs/spec.md` §4.5](../docs/spec.md)). Request and response bodies are written to
  neither.

## Two deployments of one vendor, and what currency means

A vendor often runs more than one deployment: a China-mainland one and an international one. They are
**two different endpoints with two different keys and two different price lists**, and the same model very
often does **not** carry the same id on both. Router expresses that with three small keys — and with nothing
else, because none of them changes how a request is routed.

```yaml
providers:
  - name: zai                       # international: one deployment, one entry
    region: intl                    # which deployment this entry is (the default when the key is absent)
    currency: USD                   # the unit every price below is written in (the default when absent)
    base_url: <the international endpoint>
    api_key_env: ZAI_API_KEY
    models: [ { id: glm-5.3, ... } ]
  - name: zai-cn                    # the mainland deployment of the same vendor: its own entry
    region: cn
    currency: CNY
    base_url: <the mainland endpoint>
    api_key_env: ZAI_CN_API_KEY
    models: [ { id: glm-5.3, ... } ]
```

- **`region` is a declaration, not a switch.** A client still asks for `provider/model`; the deployment
  behind that name is what the entry is. It exists so that you — and `GET /health` — can see which
  deployment a request will actually spend on, and the router itself does not check it against the URL,
  because only the vendor knows its own host list.
- **`currency` says what a price is, and the unit never changes.** A mainland entry's prices are
  transcribed from that deployment's own official page **in that page's currency** and stay there. Router
  applies no exchange rate anywhere: it does not convert a price, a cost, or a report total. That is not a
  missing feature — a converted number is a number nobody published, and the whole cost report exists to be
  recomputable from the config and the trace.
- **The names are yours.** The convention this repository uses is `<vendor>[-<region>][-<account>]`
  (`kimi`, `kimi-cn`, `kimi-plan`, `kimi-cn-plan`), with the vendor part matching the name its own API-key
  variable uses, so `/health` does not show a name that contradicts its key.
- **A plan must use the plan's own endpoint.** Vendors say so explicitly for coding plans, and a plan
  endpoint is usually a *different* base URL from the same vendor's metered API (sometimes a different
  host entirely). Copy the plan's base URL from the plan's own documentation, not from the metered entry.
- **A plan and a metered account can serve one model under two different ids.** Pair them with a **family
  tag**: give both model entries the same `family` string and name that string in `plan_policy.family`. Each
  route still sends the id its own provider expects, so the tag changes what the router prefers, never what
  goes on the wire ([`docs/spec.md` §4.8](../docs/spec.md)).
- **Check the plan's terms before pointing it at a gateway.** Not every vendor allows a subscription key to
  be used through your own tool: at least one mainland coding plan documents that its allowance is only
  usable inside the vendor's supported coding tools, while another documents handing the subscription's key
  to third-party tools. The router cannot know which case you are in — it is in your plan's terms, and the
  ADR that records both cases is
  [ADR-018](../design/decisions/ADR-018-currency-region-and-family-mapping.md).

- **The names are yours.** The convention this repository uses is `<vendor>[-<region>][<account>]`
  (`kimi`, `kimi-cn`, `kimi-plan`, `kimi-cn-plan`), with the vendor part matching the name its own API-key
  variable uses, so `/health` does not show a name that contradicts its key.

## Authoritative sources

- [`README.md`](../README.md) — quick start (the verified copy-paste path), CLI surface,
  endpoint table.
- [`config.example.yaml`](../config.example.yaml) — the roster, the price schema and its
  per-model `source` provenance.
- [`docs/spec.md` §4](../docs/spec.md) — the config schema as a contract, §4.1 for the
  trace output parameters, §4.7 for the inbound auth key.
- [`docs/spec.md` §5](../docs/spec.md) — the client-side prerequisite (also covered in
  the next chapter).
- [`AGENTS.md`](../AGENTS.md) — the build/test commands and the environment gotchas.
