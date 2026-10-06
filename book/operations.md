# Operations

Status: written for v0.1. Behaviour under failure is normative in `docs/spec.md` §8 and the
store's contract is §4.5; this chapter is the day-2 view for whoever runs the gateway.

vadis is a single local process for a single operator. Operating it is mostly about
knowing what it persists, what it deliberately does not, and what it does when an upstream
misbehaves.

## Run it

```bash
vadis serve                        # finds the config by the rule below
vadis serve --config config.yaml   # or name it explicitly
```

Everything the process does comes from that file: the listen address, the plugin set, the
roster, the aliases, the fallback chain, the trace directory and the state path. There is
no hidden default address and no built-in plugin list to reconcile with it.

### Where the config comes from

`serve` and `stats` find the file in one order ([`docs/spec.md` §4.12](../docs/spec.md)): an explicit `--config`,
else `${XDG_CONFIG_HOME:-$HOME/.config}/vadis/config.yaml`, else `./config.yaml` in the directory you are in.
When none of them is there they exit non-zero and name `--config` and the setup command — there is no silent
fallback to a default configuration. The `vadis setup` command writes to the XDG location by
default (mode `0600`, and any directory it creates at `0700`), which is why the bare `vadis serve` above works
after a first run: `--config` becomes unnecessary, not forbidden.

The paths a config owns all resolve against **the config file's own directory**, never the directory you
run from: the trace directory it names, a plugin's rule file, the state store, and the roster the config names
([`docs/spec.md` §4.1](../docs/spec.md) and §4.14). A config in the XDG location therefore keeps its traces,
its store and its roster beside itself, under `~/.config/vadis/` — so "back up the config and its state
together" means copying that directory, roster included. To keep the traces somewhere else, write an **absolute**
`trace.dir` (a leading `~` is not expanded), and see the startup table below for what a config that cannot be
loaded does.

Four startup outcomes are worth knowing before the first request:

| At startup | What happens |
|---|---|
| the config does not validate | the process **exits non-zero** and names the offending key and the reason. It never silently falls back to a default: a config you edited that "did not take effect" is the most expensive silent failure there is |
| an env var named by `api_key_env` is missing | **not** a startup failure: that provider is marked unavailable and reported by `/health`; the rest of the roster still serves. A missing provider key costs one provider — the one env var whose absence stops the start is the token row below |
| an env var named by `server.auth_token_env` is missing **and the key is written** | the process **exits non-zero** (exit code `4`) and names the variable. This is the one env var whose absence stops the start, and deliberately: a gateway that read "no token in the environment" as "so no auth is required" would drop the operator's only access control |
| the state store cannot be opened or migrated | the process **exits non-zero** (exit code `4`) with the reason. There is no in-memory degraded mode — a gateway that enforced quota from numbers it could not recover would report figures it could not stand behind |

Liveness is `GET /health`, which reports what this process actually loaded: the plugin
set, each provider's key presence, whether inbound auth is required (spec §4.7 — the
variable's **name**, never its value), the resolved state path, and the state store's status
(`open`, or the process would not be running — a store that cannot open is a startup
refusal, see the table above). A plugin listed as `disabled` in the config appears as
disabled rather than missing.

**A change to the config takes effect while the process runs.** `serve` reads the file — and the
roster it names — at startup, and then keeps watching both: edit `config.yaml` (or the roster a
split config names) and a running gateway notices, re-reads the pair, and serves the new
configuration without a restart ([`docs/spec.md` §4.15](../docs/spec.md)). Three things are worth
knowing:

- **Only a real change is a reload.** The decision is the digest of the two files' bytes, so a touch
  or a same-bytes rewrite is no reload at all — nothing happens, and that is observable only as
  *nothing* happening.
- **You can see a reload happen.** `GET /health`'s `config_digest` moves to the new revision; the
  state store gains one `config.applied` row naming the revision it applied, the one it replaced,
  and the keys whose values moved; and a candidate the loader refuses is reported as exactly one
  line on the process's stderr — `vadis: reload refused (…, still serving revision <digest>): <reason>`
  — while the gateway keeps serving the revision it already had and writes nothing to the store.
