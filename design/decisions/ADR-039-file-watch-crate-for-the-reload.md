# ADR-039 — the reload's file-watch mechanism is a crate (`notify`): ADR-037 D7's mechanism half, decided by the owner

- Status: accepted
- Date: 2026-09-26
- Related: **ADR-037** — the decision this one completes: **D7** leaves the reload to its own round and
  names its pre-requisites, the first of which is the mechanism (a file-watch crate *versus* a std-only
  mtime poll); D6 (the byte-digest identity) is the value the reload re-reads, and D4 (*named, never
  searched*) is why the watcher has two known paths rather than a search; **ADR-036** (the house shape
  this ADR follows for the allowlist boundary) and **ADR-012** (the measurement is not the search space —
  this ADR moves no gate, no corpus and no conformance assertion); **ADR-025** (the writer whose landing is
  an atomic `rename` — the fact D4's obligation below is about); **ADR-002** (the runtime's declarative
  loader and its keyed config diff — machinery a revision switch *may* feed, which this ADR does **not**
  decide); AGENTS hard constraints **2** (content determinism), **8** (docs before code), **9** / ADR-012;
  spec §4.11 (the writer), §4.14 (the pair the watcher watches), §6 / §9.1 (the identity the reload
  re-reads); DESIGN §12.1 (the allowlist row this ADR adds and the dependency discipline it obeys), §12.2
  (the runtime's config-diff arm), §12.14 (the writer's landing).
- Numbering note: the register holds **38** ADRs (`ADR-001` … `ADR-038`), so **039** is the next free number.
- Scope note: **this ADR writes no code and moves no manifest.** It decides a mechanism and states the
  contract a dependency must satisfy; the dependency itself lands with the reload round's own cards, and
  `Cargo.toml` at this commit does not name the crate (measured below). Every sentence about the tree is a
  measurement taken on 2026-09-26 at `e72e406`.

## Background

**The item this ADR closes is one line long, and it has been waiting since R43.** ADR-037 **D7**
(`design/decisions/ADR-037-roster-file-and-config-identity.md:221-231`) puts the reload in its own round
and names its pre-requisites in terms: *"its named pre-requisites are the watcher decision (a file-watch
crate — a change to the dependency allowlist of DESIGN §12.1, and therefore a **human** decision — versus a
std-only mtime poll), the atomic-swap decision, and a fresh p99 ladder run."* `autowork/STATE.md`'s
waiting-on-human **row 17** carries the same item, registered by R43's close-out and deliberately not
decided by it.

**Why it was the owner's and not a round's.** A new third-party dependency is a change to the allowlist
that `design/DESIGN.md` §12.1 *is*, and `Cargo.toml:30-31` states the rule in the manifest itself: *"The
dependency whitelist = DESIGN §12.1. New dependencies must first be reconciled against the whitelist, with
the rationale stated in the commit message."* A round may not spend that budget on itself.

**The owner's ruling (2026-09-26), recorded as this round's opening card states it: a file-watching crate,
not a std-only mtime poll.** The wording here is the entering card's; no verbatim quote of the ruling is
invented in this ADR. What the ruling settles is the **mechanism** — an OS-level filesystem-notification
crate enters the allowlist — and that is the half of D7 this ADR resolves. What it does not settle is
everything D7 listed beside the mechanism (D4 below).

**What the reload needs, so the choice is checkable.** The reload's job is to notice that the effective
configuration changed and re-read it. The identity that decides *whether* it changed already exists and is
R43's (ADR-037 D6): `config_digest = sha16(root_sha16 + ":" + roster_sha16)` over the two files' **bytes**
(spec §4.14). The watcher therefore decides only *when to look* — never *what the answer is*. That is the
property this ADR's decision is chosen against: the mechanism may be lossy about events, and the digest is
what may not be.

## Decision

### D1 — The mechanism is `notify`, at its current stable line, and the version is measured rather than assumed

