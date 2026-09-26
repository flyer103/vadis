# ADR-040 — what a revision switch means: one immutable revision per request, one continuous ledger, and nothing the reload invents

- Status: accepted
- Date: 2026-09-26
- Related: **ADR-037** — the decision this one continues: **D6** gives the identity the switch is
  keyed on (`config_digest = sha16(root_sha16 + ":" + roster_sha16)`, a *byte* digest over the pair),
  **D2** gives the files it reads (*named, never searched* — the root names its roster, so the reload
  has one path to look at and not a search), **D4** gives the once-at-load join the serving path reads
  (`RouterConfig::providers` stays the only representation), and **D7** is the item this ADR closes:
  *"the atomic-swap decision"* left to the reload's own round; **ADR-039** — the **mechanism** half of
  D7 (a file-watch crate, `notify` 8.2.0, in `router-cli` only; the watcher decides *when to look*, the
  digest decides *whether anything changed*), and D4 there is the list of what it deliberately left
  here; **ADR-002** (the declarative loader, the fiber state machine, the unload order, and the keyed
  config diff this ADR does **not** assume exists — §12.2's sketch, measured below); **ADR-004** (the
  stateless client and the sticky table: no *durable* session state is invented to serve a client that
  resends everything — the reason the reload adds no column and no pin it cannot reconstruct);
  **ADR-009 / ADR-010** (the event log is the truth, intent precedes effect, projections are
  rebuildable — the store's contract the switch must not disturb); **ADR-025** (the writer whose landing
  is a temp file plus a `rename`, the fact behind D4's torn-pair case); **ADR-003 / ADR-005** (the
  observation boundary: the trace is the only product → autowork channel); AGENTS hard constraints
  **1** (the byte boundary — untouched by definition here), **2** (content determinism — the constraint
  D2's capture-once decision exists to protect), **3** (the observation boundary), **4** (no unverified
  savings — D7 refuses to claim the upstream cache is preserved or lost without measuring it),
  **8** (docs before code) and **9** / ADR-012 (the measurement is not part of the search space — this
  ADR moves no gate, no corpus and no conformance assertion); spec §4.11 (*the landing*, and *the loader
  is the gate*), **§4.14** (the pair, the ladder and the identity), §6 (`config_digest` on the record),
  §9.1 (`/health`'s config member); DESIGN §12.2 (the runtime's primitives and the keyed diff),
  §12.10.2 (the loader's refusals and the exit codes), §12.10.4 (the store's DDL and the projections),
  §12.10.5 **row 12** (`plugin.loaded`/`unloaded`) and **row 13** (`config.applied`), §12.6 (the record),
  §12.10.5 **note R9** (new, this ADR's landing).
- Numbering note: the register holds **39** ADRs (`ADR-001` … `ADR-039`), so **040** is the next free
  number.
- Scope note: **this ADR writes no code and moves no manifest.** It freezes the semantics of a revision
  switch; the watcher plumbing, the loader's second entry point, the publish and their tests land with
  the reload round's own cards. `Cargo.toml` at this commit names no watch crate and `crates/` contains
  no watcher (both measured below, re-measured at this ADR's commit rather than inherited from ADR-039).

## Background

**The item is the second sentence of ADR-037 D7 and it has been waiting since R43.** D7 puts the reload
in its own round and names three pre-requisites: *"the watcher decision (a file-watch crate … versus a
std-only mtime poll), the atomic-swap decision, and a fresh p99 ladder run."* ADR-039 closed the first
and explicitly left this one (its D4, row 1: *"what a revision switch is: which structures are replaced,
how the old revision stops being used, what an in-flight request sees"*) together with the state store's
side of it (its D4, row 2). `autowork/STATE.md`'s waiting-on-human row 17 carries the same item.

**What already exists, so the decision is as small as it is.** The reload does not need an identity
invented: `config_digest` is computed once at load (`crates/router-cli/src/config_load.rs:199`), carried
on every trace record (`crates/router-core/src/trace.rs:46`), written into `config.applied`'s payload
(`crates/router-cli/src/lib.rs:229`) and reported by `/health` (`crates/router-proxy/src/health.rs:41`).
The reload does not need a second reader: `config_load::load`
(`crates/router-cli/src/config_load.rs:146`) reads the root, reads the named roster, joins and validates
— the same two calls `router setup` reaches the parser through (DESIGN §12.10.2's *"`load` is the only
reader, and `load` is the only validator"*).
And it does not need a new event: DESIGN §12.10.5 **row 13** already specifies `config.applied` as
written *"at startup after validation, **and on every accepted config diff**"* — a trigger the tree has
never had a caller for, because nothing diffed a config (measured: `config.applied` is appended at
exactly one site, `crates/router-cli/src/lib.rs:212`, on the startup path).

**Three facts about the tree bound this ADR, and each is a measurement rather than an assumption.**

1. **There is no reload machinery, and the assembly says so in its own words.** `declared_inject`
   (`crates/router-plugins/src/assembly.rs:276-277`) records it: *"the assembly is built once at start-up
   and never reloaded (the round's scope call (ii) — no config-diff, no live reload)."* The runtime can
   load, unload and **re-load** a fiber — `router-runtime/src/loader.rs:807` is the unit test
   `a_reloaded_provider_reactivates_its_dependents` — but the *keyed config diff* DESIGN §12.2's last row
   names (`apply_config_diff`) **does not exist** (`git grep apply_config_diff` → no output). So D5's
   "mount only the delta" is a contract for the implementing round, not a capability it inherits.
2. **No store table is keyed by the configuration, and the DDL is the evidence.** The seven tables
   (`crates/router-store/src/lib.rs:42-110`) are keyed by `event_id`, `session_key`,
   `(session_key, block_index)`, `(provider, plan_idx, window_start_us)`, `(scope, provider, model)` and
   `family`; **no column anywhere holds a digest or a revision**. The identity is a *carried value*
   (trace field, event payload member, `/health` member), which is exactly what ADR-037 D6 decided it is:
   *"the identity is attribution, not a score … it may not become a gate input."*
3. **The landing that produces the change is atomic per file, and the pair is two files.**
   spec §4.11: the candidate *"goes to a temporary file in the target's directory, is flushed, and is then
   `rename`d over the target (one atomic replace on one filesystem) … Nothing lands partially, and no
   other process can observe a half-written config."* One file, one atomic replace — but a split config
   is a **pair**, and §4.11's own *"the candidate is the pair"* bullet is about **validation**, not about
   atomicity: two `rename` calls are two instants, so a reader can observe a new root beside an old
   roster. D4 answers that case rather than pretending it cannot happen.

**What a "revision" therefore is, in one sentence.** A revision is **the immutable value a successful
load of the pair produces** — the validated `RouterConfig`, the resolved paths and identity (§12.10.2's
`ResolvedConfig`), and whatever the runtime mounted for it — and two revisions are the same revision
**iff their `config_digest` is equal**, because ADR-039 chose the digest as the decider and the
notification only as the trigger.

## Decision

### D1 — The identity decides, not the event; and a look that finds no change is a no-op

The watcher's event is a **hint that something may have moved**. The reload's first act on every look is
to read the pair and recompute the identity; only a **different** `config_digest` is a revision. A look
that finds the same digest ends there: no revision, no event, no plugin edge, no log line that claims a
change, nothing observable at all. This is ADR-039 D2's own argument (*"the mechanism's lossiness lands
on when we look, not on what is true"*) read from the other side, and it is what makes a debounce policy
harmless: a coalesced event can only cause an extra *look*, never a spurious *revision*.

*Observable outcome:* after a file touch that changes no byte of either file (a `chmod`, a re-`chmod`, a
same-content rewrite), `/health`'s `config_digest` is unchanged **and** the store gains no
`config.applied` row **and** the trace gains no `plugin.loaded`/`plugin.unloaded` row.

### D2 — The switch is a publish of one already-built revision: all-or-nothing, and one revision per request

**The candidate is built and validated before anything is visible, and the switch itself is one atomic
publication of that finished value.** The sequence, in order:

1. **read** the pair (the root, and the roster the root names — ADR-037 D2);
2. **gate** it through the *same* loader `serve` starts with (spec §4.11's *loader is the gate*;
   DESIGN §12.10.2's table): parse, join, `validate()`, and resolve every path. A refusal here ends the
   attempt (D4) and nothing below runs;
3. **commit** one `config.applied` row (row 13, `FULL`, the class intent/accounting uses) carrying the
   new digest — the same order the startup path already uses (*"committed before the process starts
   serving on top of it"*, `crates/router-cli/src/lib.rs:210-231`);
4. **publish** the handle: one store that a reader either sees complete or does not see.

**The serving path captures the published revision once per request**, at receive, and reads *that*
revision for the request's whole lifetime — the same discipline that already governs the trace writer
(`ConfigTraceWriter` holds the digest beside the sink it wraps, `crates/router-cli/src/lib.rs:99-135`,
and refuses to construct with an empty one) and the same discipline ADR-037's consequences demand
(*"nothing behind a request opens, reads or hashes a file"*).

*Observable outcomes* (each assertable from the trace plus the event log; the assertion list is
`RV-1`…`RV-6`, stated once in D8):
- **RV-1.** Every `DecisionRecord` carries a `config_digest` that some `config.applied` row of the same
  process also carries, and the last such row precedes the record's `identity.event_id`. A record whose
  digest has no such row is a defect, not an edge case.
- **RV-2.** No request is ever served by a mixture: there is no `config.applied` row for a revision the
  loader refused, and no record carries a digest composed of two files that were never loaded together.

### D3 — The window where two revisions exist: *within* one request, one revision, always; the bound on the window is stated per arm

**Within a request there is never a window.** A request that arrived before the publish finishes on the
revision it captured; a request that arrives after it is served by the next one. The reload cannot
interleave inside a request, because the request's view of the config is a value it took once — which is
AGENTS constraint 2's requirement (*a request's outbound bytes are a pure function of (client bytes,
stable config)*) and not a convenience.

**Across requests, two revisions are necessarily live for a while**, and what bounds that while is a
function of the owner's answer to the question in the next section:

- **if no session is pinned** (the "no pin" family), the bound is **the last in-flight request that
  captured the old revision**: the old revision's structures are retained while a handle to them exists
  and are dropped with the last holder. No deadline, no timer, no drain window, nothing to configure.
- **if a session is pinned** (the "pin" family), the bound is **the last live session bound to the old
  revision** — i.e. up to `session.ttl` (`SessionCfg`, `crates/router-core/src/config.rs:628`; DESIGN
  §12.10.4's `sessions.expires_at_us`), and the old revision's structures must be retained for that whole
  period.

Either way there is no knob: the bound is derived from state the process already has.

*Observable outcome:* a trace window spanning a switch contains **both** digests, and that is the correct
record of the switch, not a defect — `identity.event_id` (the trace's ordering anchor) is not monotone in
`config_digest` while `events.event_id` is; RV-1 is what stays true in both directions.

### D4 — Rollback: a refused or half-landed change is a *report*, never an exit, and a published revision is immutable

The three ways a switch fails, and the state each leaves behind:

| Case | What happens | What the process serves afterwards |
|---|---|---|
| the loader **refuses** the candidate (spec §4.14's ladder, or any other load refusal) | the attempt ends at step 2 of D2: no `config.applied` row, no mounted plugin, no publish; the loader's own reason is reported on the process's surface | the revision it was already serving, **unmodified** |
| the **file vanishes between the two reads** (root read, roster gone/unreadable/not UTF-8) | the loader refuses (ladder shape 3, naming `providers_file`, the value as written and the resolved path); same as above | the last accepted revision |
| the watcher **fires during the landing's `temp` + `rename`**, or between the pair's two renames | the digest decides (D1): an unchanged digest is a no-op; a changed one is a candidate that either loads (a legitimate revision the operator landed) or is refused (the last accepted revision keeps serving) | the last accepted revision, or the new one if the pair as read loads |

**The reload never exits, and that asymmetry with startup is the decision.** At startup a refusal is
`exit 2` (config) or `exit 4` (an unsatisfiable environment/store prerequisite) because *nothing is
serving yet* and DESIGN §12.10.2's rule — *"there is no partially-started process"* — is what protects
the operator. A reload has a live revision and a live client; the same gate therefore answers
differently: **keep serving, report the reason, and keep the reason out of the state store** — because
`config.applied` means *"this process is serving this revision"* and a refusal means it is not. Writing a
row there would make the row read as an application that did not happen. Whether a refusal gets a surface
of its own (a log line's exact shape, an event of its own, a `/health` member) is the observability
axis (R47-0b's); what this ADR fixes is that a refusal is **observable** and that it **writes no
`config.applied` row**.

**A revision once published is immutable.** A correction is published as a *new* revision; nothing
patches a loaded `RouterConfig`, a resolved path or a mounted rule set in place, and the publish is the
only writer of what the request path reads. That is what keeps AGENTS 2 true across a switch: the
"stable config" a transform is a pure function of cannot change underneath a request.

*Observable outcomes:*
- **RV-3.** After a refused reload: the process is the same process (no exit code 2/4 attributable to a
  reload — a restart is the only way to see one), `/health`'s `config_digest` is the pre-attempt value,
  and no new `config.applied` row exists. The refusal itself is visible on the process's surface.
- **RV-4.** After a **torn pair** (root new, roster old) that loads: it *is* a revision — the loader is
  the only judge of what a configuration is, and a mixed pair that validates is a configuration the
  operator landed. What never happens is the *other* thing: a record carrying digest `D` while the
  process serves structures belonging to `D'`.

### D5 — What a revision may change: everything the serving path reads per request, and nothing the process resolved once

**Reloadable** (any key the serving path reads per request or per revision): the roster (`providers:`,
or every key of the file it names), `aliases`, `fallback`, `plan_policy`, `quota`, `plugins` (including
`disabled`), `session`, `cache`, `server.upstream_attempt_timeout`, `server.request_timeout`,
`server.max_body_bytes`, `server.auth_token_env` (see the honesty note below), `trace.rollover` — and so
on: the rule is the read site, not a list.

**Not reloadable — refused, with the key named**: `server.addr` (the listener is bound once), the
state store's path (fixed at `<config dir>/state/router.db` in v0.1, spec §4.5) and the resolved
`trace.dir` (resolved once at load and held by the trace writer, which is also what makes `trace.rollover`
the unit that may move, not the directory). A revision that differs in one of these is refused at
publish, the refusal names the key, and the remedy is a restart — which is the honest answer, because a
reload that re-bound a listener would be a drain-and-rebind capability and not a config reload. This set
is deliberately short and revisitable (Reversibility, below).

**A revision mounts only the delta.** `plugins` are compared by the same identity a plugin entry already
has (`id`, `kind`, `enabled` and its config — DESIGN §12.2's last row), and a switch writes
`plugin.loaded` / `plugin.unloaded` (row 12) **only at the edges it actually crossed**: an unchanged
plugin is left mounted, a `disabled` one is unloaded, a changed `id`/`kind` is rebuilt. A revision that
changes only the roster writes **one** row (row 13) and no plugin rows at all. Measured honesty: the
diff API for this does not exist yet (Background, fact 1), so this is a contract for the round that
lands the reload, not a description of the tree.

**One honesty note on `auth_token_env`, because it is a process fact and not a file fact.** The token's
*value* is read by `std::env` once, at startup (`crates/router-cli/src/lib.rs:176-193`), and rotating it
means restarting the process — that is the existing contract (spec §4.7). A revision may therefore
change the *name* the config names, and the reload must then read the newly-named variable; a revision
that names a variable that is unset refuses the switch with the startup refusal's own reason
(`crates/router-cli/src/lib.rs:184-189`) rather than quietly serving without a gate. Whether the value
should be re-read per switch is **not** decided here: the existing sentence ("read once") stands, and
re-reading it would be a change to §4.7's contract, not to this one.

### D6 — The state store across a switch (A2): one ledger, no generation, no migration, no revision column

**Nothing in the store is keyed by the revision, and nothing becomes keyed by it.** A switch adds
**exactly one row** — the `config.applied` of D2 step 3 — and changes **no** row of any projection. The
consequences, each following from a decision already made elsewhere:

- **No new generation.** The event log is linear and its `event_id` is the ordering anchor (ADR-010);
  a per-generation ledger would break the two joins the store's own conformance asserts — projection ==
  rebuild (CONF-21) and trace ↔ event (CONF-24) — for no information the digest does not already carry.
- **No migration, no backfill.** `config_digest` is not a stored column today (Background, fact 2), so
  there is nothing to backfill and no DDL version to move. A run of records that predates a switch simply
  carries a digest whose `config.applied` row is earlier in the log; a record that predates the field
  entirely is ADR-037's "this record predates `config_digest`" case, unchanged.
- **A session pinned to the old revision is answered by D3's arm, not by a column.** The pin, if the
  owner chooses one, is process memory, and it is deliberately **non-durable**: a restarted process has
  one revision and serves everyone by it. Making the pin durable would put `config_digest` into
  `sessions` — turning attribution into a key, which ADR-037 D6 forbids. The consequence is stated
  plainly: **the pin arm is only honest as an in-memory, restart-forgetting pin.**
- **A binding the new revision no longer resolves is a miss, never a route.** The sticky table stores
  `(provider, model)`; a switch can make that pair dangle (the model left the roster, the provider was
  removed). The rule, and it is arm-neutral: **a binding is always checked against the revision that is
  serving the request** — the pinned one under the pin arm, the current one otherwise — and a binding
  that the serving revision cannot resolve is treated as a **miss** (the request is re-resolved from the
  request itself, which is what a stateless client's resend makes possible), not as a route to a model
  the configuration no longer declares. A request naming a model the configuration does not have is
  refused by the existing unknown-route path (§8), which is a client-visible fact and not ours to
  invent.
- **No revision state at all survives a restart.** The current revision lives in process memory and is
  re-derived from the files at the next start, where one `config.applied` row is written exactly as it is
  today. ADR-004's rule holds: no durable session or revision state is invented to serve a client that
  resends everything.

*Observable outcomes:*
- **RV-5.** The `sessions`, `cache_ledger`, `quota_counters`, `provider_cooldown` and `plan_state`
  tables are **row-for-row identical before and after a switch**; the switch's only new row is a
  `config.applied`. (Assert by snapshotting the five tables around a live switch, or by re-running
  CONF-21's rebuild oracle across one.)
- **RV-6.** A request whose sticky binding the new revision cannot resolve carries `sticky_hit: false`
  and is served by the new revision's resolution (or refused with the existing unknown-route error) —
  never routed to a route the revision does not declare.

### D7 — The cache consequence, stated rather than assumed: the reload is content-transparent, and it does not preserve the upstream prefix

AGENTS.md's own measurement is the frame: prefix stability is the **first-order** cost lever — 960 cached
tokens of 14 409 on turn 1 rising to **14 400 of 14 520 (99.2 %)** on turn 2 of the same session, with
model choice second-order. So a switch that perturbs the outbound prefix silently raises cost, and this
ADR answers it with three statements and no mechanism:

1. **The reload normalizes nothing.** It re-reads the pair and publishes the config the operator wrote:
   no canonical form, no key reordering, no re-serialization of the roster, no "stable ordering" pass.
   The outbound bytes remain a pure function of (client bytes, revision) — ADR-025's refusal of a second
   serializer is the same refusal, one layer down.
2. **A revision with no outbound-visible change is prefix-neutral by construction.** The facts of a
   revision that can reach the wire are narrow: the **resolved route's provider-native model id**
   (spec §2 mutation (b)) and the **set/plan of content transforms** the revision mounts (which rule file
   each transform plugin loads, and whether it is enabled). A revision that changes only non-outbound
   keys — the listen address, timeouts, `trace.dir`, `session.ttl`, cache/breakeven defaults, prices,
   quotas, `currency`/`region` — produces byte-identical outbound bodies for the same inbound bytes, and
   therefore leaves the upstream's cached prefix intact. **That is a testable claim, not a hope:**
   consecutive same-session turns across such a switch carry **identical** `prefix.blocks[].hash` and
   `cache_control_breaks == 0`.
3. **A revision that *does* move an outbound-visible fact invalidates the upstream prefix for live
   conversations, and the reload does not pretend otherwise.** It does not preserve the old prefix by
   keeping two encoders, two rosters or two rule sets alive for old sessions — that would be invented
   state (ADR-004) bought with an *inferred* saving (AGENTS 4). What it does instead is **attribute** the
   cost: the records after the switch carry the new `config_digest`, and the re-prefill is visible where
   the repo already measures prefix breaks (`prefix.continuity`, `cache_control_breaks`, and the
   `reprefill_tokens` / `switch_cost_nano` figures the failover path already labels *inferred*). A
   switch's cost is therefore readable from the trace and attributable to a revision, never estimated by
   the reload itself.

**One correction of the entering card's own illustration, because a contract may not repeat an
unverified example.** The card illustrates the cache risk as *"a switch that perturbs `tools` or their
order"*. The tool schemas are **the client's bytes**, not the configuration's: they arrive in every
request, and the reload cannot reorder or reformat them without violating AGENTS 1 (the byte boundary)
and §12.10.7 (mutation (b) is the only outbound rewrite). The configuration's real outbound-visible
levers are the two in statement 2 above, and the transform **rule files** are not among the paths the
reload watches at all — the reload's watched pair is the root and the roster (ADR-037 D2; ADR-039 D4's
*which module owns the watcher*), and the identity is defined over that pair alone, so a rule-file edit
does not even move `config_digest`. Whether rule files ever join the watch set is a **separate decision**
for the round that wants it, and it would change what the identity is computed over (ADR-037's own
reversibility note anticipates exactly that).

### D8 — What must remain invariant (the list the implementing round is held to)

| # | Invariant | The observable outcome that asserts it |
|---|---|---|
| **RV-1** | one revision per request, attributable | every record's `config_digest` has a `config.applied` row of the same process, the last such row preceding its `identity.event_id` |
| **RV-2** | never a partial revision | no `config.applied` row exists for a candidate the loader refused; no record's digest is composed of two files never loaded together |
| **RV-3** | a reload never stops serving | a refused reload leaves the process running, `/health`'s digest unchanged, no new `config.applied`; no exit code 2/4 is attributable to a reload |
| **RV-4** | a published revision is immutable | no in-place mutation of a loaded config; corrections appear as a new digest |
| **RV-5** | the store's projections are untouched | the five projection tables are row-for-row identical across a switch; the only new row is `config.applied` |
| **RV-6** | the digest never becomes a key | no store column, no config key and no gate input holds `config_digest`; a record's digest is attribution |

Three further invariants are inherited and restated only so a reader holds them while reading this ADR:
**the byte boundary** (AGENTS 1) is untouched by definition — a reload moves no request byte, and the
only mutations are the two §12.10.7 already owns; **the observation boundary** (AGENTS 3/ADR-003) is
untouched — the reload's only product → autowork channel is the trace JSONL, and the *revision identity
on that trace is `config_digest` on every record*, additive as ADR-037 D6 left it, with
`TRACE_SCHEMA_VERSION` staying **2**; and **there is no new user-facing flag** — the reload is default
behaviour, as the owner's standing R44 ruling requires (`setup`'s pair write is the precedent: the
capability is a behaviour, not a parameter). No `--watch`, no `router reload` verb, no `SIGHUP`, no
`server.reload: true`, no interval key. The **debounce window** ADR-039 D3 registered as a second
allowlist decision is an implementation constant, **not** a config key, and it is R47-0b's to decide.

### D9 — What this ADR does not decide, and where each remainder lives

| Not decided here | Whose |
|---|---|
| the exact payload shape of `config.applied`'s *changed keys* half, its emission point, and whether a *refusal* gets a surface of its own | R47-0b (the observability axis) — this ADR fixes only that the row's trigger is an **accepted** diff and that a refusal writes no such row |
| the debounce window and its owner (a second DESIGN §12.1 allowlist row, or a hand-rolled timer) | R47-0b (ADR-039 D3's registered decision) |
| **what a conversation that spans a switch sees** — the user-visible half of A1 | **the owner** (the question below) |
| whether a reload invalidates the L1 latency envelope, and which gate consumes a fresh ladder | **the owner** (AGENTS 9 / ADR-012: the measurement is not part of the search space) |
| the module that owns the watcher, how it survives the landing's `rename`, and what the *next look* is after a missed event | R47-0b / the implementing round (ADR-039's consequences make the false-negative question an obligation, not a preference) |
| any conformance **ID** (the assertion intent in D8 is intent; §12.8's rule makes an ID a human allocation) | the owner, on the round's request |

## The owner's question — A1's user-visible half, drafted and not decided

**The structural half is decided (D2, D3): a request is served by one revision, and a request in flight
when a switch lands finishes on the revision it captured, because it took that revision once.** What is
**not** decided — and may not be decided by a round, because it is the behaviour a *client* sees — is
what happens at the **conversation** level, i.e. on the *next* request of a session that spans a switch.
Both arms are drafted below with the consequence each has for a client mid-conversation; the ADR picks
neither.

**Arm P — "finish on the old revision" (a session is pinned).** A conversation that began under revision
N keeps being served by N until its binding expires (`session.ttl`) or the conversation ends; new
conversations take N+1 immediately.
- *What a client mid-conversation sees:* **nothing changes.** No error, no re-prefill, no behavioural
  surprise inside a turn sequence. The upstream prefix cache for that conversation **survives the
  switch**, which is the 99.2 % lever AGENTS.md measures; a client comparing two consecutive turns sees
  the same contract.
- *What it costs:* the fix is **deferred** for live conversations — an operator correcting a wrong price,
  a broken route or a provider outage waits up to `session.ttl` for the last pinned conversation, and the
  process serves two revisions of the outbound contract at once for that window (the two-revision bound
  becomes "the last live pinned session", D3). The pin must live in **process memory** (D6): durable, it
  would make `config_digest` a key on `sessions`, which ADR-037 D6 forbids — so the pin is
  restart-forgetting, and a restart serves everyone by the newest revision.

**Arm D — "drop with a typed error".** No session is pinned: the revision retires the moment it is
replaced, and a request belonging to a conversation that was bound to a retired revision is **refused**
with a typed error (a new `error.type` in §12.7's vocabulary, naming both digests) rather than silently
served by the new one.
- *What a client mid-conversation sees:* an **interrupted conversation** — the turn fails, and the client
  must restart its thread (a fresh session, a cold prefix) to continue. It is a failure the client did
  nothing to earn: the trigger is entirely internal to router.
- *What it buys:* exactly **one** revision serves at any instant — no pin, no retention, no second live
  revision, and no possibility of two turns of one conversation being governed by different
  configurations. The switch is total and immediate, and its effect is unmissable.

**The third shape, named because the question is really about the pin, not about the error:** *no pin and
no error* — the next request of a spanning conversation is simply served by the **current** revision
(the middle arm), paying at most **one** re-prefill on the turn that crosses an outbound-visible change,
visible in the trace as `prefix.continuity < 1.0` and `cache_control_breaks > 0` and attributed to the
new digest. This is the arm that needs no new behaviour at all; it is the honest baseline both arms above
are departures from.

**Recommendation: Arm P**, with the pin held **in memory only** and bounded by `session.ttl`. The reason
is the repo's own cost ordering and its own precedent: prefix stability is the first-order lever
(AGENTS.md's measurement), and the sticky table already makes exactly this trade for the *route* — a
conversation is kept on one route for the life of its binding (ADR-004) so that its prefix stays intact.
A revision pin is the same trade one level up, and its failure mode is the benign one: live conversations
keep the revision that was working, while a *fix* reaches every new conversation at once. It should be
chosen against the honest counterweight, which is that the process then serves two configurations'
contracts at the same time (a second live revision, retained in memory for up to `session.ttl`) and that
"the file is what is served" becomes "the file is what the next conversation is served".

**If the owner prefers Arm D**, the error's vocabulary and its trace/event treatment are implementation
work this ADR does not budget; and **if the owner prefers the middle arm**, the reload needs no new
behaviour at all and this ADR's D2/D3 stand unchanged. Until the ruling, **no implementation of the
session-level policy may land** — a card that needs it blocks on the owner.

## Alternatives considered

| Alternative | Why it is not the decision |
|---|---|
| **A per-request read of the configuration** (check the digest at receive, or read the files on the request path) | it makes a request's cost a function of the filesystem, and AGENTS 2's determinism argument is exactly about this class; ADR-039 already refused it in the same words (*"the reload must be its own loop, never a per-request check"*) |
| **Drain the old revision on a deadline** (a fixed window after which the old revision is dropped and in-flight requests are cut) | it buys a knob (the deadline is a value someone must choose and remember — the thing the owner's R44 ruling refuses) for a bound the last holder already gives for free, and it introduces the only failure mode the reload has no reason to have: a request killed because a timer expired |
| **A second process, a socket hand-off, or a re-exec** (the "blue/green" reload) | the store's writer lock is exclusive by design (DESIGN §12.10.4: a second `serve` on the state directory is `Locked`), so a second process cannot hold the same state; and re-exec discards the trace writer, the store handle and every live conversation's in-memory continuity for a switch whose whole point is continuity |
| **A revision column on the projections** (or a `revisions` table) | it makes `config_digest` a store key, which ADR-037 D6 forbids (*attribution, not a score*), and it forces the serving path to read a revision it already holds in memory |
| **Preserving the upstream prefix across a switch** (keep the old roster/rule set alive for old sessions, or replay the old `tools` layout) | it is invented state (ADR-004) bought with an *inferred* saving (AGENTS 4), and the second live encoder is a second outbound contract — the L2 leak pattern §12.12 names, one layer up |
| **A `config.applied` row for a refused candidate** | the row's meaning is *"this process is serving this revision"*; a refusal means it is not. A refusal needs a surface (0b's), not this row |
| **Restart-on-change** (the operator's `systemctl restart`, or a self-exec) as the reload's implementation | it is the behaviour the round exists to replace, and it is not reload's equal: a restart loses the store's in-memory projections' warmth, the trace file handle and any live conversation's continuity |
| **Making the reload opt-in** (`--watch`, `serve --reload`, a `server.reload` key) | the owner's standing R44 ruling: no parameter the user must remember. The capability is a behaviour, and `setup`'s pair write is the precedent |
| **Normalizing the config on load** (a canonical ordering/formatting pass, so "equal configs" have equal bytes) | it would make a *comment* edit invisible and a *reformat* a no-op, destroying the one thing the digest is for (ADR-037: the comments are where the price citations live), and it would violate the byte boundary's spirit by rewriting what the operator wrote |

## Rationale

- **The reload is a publish, not a process change.** Every property the round needs — all-or-nothing
  visibility, one revision per request, a bounded window, a re-statable rollback — follows from treating
  a revision as an immutable value that is built, validated, committed and then published once. The
  alternative (mutating a loaded configuration in place) has no rollback story at all, which is why D4
  can be one paragraph.
- **The identity the tree already has is the whole key.** `config_digest` decides *whether* (D1), names
  *what* a record was priced by (RV-1) and attributes *what a switch cost* (D7.3). No new vocabulary is
  introduced; the reload makes an existing field do the work ADR-037 D6 designed it for.
- **The asymmetry between startup and reload is the gate read correctly.** The loader is the gate in both
  cases (spec §4.11), but its *consequence* is a function of what is at stake: at startup there is
  nothing to protect, so refusal is an exit; during a reload there is a live client, so refusal is a
  report. Reading the same gate two ways is not a contradiction; treating a live process like a starting
  one would be.
- **Where the cost question is answered matters.** The reload neither preserves nor destroys the upstream
  cache; the *content* decides, and the reload is content-transparent (D7). That is the only answer that
  keeps AGENTS 4 honest — any stronger claim ("the reload preserves your cache") would be an unverified
  saving, and any weaker one ("the reload may invalidate everything") would be false for the revisions
  that change nothing outbound-visible.
- **The store's continuity is not an implementation detail.** The event log is the analysis truth and the
  projections are its rebuildable mirrors; a switch that started a generation or carved a revision column
  would put the reload inside machinery whose invariants (CONF-21, CONF-24) are already asserted. The
  reload stays outside the store's model and writes one row into it.

## Consequences

- **A new contract section and a new design section, docs-only.** spec **§4.15** states the behaviour a
  user can rely on (the change takes effect without a restart, what a refusal does, what the store keeps,
  what is refused because the process resolved it once, and that there is nothing to configure);
  DESIGN **§12.20** states the landing (the publish sequence, the capture-once seam, the reloadable set,
  the invariants) and DESIGN §12.10.5 gains **note R9** beside row 13, which is the row whose second
  trigger the reload is.
- **ADR-039's D4 rows close.** Its row 1 (the atomic-swap semantics) is D2–D4; its row 2 (the state store
  across a switch) is D6; its row 4 (*which module owns the watcher, and whether it debounces*) stays
  open for 0b. Its row 3 (the fresh p99 ladder) and row 5 (how the watcher survives the `rename`) remain
  where they were: the owner's and 0b's respectively.
- **What the implementing round inherits as work, not as invention:** the reload's own loop and its
  second `load` entry point in `router-cli`; the published handle and the capture-once seam in
  `router-proxy`; the delta-only plugin mount, which needs the keyed diff DESIGN §12.2 sketches and
  nothing yet implements; the debounce (0b); and the rig that drives a landing the reload performs
  itself — a `temp`+`rename` of the pair, not a hand edit (ADR-039's own consequence for the platform
  backends).
- **Nothing about the request path's *bytes* moves.** No mutation is added, no field is stripped
  differently, no ordering changes: the reload changes *which* configuration is in force, never what a
  configuration does to a request. The byte-identity arm a verification card can run is therefore the
  narrow one D7.2 names (a same-bytes-serving revision is prefix-neutral), not a new byte assertion.
- **No gate, no corpus, no conformance assertion and no L1-envelope value moves** (AGENTS 9 / ADR-012).
  §12.8's allocation is untouched: this ADR names assertion *intent* (RV-1…RV-6) and allocates no ID.
- **STATE.md's waiting-on-human row 17 is now fully answered at the contract level**, and this ADR does
  not edit `STATE.md` — the round's close-out records it. The row's remaining live part is the owner's
  ruling on the question above.

## Honest boundaries

- **Nothing here is served at the time of writing.** There is no watcher, no second `load` entry point,
  no published handle and no keyed diff in the tree (all three measured above). Every sentence about
  behaviour is a contract to land, and no card may cite this ADR as "the reload works".
- **The two-revision window's bound is arm-dependent and one arm is not chosen.** D3 states both bounds;
  which one is real depends on the owner's ruling.
- **The reloadable set is a judgement, and it is short on purpose.** `server.addr`, `trace.dir` and the
  state path are refused because the process resolved them once; a reader who disagrees should note that
  relaxing the set is a *smaller* change than tightening it, and that the refusal names the key either
  way.
- **D7.2's prefix-neutrality claim is a claim about a *rig* that does not exist yet.** It is stated as an
  assertion with its measurement (identical `prefix.blocks[].hash`, `cache_control_breaks == 0`) so a
  later card can refute it; nothing in this ADR was measured through a live upstream.
- **No price, quota, URL or saving figure is decided, quoted or changed here**, and none may be derived
  from this ADR (AGENTS 4 and 5). The AGENTS.md cache figures quoted in D7's framing are the repository's
  own recorded measurement (960 → 14 400 cached tokens, 99.2 %), cited as the *reason* the question is
  asked, not as this ADR's evidence.

## Reversibility

**Every decision here is reversible inside `router-cli`'s and `router-proxy`'s own code, and none of
them is stored.** Removing the watcher returns the tree to a process that serves what it loaded at
startup, exactly as today; the identity, the trace field and the `config.applied` row all predate the
reload and outlive its removal. The pin arm, if chosen, is memory-only and dies with the process, so
choosing it and later abandoning it leaves no migration — which is the strongest argument for keeping it
non-durable (D6). The reloadable set can be widened (a key moves from "refused" to "applied") without
touching anything else, and narrowed only at the cost of a refusal a running process already knows how
to produce. The one decision a later round must revisit rather than inherit is the **watched set**:
adding the transform rule files (or a second roster) to the watch set changes what
`config_digest` is computed over, and that is ADR-037's own reversibility note — a decision for the round
that wants it, with its own ADR.

## Evidence (all re-runnable; none of it is an estimate)

```
# the facts this ADR rests on, at this ADR's commit
bash autowork/harness/r47-0a/probe.sh > autowork/harness/r47-0a/probe.out   # P1..P17, raw output tracked

# the register's last number (040 is free) and the two free section numbers
ls design/decisions/ | sort | tail -1              -> ADR-039-file-watch-crate-for-the-reload.md   (39 ADRs)
grep -n '^### 4\.15' docs/spec.md                  -> no output   (free)
grep -n '^### 12\.20' design/DESIGN.md            -> no output   (free)

# no reload machinery, and the assembly says so in its own words
git grep -n -i -e reload -e notify -e watcher -- 'crates/**/*.rs'
  -> crates/router-plugins/src/assembly.rs:276-277  "built once at start-up and never reloaded ... no config-diff, no live reload"
  -> crates/router-runtime/src/loader.rs:807        a_reloaded_provider_reactivates_its_dependents (the runtime can re-load a fiber)
git grep -n apply_config_diff -- crates/           -> no output   (DESIGN 12.2's keyed diff is a sketch, not code)
git grep -n notify -- Cargo.toml crates/*/Cargo.toml -> no output (the dependency has not landed; ADR-039 D3)

# no store table is keyed by the configuration
git grep -n -e 'CREATE TABLE' -e 'PRIMARY KEY' -- crates/router-store/   # 7 tables: event_id, session_key,
  # (session_key, block_index), (provider, plan_idx, window_start_us), (scope, provider, model), family
git grep -n config_digest -- 'crates/**'   # trace field, config.applied payload, /health, the trace writer

# the gate's two consequences: startup exits, a reload may not
git grep -n -e 'return 2;' -e 'return 3;' -e 'return 4;' -- crates/router-cli/src/lib.rs   # 2 config, 3 bind, 4 env/store
crates/router-cli/src/lib.rs:212   -> the only append of EventKind::ConfigApplied in the tree (startup)

# the landing is atomic per file (the torn-pair case is two renames, not one)
grep -n rename docs/spec.md   -> spec 4.11: temp file "flushed, and is then renamed over the target
  # (one atomic replace on one filesystem) ... Nothing lands partially"
```

The probe script and its raw output are tracked at **`autowork/harness/r47-0a/probe.sh`** and
**`probe.out`** (flat names, one `git ls-files` away in a fresh clone), per `autowork/work-mode.md`'s
evidence rule (*"every number a claim rests on must resolve in a fresh clone"*).

## Dated note — 2026-09-26, the entering card's premise, corrected rather than inherited

The entering card states two premises that the tree does not support, and a contract may not repeat an
unverified one. Both are recorded here, in ADR-039's own dated-note form, so the audit (R47-0c) does not
have to rediscover them.

1. **"The ledger is keyed by the configuration identity (ADR-037 D2)."** ADR-037 **D2** is *"the roster
   is named, never searched"* — it is about how `providers_file` resolves, and it says nothing about a
   ledger. Measured: **no store table is keyed by the identity** (the seven tables' primary keys are
   listed above), and the identity's carriers are the trace field, the `config.applied` payload and
   `/health`'s member. What is keyed by the configuration is nothing — deliberately, because ADR-037
   **D6** rules the identity *attribution, not a score*, and forbids it becoming a key. D6 of this ADR
   is therefore written from the measured state (one ledger, no key) rather than from the card's
   premise, and RV-6 is the invariant that keeps it that way.
2. **"a switch that perturbs `tools` or their order silently raises cost."** The tool schemas are the
   client's bytes; the configuration cannot reach them (AGENTS 1, §12.10.7). The configuration's real
   outbound-visible levers, and the reload's watched pair, are D7's statements 2 and 4.

Nothing else in the entering card was contradicted: the mechanism (ADR-039), the loader-as-gate reading
(spec §4.11), the owner's ownership of A1's user-visible half, the no-new-flag ruling, and the evidence
homing rule all hold as stated.