- **Some keys still need a restart.** The keys the process builds once and holds — the listen
  address (`server.addr`), `trace.dir`, the upstream-attempt timeout, the inbound body bound — are
  refused under a reload with the key named; the process keeps serving, and such a change takes
  effect at the next start.

Nothing in the serving path depends on wall-clock time or turn order, and the reload adds no
normalization: a request's outbound bytes stay a function of the client's own bytes and the revision
in force. (The mechanism and the reasoning are recorded in
[`design/decisions/ADR-039-file-watch-crate-for-the-reload.md`](../design/decisions/ADR-039-file-watch-crate-for-the-reload.md)
and
[`design/decisions/ADR-040-the-revision-switch-and-the-state-store.md`](../design/decisions/ADR-040-the-revision-switch-and-the-state-store.md);
the contract is [`docs/spec.md` §4.15](../docs/spec.md).)

## What it persists

| Artifact | Where | Lifecycle |
|---|---|---|
| trace files | `<config dir>/<trace.dir>/YYYY-MM-DDTHH.jsonl` | append-only, rolled hourly; v0.1 has **no** automatic retention or cleanup, so archiving is a manual operations job |
| the state store | `<config dir>/state/vadis.db` | one SQLite file in WAL mode, holding the event log and the projections built from it |
| request and response bodies | **nowhere** | never written to either artifact; the log keeps a body hash plus a pointer to the trace line |

The parts of the book that matter here: the trace is the only analysis channel, and the
store is never read by the analysis side (ADR-005, ADR-010). Both paths are resolved
relative to the config file's directory, so the state can live outside the repository
(`state/` is gitignored).

## The state store, and its writer

- **One writer per state directory.** While `serve` runs it holds the database file
  exclusively. A second `serve` pointed at the same state directory is **refused at
  startup** with a stated reason, rather than becoming a second writer on one file.
- **Consequence for inspection:** you cannot casually open the file with a SQLite tool while
  the gateway is running (and you will get a busy error rather than a corrupt read): while
  `serve` runs it holds the store exclusively, and a second process's open — a read-only one
  included — is refused. Stop `serve` first, inspect, then start it again. The read-only
  surfaces split the same way: `GET /health` answers against a live gateway because it is
  served by the running process itself, while `vadis stats`' own read-only open is refused
  there too — against a live gateway its report comes out with the one store-derived figure,
  `unknown outcome requests`, **omitted** and a one-line note on stderr naming the refusal,
  every other figure printed unchanged (see
  [Observability](observability-and-accounting.md)). To read that figure, run `stats` with
  `serve` stopped. A dedicated `vadis state`-style surface is a
  separate change, not part of v0.1.
- **The store is a startup prerequisite** (see the table above). An unreadable file is a
  permissions problem to fix, not a mode to run in.

### Backup

Back up the store **and** the traces, together, and treat both as operator data:

- the store's projections (quota counters, sticky bindings) are rebuildable from the store's
  own event log — but **not** from a trace, and not from a log you did not back up;
- a trace is not reconstructible from the store either, and it is the only record of what
  each request cost;
- for a consistent copy, **stop `serve` and copy the database together with its `-wal` and
  `-shm` sidecar files** (copying only `vadis.db` while a WAL is pending loses the tail of
  the log). A SQLite-aware backup taken while stopped is equally fine.

**Why there is no migration for the store's name.** No install of vadis has ever been released,
so no state store exists outside a working tree, and the migration is empty. A fresh start
creates `state/vadis.db`; nothing reads, converts or deletes a store under a pre-rename leaf
name. If such a file exists beside your config (only possible from an unreleased in-tree build),
the gateway simply does not read it — a fresh store is created at next startup. You may delete
the old file, or move it to the new leaf name **together with its `-wal` and `-shm` sidecars**
(the sidecar mistake above applies to the move exactly as it does to a copy). This is optional;
it is not an upgrade path.