The crate is **`notify`** (the cross-platform filesystem-notification library,
<https://github.com/notify-rs/notify>), taken at its **stable 8.x line**; the version a lockfile resolves
today is **8.2.0**, and the 9.x line is a **release-candidate** series (`9.0.0-rc.5`), so it is not the
choice. Measured 2026-09-26 against crates.io and against a scratch consumer crate (commands in
*Evidence*; no figure below is estimated):

| Fact | Value | Why it is stated |
|---|---|---|
| crate / resolved version | `notify` / **8.2.0** … `notify = "8"` resolves to it | the row DESIGN §12.1 gains says a *crate*, not a version; the version is what this measurement found |
| MSRV (crates.io `rust_version`) | **1.77** | ≤ the workspace's floor, `rust-version = "1.88"` (`Cargo.toml:28`) — the floor is a *measured* minimum and a dependency can raise it |
| MSRV of the **whole resolved set** | **1.85** (the highest: `notify-types` 2.1.0) | the floor is decided by the tree's maximum, not by the crate: `1.85 ≤ 1.88` holds today, three minors of head-room, and the reload round re-measures it with `cargo +1.88.0 build --workspace --locked` |
| licence | `notify` **CC0-1.0** (a public-domain dedication); the rest of the set MIT / Apache-2.0 / ISC / `Unlicense OR MIT` | all permissive; nothing in the tree is copyleft, and the product's own licence (Apache-2.0) is unaffected |
| resolved tree, macOS | **8 packages** (notify + bitflags, fsevent-sys, libc, log, notify-types, walkdir, same-file) | the platform backend on the development machine is FSEvents, via `fsevent-sys` |
| resolved tree, `x86_64-unknown-linux-gnu` | **10 packages** (that set with `inotify`, `inotify-sys` and `mio` in place of `fsevent-sys`) | the CI/container side is inotify; the two backends are the crate's, not a second code path of ours |
| default feature | **`macos_fsevent`** (on macOS); the Linux arm needs no feature | taking the crate with its defaults is the choice; `macos_kqueue` is the crate's own alternative backend and is **not** taken |
| `unsafe` in the resolved sources | `notify` **26** occurrences (2 of 9 files), `fsevent-sys` **1** (1 of 3), `libc` **676** (66 of 389), `notify-types` / `walkdir` **0**; on Linux `inotify` 24, `mio` 176, `inotify-sys` 0 | the unsafe is the platform-call boundary, and `libc` is **already in this workspace's tree** (measured: `cargo tree -p router-cli --locked -i libc`), so the marginal unsafe is the binding crate plus `notify`'s own FFI calls |
| crate_size / edition | 39 067 B / edition 2021 | the crate is small and does not move the workspace's edition |

**What `#![forbid(unsafe_code)]` does and does not mean here.** Every crate root in this workspace forbids
`unsafe` **in this repository's own code** (DESIGN §12.1). A dependency's unsafe is not a violation of
that: it is the FFI boundary such a dependency exists to hold, and the reload's own module stays inside
the forbid.

### D2 — The std-only mtime poll is refused, and the criteria it fails are stated rather than felt

The poll is not refused because it is "cruder". It is refused because it trades a **hidden latency** and an
**unbounded false-negative** for the cost of one dependency. The four criteria, each one a thing the
reload must satisfy and the poll cannot:

1. **The p99 belongs to the change, not to a timer.** A reload's latency budget (the fresh ladder D7 names)
   would be a function of a poll interval the operator never sees: either the interval is short and the
   process wakes on a schedule forever, or it is long and the budget is spent on waiting. A notification
   arrives when the change does, so the ladder measures the reload's own work.
2. **The identity decides, so a missed event must not be a silent revision.** A stat-tuple comparison
   (`mtime`, size) cannot distinguish "unchanged" from "changed in a way the tuple does not show" — a
   same-second, same-size rewrite is invisible, and the load path §4.11 uses *replaces the inode* by
   `rename`, which is a different axis from the tuple. With a notification the event is the trigger and the
   **digest** is the decision; the mechanism's lossiness lands on *when we look*, not on *what is true*.
3. **An event exists; a torn read does not.** The landing is a temp file plus `rename` (spec §4.11), so the
   watcher can be told about the replacement instead of reading a file another process is midway through
   writing. A poll has to re-read and hope the digest stabilizes, and it has no way to say "not finished".
4. **The interval would be a documented knob with a contract, and we do not want one.** A poll needs a
   value, a default, a tuning story and a test that pins its behaviour; a notification needs none. That
   argument is the same one §12.5's defaults row makes from the other side: only what is decided gets a
   value.

**What the poll would have cost us is real, and is not hidden:** one fewer dependency, no allowlist change,
no platform backends, no FFI in the tree. That is the price of D1, and the four criteria above are why the
owner's ruling paid it.

### D3 — The allowlist row is the contract; the dependency lands with the reload round's code

**The row.** DESIGN §12.1's table gains `notify` in **`router-cli`**'s *Permitted third-party
dependencies* cell — the crate that owns `serve`'s assembly and the config loader, and therefore the crate
that may hold a watcher. It is added to `router-core`'s cell **not at all**: `router-core` is the I/O-free
domain, `cargo tree -p router-core` must stay ⊆ its allowlist (§12.1's own spot-check), and a watcher is I/O
by definition. The row carries this ADR's number as its reason, as the `rusqlite` and `reqwest` rows carry
ADR-009 and §12.1's older rows.

**The manifest comment shape**, for the round that lands it — the same shape the other workspace entries
use (`Cargo.toml`), written here so the reload round does not have to invent prose:

```toml
# notify (the file-watch mechanism): DESIGN §12.1's allowlist row for router-cli,
# added by ADR-039 — the watcher decision ADR-037 D7 left to the owner's ruling
# (2026-09-26). The reload re-reads the pair spec §4.14 names and decides by the
# byte digest (ADR-037 D6); this crate decides only when to look.
notify = "8"
```

**What lands when.** This ADR and the DESIGN row land **now**, docs-only. The `Cargo.toml` entry, the
`crates/` code, the tests and the measured floor (`cargo +1.88.0 build --workspace --locked`) land with
the **reload round's own cards** — one dependency, one commit, one stated reason, exactly as
`Cargo.toml:30-31` requires. Nothing in this ADR may be cited as "the reload landed".

### D4 — What this ADR does **not** decide

The mechanism is one half of D7. Everything else D7 named stays where D7 left it, and the reload round's
own cards own each of them:

| Not decided here | Why it is not this ADR's |
|---|---|
| **the atomic-swap semantics** — what a revision switch *is*: which structures are replaced, how the old revision stops being used, what an in-flight request sees | D7's own next item. This ADR says *how the change is noticed*, never what is done about it |
| **what a revision switch does to the state store** — the event log's continuity, whether a switch writes an event, how the projections and the session bindings relate to a revision | the state store's contract is ADR-009/ADR-010's, and a reload that touches it is a decision no watcher can make |
| **the fresh p99 ladder run** (the reload's latency budget, measured) | D7's third item. The watcher's arrival latency is a fact of the platform's notification path, and only a measurement may state it |
| **the "changed keys" half of the `config.applied` payload** (DESIGN §12.10 row 13) | a payload of the event log, i.e. the state store's vocabulary — the reload round's, registered in R43's own row 13 note |
| **which module owns the watcher, and whether it debounces** | the crate offers the raw watcher and a separate debouncing wrapper crate; a debounce is a *policy* (a window, a coalescing rule), and a second crate is a second allowlist decision. The reload round decides, and if it wants the wrapper it opens that decision |
| **how the watcher survives the landing's `rename`** | the landing replaces the inode (spec §4.11), so a registration that binds the old inode can go silent. This ADR's mechanism makes that failure *possible*; closing it is the reload round's obligation, and it is named here so it cannot be discovered twice by two cards |

## Alternatives considered

| Alternative | Why it is not the decision |
|---|---|
| **A std-only mtime poll** (`std::fs::metadata` on a timer) | **D2**: the four criteria. Its real cost — no dependency, no allowlist change — is why the owner had to rule, and the ruling went the other way |
| **`notify-debouncer-full`** (the wrapper: `notify` + `file-id` + a debounce policy) | it adds a second crate to the allowlist and buys a *policy* (a debounce window, event coalescing) that belongs to the reload's own semantics, not to the mechanism. Available, not taken; D4's last row names it |
| **`hotwatch`** (a thin convenience wrapper over the same backends) | an indirection over the same platform code, `0.5.0`, last published 2023-06, `rust_version` **unstated** on crates.io — a wrapper whose maintenance and MSRV must be taken on trust is a worse bet than the crate the whole ecosystem's watchers are built on |
| **Direct `inotify` / `kqueue` per platform** | two code paths in our tree, neither exercised on the other target, and each with its own edge cases — in exchange for nothing the crate does not already give |
| **No watcher: a signal (`SIGHUP`) or a management call** | the owner's ruling chose automatic-on-change; a signal makes the operator the timer, which is the poll's problem with a manual step added |
| **Watching a *derived* value instead of the files** (e.g. re-hashing on a request-path hook) | this is the byte-boundary-adjacent failure: a request-path read of configuration makes the request's cost a function of the file system, and AGENTS constraint 2's determinism argument is about exactly this class. The reload must be its own loop, never a per-request check |

## Rationale

- **The mechanism and the decision are separable, and this ADR keeps them separate.** A watcher answers
  *when to look*; the digest answers *whether anything changed*; the swap answers *what happens next*.
  Choosing the first by the cost of the third would be choosing twice at once.
- **A dependency is a contract, so it gets a row.** The allowlist exists so that a dependency is a
  *decision* with a reason in a dated record and a line in the commit message, not a line in a manifest.
  This ADR is that reason; the reload round writes the line.
- **The measurement is the argument.** MSRV, tree size, licences and unsafe counts are all read off the
  resolved set rather than asserted from the crate's reputation — including the two numbers that cut
  *against* the crate (`notify-types` 1.85 as the set's floor, and 26 `unsafe` occurrences in `notify`
  itself).

## Consequences

- **One more dependency, permanently.** `router-cli`'s allowlist gains a row; the workspace's floor may
  move when the crate's own minor versions move (1.85 today against a 1.88 floor), and the reload round's
  gate re-measures it rather than inheriting this ADR's reading.
- **Two platform backends become ours to test.** FSEvents on macOS and inotify on Linux are the crate's
  implementations, but their *observable* differences (coalescing, delivery order, the case where the
  watched name is replaced rather than modified) are ours to know. The reload round's rig must therefore
  drive a landing it performs itself — the `rename` of spec §4.11 — and not only a hand edit.
- **A false negative must be survivable, and that is a design obligation.** With the digest as the
  decision, a missed event costs a revision until the next look; the reload round must say what the next
  look is (the next event, a bounded re-check, or a startup-time reconciliation), because "we missed it and
  never looked again" is the silent failure this mechanism makes available.
- **Nothing about the request path, the gates, the corpus or the shipped config moves.** No `crates/` byte,
  no manifest line, no conformance assertion and no measurement definition is touched by this ADR
  (AGENTS 9 / ADR-012). The reader side of §4.14 and the writer of §4.11 are untouched: a running process
  still serves the configuration it loaded, and the reload is one round away from being observable.
- **STATE.md's waiting-on-human row 17 moves only in part** — its *mechanism* half is now decided, and its
  remaining half (the swap, the store, the ladder) stays open. This ADR does not edit `STATE.md`; the
  round's close-out card records the row's partial closure.

## Evidence (all re-runnable; none of it is an estimate)

```
# the crate, its licence, its MSRV and the release-candidate line
GET https://crates.io/api/v1/crates/notify               -> max_stable 8.2.0, max_version 9.0.0-rc.5
GET https://crates.io/api/v1/crates/notify/8.2.0         -> license CC0-1.0, rust_version 1.77, edition 2021
GET https://crates.io/api/v1/crates/notify/8.2.0/dependencies -> 15 normal deps, default feature macos_fsevent

# the resolved tree, from a scratch consumer crate (`notify = "8"`, nothing to do with the workspace)
cargo tree -e normal                              -> 8 packages on macOS (notify 8.2.0 + 7)
cargo tree -e normal --target x86_64-unknown-linux-gnu -> 10 packages (inotify/inotify-sys/mio in place of fsevent-sys)

# the MSRV of the whole set, per package, read from the resolved Cargo.lock -> max 1.85 (notify-types 2.1.0)

# the unsafe counts, per vendored crate, over ~/.cargo/registry/src/**: notify 26, fsevent-sys 1,
# libc 676, inotify 24, mio 176, inotify-sys 0, notify-types 0, walkdir 0 (script: unsafe_count.py)

# libc is already in the workspace's tree, so it is not a marginal addition
cargo tree -p router-cli --locked -i libc          -> libc v0.2.189 (via sha2/cpufeatures, ...)

# the dependency is NOT in any manifest at this commit
git grep -n notify Cargo.toml crates/*/Cargo.toml  -> (no output)
```

The probe scripts and their raw output are tracked at **`autowork/harness/r46-0/`** (flat names, one
`git ls-files` away in a fresh clone); the numbers above are also reproducible from the commands, which is
the point of writing them down (`autowork/work-mode.md`'s evidence rule).

## Dated note — 2026-09-26 (the R46-0b audit, card `t_d8a3e014`)

Two corrections, both re-measured against live sources on the same day rather than trusted:

1. **The resolved Linux tree is 10 packages, not 12.** Re-measured with a fresh scratch consumer crate
   (`notify = "8"`): `cargo tree -e normal --target x86_64-unknown-linux-gnu` resolves `notify` + `bitflags`,
   `inotify`, `inotify-sys`, `libc`, `log`, `mio`, `notify-types`, `same-file`, `walkdir`. Every *other*
   crate fact in D1 reproduced exactly — 8.2.0 as max_stable against a `9.0.0-rc.5` line, licence
   **CC0-1.0**, `rust_version` **1.77**, the set's MSRV max **1.85** (`notify-types` 2.1.0), `crate_size`
   39 067, edition 2021, **15** normal deps, default feature `macos_fsevent`, and every `unsafe` count
   (notify 26 in 2 of 9 files, fsevent-sys 1, libc 676, inotify 24, mio 176, inotify-sys / notify-types /
   walkdir 0).
2. **The bundle sentence above originally read "the R46-0 card's attachments" — and that card has none.**
   The orchestrator's attach attempt was stopped by the approval gate, so the raw bundle (which lived only
   in a pruned scratch directory) was landed here in the tree by the audit instead, per
   `autowork/work-mode.md`'s rule that a fresh clone must resolve every number's evidence.
