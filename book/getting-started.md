# Getting started

Status: outline only. Commands below are the ones the repository already documents; this
chapter does not restate config keys or price tables, it points at their definitions. The
copy-paste quick start itself — including the verified curl and codex examples — lives in the
[README](../README.md#quick-start); this chapter stays the map of how the pieces fit together.

The guided-configuration walkthrough below is the served `vadis setup` command (contract in
[`docs/spec.md` §4.11](../docs/spec.md) and
[ADR-025](../design/decisions/ADR-025-setup-writes-by-anchored-edits-on-a-verbatim-template.md)).

Getting from a clone to a first request is four steps: build, copy the example config,
put provider keys in the environment, start the gateway — or let `vadis setup` do the copying and asking for
you.

## Outline

- **Build** the workspace and run the test suite that guards the protocol contract.
- **Configure**: copy `config.example.yaml` to `config.yaml`, keep `providers.example.yaml` beside it,
  and edit the roster — providers, models, prices, quotas, aliases. The example pair is the single source of
  truth for every price number, and each entry carries a `source` URL plus capture date.
  The served [`vadis setup`](#your-first-configuration) does the copy and asks you the handful of
  questions that are about your own deployment, leaving everything it cannot know alone.
  The roster lives in **a file of its own**, which the config names with `providers_file:`
  ([`docs/spec.md` §4.14](../docs/spec.md)) — the shipped example is split this way (the server's settings in
  `config.example.yaml`, the roster in `providers.example.yaml`), and carrying the roster inline instead is
  equally legal: exactly one of the two shapes is written.
- **Secrets stay in the environment**: the config references environment variable names
  only; API keys are never written into YAML. That covers the inbound token too: the key
  `server.auth_token_env` names a variable, and its value is never in the file
  ([`docs/spec.md` §4.7](../docs/spec.md)).
- **Start** `vadis serve --config config.yaml` and confirm liveness on `GET /health` —
  which answers without a token even when inbound auth is on, and reports which auth mode
  the process is in (`"auth":{"required":true,"env":"VADIS_TOKEN"}` when the key is set).
  One refusal to know about before it surprises you: with `auth_token_env` naming a
  variable that is unset or empty, `vadis serve` exits with code 4 instead of starting
  unauthenticated.
- **Point a client** at the gateway base URL — but do the local-proxy prerequisite from
  [Connecting clients](connecting-clients.md) first, or nothing will reach vadis. If you
  turned inbound auth on, that chapter also has the token step.
- **First request**: send one request in any of the three inbound protocols. A route is
  served natively on every protocol its provider entry declares in `supports` — the client's
  own bytes, posted to that cell's own URL — and a protocol the entry does not declare
  answers `400 capability_unsupported` (the README's curl example shows the stock roster's
  `deepseek` entry, which declares all three). The `vadis_meta` response block (plugin chain, per-transform
  accounting, session and cache state) is planned design intent, not served in v0.1 —
  today those same facts are read from the trace record instead.
- **Know where state lands**: traces are appended under the `trace.dir` configured for
  the run, and the gateway's own state (session stickiness, cache ledger, quota counters) is
  a local store beside the config file, so the two can be backed up together
  ([`docs/spec.md` §4.5](../docs/spec.md)). Request and response bodies are written to
  neither.

## Your first configuration

`vadis setup` is the guided path, served since this landed: its contract is frozen in
[`docs/spec.md` §4.11–§4.12](../docs/spec.md) and
[ADR-025](../design/decisions/ADR-025-setup-writes-by-anchored-edits-on-a-verbatim-template.md). It brings your config file into
existence and asks you only about the things that are yours: where the file lives, the address to listen on, how a
conversation is identified and kept sticky, the **names** of the environment variables your keys live in, and —
if you run a subscription plan — which routes form that family. Everything a vendor owns is *shown* to you,
never guessed at.

```bash
vadis setup                    # every section, in order, most answers defaulting to what the file already says
vadis setup auth               # one section: server | auth | session | paths | providers | routing | plugins
vadis setup --print            # show every key it knows, with the value your file carries
vadis setup --check            # load the file and report which named environment variables are present
vadis setup --dry-run          # show the edits a run would make, and write nothing
vadis setup --non-interactive  # CI / containers: no questions, every answer at its default
```

A few things worth knowing before you run it:

- **The files come from the shipped example, and what you get is always the working set.** Without `--from`, the
  starting point is the `config.example.yaml`
  embedded in your binary — the one of its own commit — together with the embedded `providers.example.yaml` it
  names: a fresh run writes **both**. It also writes the **third** file that same config names — the rule file
  the `plugins` entry points at (`./rules/tool_output.toml`), copied from the rule file embedded in your binary
  — so a fresh install's transform engine finds its rules instead of starting with a dangling reference
  ([ADR-046](../design/decisions/ADR-046-setup-lands-the-rule-file.md); [`docs/spec.md` §4.11](../docs/spec.md)).
  Where the result goes is described below; the absolute
  path is always printed. If the file already exists it is the starting point
  instead, so your own edits are what the questions start from — and if that file carries the roster
  **inline** (`providers:` in the config itself, the shape every config written before the example split
  has), the run **moves** it into `providers.example.yaml` for you instead of leaving you a two-shape
  config ([ADR-038](../design/decisions/ADR-038-setup-writes-the-pair.md)).
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
  pages, and they are yours to edit by hand ([`providers.example.yaml`](../providers.example.yaml),
  [`docs/spec.md` §4](../docs/spec.md)). `aliases`, `fallback` and `plugins` entries — anything whose edit would
  mean *adding* or *removing* a line — are shown for reference and edited in the file too.
- **Holding more than one plan, or several keys for one provider?** That is the fan-out: every coding-plan
  account of a family is drained before anything is metered, a provider's keys rotate when one is refused,
  and the metered accounts can be ranked by their own published prices. It is configured by the **tag** your
  roster already carries — not by a new file — and the whole story is
  [Cost and caching § Several keys, several plans](cost-and-caching.md#several-keys-several-plans-drain-them-all-then-spend).

### Where the file lands, and what is inside it

- **The default location is `${XDG_CONFIG_HOME:-$HOME/.config}/vadis/config.yaml`**, and `--config <path>`
  overrides it. The same order finds the file that already exists: an explicit `--config`, else the XDG
  location, else `./config.yaml` in the directory you are in, else — on a first run — the XDG location,
  created directory and all. `vadis serve` and `vadis stats` look in the same order, so once the file exists
  you stop passing a path at all ([`docs/spec.md` §4.12](../docs/spec.md)). With no `--config` and nothing to
  find, they refuse and name the setup command rather than falling back to a default silently.
- **Permissions.** A file this command creates is mode `0600` and a directory it creates is `0700` (set
  explicitly, not left to the umask; a directory that already exists is never re-moded). The config names your
  key variables and your whole roster, so it stays private — even though no key *value* is ever written into it.
- **What a path inside the file means.** Every relative path in it — `trace.dir`, a plugin's rule file, and the
  state store — resolves against **the directory the config file itself sits in**, never the directory you
  happened to run the command from. With the default location that puts your traces and your state store under
  `~/.config/vadis/`, travelling with the config, which is what the backup advice in
  [Operations](operations.md#backup) assumes. To keep your traces somewhere else — a directory you already back
  up, or a drive that is not your dotfiles — write an **absolute** `trace.dir` in the `paths` section: a
  leading `~` is not expanded, so spell the path out in full.
- **Where the rule file lives — written for you, yours afterwards.** The `plugins` entry names its rule file
  (`plugins[].config.rules_file`), and `vadis setup` writes it beside the config, resolved against the config
  file's own directory like every other path above, from the rule file embedded in the binary: **created** when
  it is not there, left **untouched** when it is, and replaced only by `--force` with the `plugins` section
  selected — the previous bytes kept at `<file>.bak` first — so a reconfigure never discards rules you wrote.
  The directory it creates is mode `0700` and the file it creates `0600`; nothing *inside* the file is ever
  edited by the wizard, because a rule's own knobs are yours
  ([ADR-046](../design/decisions/ADR-046-setup-lands-the-rule-file.md);
  [`docs/spec.md` §4.11](../docs/spec.md)). Without this the file the shipped config names would not exist on a
  fresh install, and a request that asks for transform mode would run with an empty ledger.
- **Where the roster lives — its own file, named.** The `providers:` block (its prices, its `source`
  citations, its key-variable names) sits in `providers.example.yaml`, which the config **names** with
  `providers_file:` — and it may instead sit **in the config file** inline. Exactly one of the two keys is
  written: a file that carries both, or neither, is a load refusal that names both keys — an empty roster
  (`providers: []`) is a decision you write, never a default the process assumes. A run of `vadis setup`
  never leaves the roster inline: if your config carries it that way, the **block moves into
  `providers.example.yaml` byte for byte** — entries, comments and every `source` citation included — and
  the config keeps a `providers_file:` line where the block was, so the wizard's output is one shape
  whatever your file's history ([ADR-038](../design/decisions/ADR-038-setup-writes-the-pair.md)). Three
  details worth knowing: the run says so when it happens (and `--dry-run` shows the span before anything
  is written); a `providers.example.yaml` that is already there is copied to `.bak` **before** it is
  overwritten, so nothing of yours is lost; and a config that writes both keys or neither is left exactly
  as it is — the wizard configures your file, it does not repair one that does not load. The roster is
  **named, never searched**: there is no default location and no `providers.yaml` picked up because it
  happens to sit beside the config, and a relative value resolves the way every other path in the file does,
  against the directory the config file sits in ([`docs/spec.md` §4.14](../docs/spec.md)). Two consequences
  for your own habits: the roster is part of "the config" when you back it up, and `vadis setup providers`
  edits whichever of the two files actually holds the entry ([Operations](operations.md#backup)). To swap a
  roster wholesale, hand the wizard a file you wrote: `vadis setup providers --from <your-roster.yaml>
  --force` replaces the roster **as a unit** and touches no other byte — a replacement, not an insertion; the
  wizard still cannot add a provider entry to a roster.

### Keys: export them, never write them

The config names environment variables; it never carries a value
([`docs/spec.md` §4.7](../docs/spec.md)). The wizard asks for the **name** and checks whether that variable is
present in your environment — it does not read the value, print it or store it, and it does not write a `.env`.
So collect the snippet it prints for each missing name and export it in your shell:

```bash
export DEEPSEEK_API_KEY='<paste the key here>'
export VADIS_TOKEN='<what you want your clients to send>'   # only if you turned inbound auth on
```

`vadis setup --check` is the read-only version of that check, and it is the one a
script should call: it exits `0` when every named variable is present, and `4` when one is missing or (for
`server.auth_token_env`, which refuses the start) present but empty — see [Operations](operations.md#run-it) for
why that one is fatal.

### After it runs, check yourself

```bash
vadis serve                     # start it; the config is found by the same rule, and a problem is named
curl -s localhost:8790/health    # what this process actually loaded: plugins, per-provider key presence, auth
vadis stats --window 24h        # the figures, once requests have gone through
```

If `serve` exits instead of starting, the message names the key — most often a variable the config names that
your shell does not have. The `--check` mode above answers the same question without starting anything: it
loads the config the way `serve` does.

## Two deployments of one vendor, and what currency means

A vendor often runs more than one deployment: a China-mainland one and an international one. They are
**two different endpoints with two different keys and two different price lists**, and the same model very
often does **not** carry the same id on both. Vadis expresses that with three small keys — and with nothing
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
  deployment a request will actually spend on, and the vadis itself does not check it against the URL,
  because only the vendor knows its own host list.
- **`currency` says what a price is, and the unit never changes.** A mainland entry's prices are
  transcribed from that deployment's own official page **in that page's currency** and stay there. Vadis
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
  route still sends the id its own provider expects, so the tag changes what the vadis prefers, never what
  goes on the wire ([`docs/spec.md` §4.8](../docs/spec.md)).
- **Check the plan's terms before pointing it at a gateway.** Not every vendor allows a subscription key to
  be used through your own tool: at least one mainland coding plan documents that its allowance is only
  usable inside the vendor's supported coding tools, while another documents handing the subscription's key
  to third-party tools. The vadis cannot know which case you are in — it is in your plan's terms, and the
  ADR that records both cases is
  [ADR-018](../design/decisions/ADR-018-currency-region-and-family-mapping.md). And be precise about what
  routing a plan through vadis does and does not claim: what it states is that a request **reached the plan
  account on a protocol the vendor documents** — nothing more. It does **not** state, and must not be read
  as stating, that the plan's allowance is *consumed* the way you expect: whether traffic that arrives
  through a gateway counts against a coding-plan allowance is a **vendor policy** question, and the vadis
  cannot observe the vendor's meter, so no claim in this book can answer it.

- **The names are yours.** The convention this repository uses is `<vendor>[-<region>][<account>]`
  (`kimi`, `kimi-cn`, `kimi-plan`, `kimi-cn-plan`), with the vendor part matching the name its own API-key
  variable uses, so `/health` does not show a name that contradicts its key.

## Authoritative sources

- [`README.md`](../README.md) — quick start (the verified copy-paste path), CLI surface,
  endpoint table.
- [`config.example.yaml`](../config.example.yaml) and
  [`providers.example.yaml`](../providers.example.yaml) — the shipped pair: the server's own settings in the
  root, and the roster with the price schema and its per-model `source` provenance in the file it names.
- [`docs/spec.md` §4](../docs/spec.md) — the config schema as a contract, §4.1 for the
  trace output parameters, §4.7 for the inbound auth key.
- [`docs/spec.md` §5](../docs/spec.md) — the client-side prerequisite (also covered in
  the next chapter).
- [`AGENTS.md`](../AGENTS.md) — the build/test commands and the environment gotchas.
