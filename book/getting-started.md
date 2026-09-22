# Getting started

Status: outline only. Commands below are the ones the repository already documents; this
chapter does not restate config keys or price tables, it points at their definitions. The
copy-paste quick start itself — including the verified curl and codex examples — lives in the
[README](../README.md#quick-start); this chapter stays the map of how the pieces fit together.

The guided-configuration walkthrough below is written **ahead of the command it describes**, which is this
repository's own rule (the book precedes the implementation). `router setup` is **not served yet**: its contract
is frozen in [`docs/spec.md` §4.11](../docs/spec.md) and
[ADR-025](../design/decisions/ADR-025-setup-writes-by-anchored-edits-on-a-verbatim-template.md), and until it
lands the working path is the copy-and-edit step in the outline below.

Getting from a clone to a first request is four steps: build, copy the example config,
put provider keys in the environment, start the gateway — or let `router setup` do the copying and asking for
you. That command is **planned, not served yet**: until it lands, the copy-and-edit path below is the one that
works.

## Outline

- **Build** the workspace and run the test suite that guards the protocol contract.
- **Configure**: copy `config.example.yaml` to `config.yaml` and edit the roster —
  providers, models, prices, quotas, aliases. The example file is the single source of
  truth for every price number, and each entry carries a `source` URL plus capture date.
  The **planned** [`router setup`](#your-first-configuration) does the copy and asks you the handful of
  questions that are about your own deployment, leaving everything it cannot know alone — it is **not served
  yet**, so today the copy-and-edit step above is the path.
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

## Your first configuration

`router setup` is the **planned** guided path, and **not served yet**: its contract is frozen in
[`docs/spec.md` §4.11–§4.12](../docs/spec.md) and
[ADR-025](../design/decisions/ADR-025-setup-writes-by-anchored-edits-on-a-verbatim-template.md), and until it
lands the copy-and-edit step in the outline above is the path. When it lands it will bring your config file into
existence and ask you only about the things that are yours: where the file lives, the address to listen on, how a
conversation is identified and kept sticky, the **names** of the environment variables your keys live in, and —
if you run a subscription plan — which routes form that family. Everything a vendor owns is *shown* to you,
never guessed at.

```bash
# Planned (spec §4.11), not served yet — the intended interface:
router setup                    # every section, in order, most answers defaulting to what the file already says
router setup auth               # one section: server | auth | session | paths | providers | routing | plugins
router setup --print            # show every key it knows, with the value your file carries
router setup --check            # load the file and report which named environment variables are present
router setup --dry-run          # show the edits a run would make, and write nothing
router setup --non-interactive  # CI / containers: no questions, every answer at its default
```

A few things worth knowing before you run it:

- **The file comes from the shipped example.** Without `--from`, the starting point is the `config.example.yaml`
  embedded in your binary — the one of its own commit. Where the result goes is described below; the absolute
  path is always printed. If the file already exists it is the starting point instead, so your own edits are
  what the questions start from.
- **Your answers edit; they never regenerate.** The command replaces the value you answered on its own line and
  touches nothing else, so every comment, every `source` citation and every note you wrote survives verbatim.
  That is deliberate: the example's comments carry the provenance of each price, and a command that rewrote the
  file would throw the provenance away ([ADR-025](../design/decisions/ADR-025-setup-writes-by-anchored-edits-on-a-verbatim-template.md)).
- **The defaults are your file's own values.** Press Enter to keep what is there; a menu whose selected item is
  already the current value says `Skipped (keeping current)` rather than pretending you changed something. Run
  it twice and nothing moves — a second run with the same answers writes nothing at all.
- **It refuses rather than guesses.** If an answer cannot be placed exactly — the key is not in your file, or
  its value is written in a shape the wizard does not edit — it stops, names the key and writes nothing. Your
  file is never left half-edited; nothing lands unless the result still loads as a config.
- **It never touches a price, an endpoint or a citation.** Those are transcriptions of the providers' own
  pages, and they are yours to edit by hand ([`config.example.yaml`](../config.example.yaml),
  [`docs/spec.md` §4](../docs/spec.md)). `aliases`, `fallback` and `plugins` entries — anything whose edit would
  mean *adding* or *removing* a line — are shown for reference and edited in the file too.

### Where the file lands, and what is inside it

- **The default location is `${XDG_CONFIG_HOME:-$HOME/.config}/router/config.yaml`**, and `--config <path>`
  overrides it. The same order finds the file that already exists: an explicit `--config`, else the XDG
  location, else `./config.yaml` in the directory you are in, else — on a first run — the XDG location,
  created directory and all. `router serve` and `router stats` look in the same order, so once the file exists
  you stop passing a path at all ([`docs/spec.md` §4.12](../docs/spec.md)). With no `--config` and nothing to
  find, they refuse and name the setup command rather than falling back to a default silently.
- **Permissions.** A file this command creates is mode `0600` and a directory it creates is `0700` (set
  explicitly, not left to the umask; a directory that already exists is never re-moded). The config names your
  key variables and your whole roster, so it stays private — even though no key *value* is ever written into it.
- **What a path inside the file means.** Every relative path in it — `trace.dir`, a plugin's rule file, and the
  state store — resolves against **the directory the config file itself sits in**, never the directory you
  happened to run the command from. With the default location that puts your traces and your state store under
  `~/.config/router/`, travelling with the config, which is what the backup advice in
  [Operations](operations.md#backup) assumes. To keep your traces somewhere else — a directory you already back
  up, or a drive that is not your dotfiles — write an **absolute** `trace.dir` in the `paths` section: a
  leading `~` is not expanded, so spell the path out in full.

### Keys: export them, never write them

The config names environment variables; it never carries a value
([`docs/spec.md` §4.7](../docs/spec.md)). The wizard asks for the **name** and checks whether that variable is
present in your environment — it does not read the value, print it or store it, and it does not write a `.env`.
So collect the snippet it prints for each missing name and export it in your shell:

```bash
export DEEPSEEK_API_KEY='<paste the key here>'
export ROUTER_TOKEN='<what you want your clients to send>'   # only if you turned inbound auth on
```

`router setup --check` (**planned, not served yet**) is the read-only version of that check, and it is the one a
script should call: it exits `0` when every named variable is present, and `4` when one is missing or (for
`server.auth_token_env`, which refuses the start) present but empty — see [Operations](operations.md#run-it) for
why that one is fatal.

### After it runs, check yourself

```bash
router serve                     # start it; the config is found by the same rule, and a problem is named
curl -s localhost:8790/health    # what this process actually loaded: plugins, per-provider key presence, auth
router stats --window 24h        # the figures, once requests have gone through
```

If `serve` exits instead of starting, the message names the key — most often a variable the config names that
your shell does not have. The **planned** `--check` mode above answers the same question without starting
anything.

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
    urls:
      chat: <the international chat endpoint, in full>
    api_key_env: ZAI_API_KEY
    models: [ { id: glm-5.3, ... } ]
  - name: zai-cn                    # the mainland deployment of the same vendor: its own entry
    region: cn
    currency: CNY
    urls:
      chat: <the mainland chat endpoint, in full>
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