**What "the config" is, and why the count can be three files.** The config file and the roster may be two
files: the shipped example is that shape, and the roster is named by the config with `providers_file:`
([`docs/spec.md` §4.14](../docs/spec.md)). Where the roster is named, copy **three** things as a set: the
config file, **the roster it names**, and the state store above. The pair matters because restoring one without
the other does not come up degraded — it does not come up: a config naming a roster that is not there is a
startup refusal that names `providers_file` and the path it looked for. There is no default roster to fall back
on, by design: a silently empty roster would look like a gateway serving nothing rather than a backup you did
not finish. Where the roster is written **inline** in the config instead, the config file *is* the whole config
and this paragraph's only obligation is the one above — copy the store and the traces together.

Nobody else keeps a copy of that state: clients are stateless and resend their whole
conversation every turn, so the gateway is the only place a request's lifecycle is recorded
at all.

## The outcome vadis cannot verify (`unknown_outcome`)

Sometimes the gateway knows it *intended* to call the upstream and cannot know what
happened: the process died mid-request, the client disconnected mid-stream, an upstream
died after the request bytes were fully written, or a stall hit the attempt timeout after the
write. The body may have been billed. Vadis does not guess:

- the request is **recorded and reported** as `unknown_outcome`;
- the quota is **not charged again** (a conservative local re-charge would silently eat your
  plan, which is worse than a visible undercount), and no cost is invented for it;
- reconciliation is **yours**, against the provider's own bill;
- `vadis stats --config config.yaml --window <duration>` reports how many requests in the window
  are in this state (its `unknown outcome requests` line), so the ambiguity
  shows up as a number instead of being absorbed into a total.

Read the honest boundary with it too: v0.1 does not verify that an upstream honours an
idempotency key, so if the upstream does not deduplicate, "was this request billed?" is
undecidable at the protocol layer. No local bookkeeping can answer it, and a retry — the
client's or the gateway's — can legitimately be billed twice. That is why the design keeps
the intent row, refuses a retry after a full write, and leaves the decision to the client,
which at least knows its own idempotency story.

## No server-side session state

The gateway does not store conversations. Requests that arrive with server-side state are
routed stickily and tagged in the trace, so the assumption is continuously auditable rather
than assumed. No bodies are kept — only hashes and pointers — so there is no "vadis keeps
your conversations" surface at all.

## Failover, cooldowns and degradation

- **Failover**: a configured ordered chain of routes is tried when an upstream errors,
  rate-limits or exhausts its quota; switching routes loses the prefix cache, and the
  re-prefill cost plus the origin route are recorded. An exhausted chain is a clean gateway
  error, not a hang. A connection failure (the upstream unreachable, nothing sent) is its
  own reason — `connect_failure` — and walks the chain like any retryable failure; a true
  timeout keeps its own name and is never retried after a full write.
- **Cooldown**: a failure that is about the *account* (an exhausted plan, a billing refusal)
  marks the whole provider unavailable for a cooldown, so requests stop being spent
  rediscovering a dead route one request at a time. A rate limit cools the route instead.
  The cooldown is state: it survives a restart, is reported by `/health` and counted by
  `vadis stats`. It is **not** the plan's probe deadline: a provider cooldown is route
  availability, while the probe deadline is the family's own cooldown, and `/health`'s plan
  section reports them separately — a cooling provider makes a probe wait without moving the
  family's account, and the trace records why
  ([`docs/spec.md` §9](../docs/spec.md)).
- **Retry discipline**: a retry happens only where the evidence says the attempt was not
  billed. A failure after the request bytes were fully written is never retried by the
  gateway — see the unknown-outcome section.
- **Degradation rules**: a failed transform falls back to the original payload and the
  request is still served; a prefix discontinuity warns by default and can be made
  rejecting; unknown fields pass through; a failed trace write never blocks a request but is
  recorded rather than hidden.

## Upgrades and rollback

One logical change per commit, on its own branch, merged only when its gates pass; a
change that fails leaves documentation and no broken code. The store's schema is migrated
**forward only** — a database written by a newer build is refused by an older one rather
than read on a guess. Restore a backup if you need to go back. A bounded parameter may also
be adopted online: it is tried on a share of *new* sessions only (never changed mid-session,
which would discard the prefix cache), and it reverts by itself when its declared signals go
wrong (ADR-012, ADR-013).

## Troubleshooting entry points

| Symptom | First thing to check |
|---|---|
| the client sees `503` and nothing reaches the logs | the local-proxy prerequisite (`NO_PROXY=127.0.0.1,localhost`) — see [Connecting clients](connecting-clients.md) |
| the process exits immediately at startup | the config (it names the offending key), the state store (permissions, a migration, or a second instance holding the writer lock), or — with `server.auth_token_env` written — a token variable that is unset or empty (see the startup table above) |
| a request comes back `400 capability_unsupported` | the inbound protocol is not a cell the route's entry declares — read that entry's `supports` and send one it declares, or point the client at the matching endpoint; nothing is wrong with your setup |
| costs moved | prefix continuity between turns in the same session, before anything else |
| a provider seems to be skipped entirely | it is inside a cooldown, which `/health` reports |
| "nothing is being charged" | check `unknown_outcome` counts and whether usage was missing for those requests — a reported ambiguity is not a bug, an invented number would be |
| a request went somewhere you did not ask for | the guard hit and the fallback chain applied, or the route was in a cooldown; the trace records the origin route and the switch cost |

## Authoritative sources

- [`docs/spec.md` §8](../docs/spec.md) — unified error body, the error-type-to-HTTP table,
  response headers and degradation behaviour.
- [`docs/spec.md` §4.1](../docs/spec.md) — trace output parameters and path resolution.
- [`docs/spec.md` §4.2](../docs/spec.md) — the failover chain as configured.
- [`docs/spec.md` §4.11](../docs/spec.md) — the guided configuration command and what it may write
  (the state store's own contract stays §4.5).
- [`docs/spec.md` §4.12](../docs/spec.md) — where the config file is found, and what the paths inside it mean.
- [`design/decisions/ADR-025-setup-writes-by-anchored-edits-on-a-verbatim-template.md`](../design/decisions/ADR-025-setup-writes-by-anchored-edits-on-a-verbatim-template.md)
  — why the configuration is edited rather than regenerated, and why one file is read rather than several
  merged.
- [`docs/spec.md` §4.5](../docs/spec.md) — the local state store: the event-log / trace
  split, the join key, durability tiers and the failure behaviour.
- [`docs/spec.md` §9](../docs/spec.md) — the reporting surfaces: `/health`'s plan section, the
  `vadis stats` report, and what is not served yet.
- [`design/DESIGN.md` §8](../design/DESIGN.md) — state and persistence boundaries.
- [`design/DESIGN.md` §11](../design/DESIGN.md) — risks and mitigations.
- [`design/DESIGN.md` §12.10](../design/DESIGN.md) — the store's DDL, the writer lock, the
  migrations and where each event is written.
- [`design/decisions/ADR-009-persistence-boundary-sqlite-wal-store.md`](../design/decisions/ADR-009-persistence-boundary-sqlite-wal-store.md)
  — the storage boundary, durability tiers and the startup-prerequisite rule.
- [`design/decisions/ADR-010-event-log-as-state-truth.md`](../design/decisions/ADR-010-event-log-as-state-truth.md)
  — write-ahead, the `unknown_outcome` rules and the honest boundary around idempotency.
- [`design/decisions/ADR-011-upstream-error-taxonomy-and-recovery-actions.md`](../design/decisions/ADR-011-upstream-error-taxonomy-and-recovery-actions.md)
  — the error taxonomy, the provider cooldown and what a failover costs the cache.
- [`design/decisions/ADR-013-online-iteration-rails.md`](../design/decisions/ADR-013-online-iteration-rails.md)
  — how a parameter adopted online behaves on your traffic.
- [`AGENTS.md`](../AGENTS.md) — build/test commands, version-control rules, environment
  gotchas.
- [`docs/spec.md` §9.3`](../docs/spec.md) — which reporting surfaces are served and which are
  planned.
