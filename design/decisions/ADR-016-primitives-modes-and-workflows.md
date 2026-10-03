# ADR-016 — primitives, modes and workflows: the system's capabilities, the compositions built on them, the tasks that use them, and the decision-provider seam

- Status: accepted
- Date: 2026-09-21
- Related: ADR-002 (the Cordis plugin runtime; tier-B isolation), ADR-003 (a revertible, declarative, individually accounted transform pipeline), ADR-004 (native passthrough first; v0.1 keeps no server-side state), ADR-005 (the trace is the only product↔analysis-loop channel), ADR-006 (integer NanoUsd), ADR-007 (span-faithful forwarding; no parse → reserialize), ADR-008 (rule override; the trust gate is not enabled), ADR-009 (one local store; projections are not the truth; the tiered durability), ADR-010 (the event log is state truth; write ahead, then execute), ADR-011 (one classifier; the action set; the pattern tables are code), ADR-012 (the self-improvement ladder and the never-mutable paths), ADR-013 (shadow → session-bucketed canary → automatic rollback), ADR-014 (plan-first: sticky account, upstream-only authority, session-boundary probe, `plan.switched`), ADR-015 (the byte boundary permits exactly two mutations); AGENTS hard constraints 1–6 and 8; spec §1 (non-goals), §2 (the protocol contract), §3 (selection semantics and the reserved `Selector` slot), §4 (config), §4.4 (rule files), §4.5 (state), §4.6 (plan-first), §4.7 (inbound auth), §6 (observation), §7 (the accounting convention), §8 (degradation and error behaviour), §9.1/§9.2/§9.3 (the reporting surfaces, and the ones deliberately not served); DESIGN §2 (dependency direction), §3 (the pipeline), §4 and §12.2 (the plugin runtime), §5 (the cost engine), §6 (cache policy), §7 (the translation layer), §8 (state), §9 (replay), §10 (test strategy), §11 (risks), §12.3 and §12.3.1 (the pipeline types and the byte boundary), §12.4 (cost/quota/breakeven), §12.5 (config parsing), §12.6 (DecisionRecord), §12.7 (error semantics), §12.8 (the CONF table), §12.10.2 (config landing), §12.10.4 (Store, events, projections), §12.10.5 (event wiring), §12.10.7 (the outbound `model` rewrite), §12.10.8 (the plan-first landing), §12.11 (inbound token auth), §13 (**new**: the register, the module → primitive map and the leak register this ADR rules); the loop charter (the gates), the loop execution model (one card, one worktree, the freeze-before-fan-out rule), the loop's tree (R5-G5, the finding this ADR generalises)

## Background

The repository is described today in three registers, and this ADR adds a fourth.

`docs/spec.md` says **what a client can observe**. `design/DESIGN.md` says **how the product is structured**
(crates, data flow, algorithms, the landing lists). `design/decisions/ADR-NNN` says **why a choice was made**.
What is written nowhere is the middle layer: **which capabilities this system has as capabilities** — the
properties that hold whatever feature is being built, that more than one feature depends on, and that degrade
*silently* when they are restated instead of used.

That omission already has a price at `HEAD` (`76afd81`, i.e. the commit this ADR was written against), and it is
not hypothetical:

1. **One policy predicate has four re-derivations.** The plan policy's probe gate is implemented once in the
   decision core (`crates/router-core/src/plan.rs:151-178`, `PlanFirstRule::probe_admitted`) and re-derived, arm
   by arm, in the reporting surface (`crates/router-proxy/src/health.rs:182-199`, the `blocked_by` chain).
   Beside it: the cooldown's ms→µs conversion (`plan.rs:232-235` vs `health.rs:228-230`), ADR-011's
   route-availability read (`forward.rs:1308-1318` vs `health.rs:262-273`) and the local counter's window
   verdict (`forward.rs:1201-1243` vs `health.rs:279-320`). The copies are not equal: two of them take a
   different clock unit, and two read the store through a different owner (`Forwarder::store` vs
   `AppState::store`, `crates/router-proxy/src/health.rs:24-28`). R5 found one of the four and ledgered it as
   R5-G5; the class is four, and no register says so.
2. **One resolution rule has two implementations.** `Forwarder::resolve_route`
   (`crates/router-proxy/src/forward.rs:1270-1306`) and the free `resolve_route`
   (`crates/router-proxy/src/stream_forward.rs:981-1017`) resolve an alias/explicit route with the same 404
   codes and the same message strings, written twice. The capability check beside them is duplicated the same
   way (`forward.rs:491-507` vs `stream_forward.rs:332-345`), each rendering its own error body.
3. **Interfaces the documents name and the type system does not have.** DESIGN §12.3 sketches `trait Selector`
   and `trait Guard` + `GuardOutcome` (`design/DESIGN.md:367,368-369`); neither exists in `crates/`
   (`grep -rn "trait Selector\|trait Guard\|trait Transform" crates/` returns nothing). The comment in the code
   that means to use the second one refers to it as existing vocabulary
   (`crates/router-core/src/plan.rs:3-5`) while the type it actually answers with is plan-specific
   (`PlanMove`, `plan.rs:52-69`), and the caller is a hand-written method on the forwarder
   (`forward.rs:1121-1200`). Meanwhile `auto` is refused by a literal string comparison
   (`forward.rs:403-407`) even though spec §3 says the slot is "reserved structurally"
   (`docs/spec.md:78-79`) and `decision.selection_source` is a bare `String` (`crates/router-core/src/trace.rs:88`)
   whose third value (`Plugin`, DESIGN §12.3) has no producer and no entry in spec §3's own list.
4. **Capabilities that exist in prose and not in code.** The transform chain (ADR-003, spec §4.4, DESIGN
   §12.3) has no implementation: `crates/router-plugins/src/lib.rs:1-4` is a four-line stub and every record
   built today carries `transforms: Vec::new()` (`crates/router-proxy/src/accounting.rs:483`,
   `crates/router-proxy/src/auth.rs:170`). The plugin runtime (ADR-002, DESIGN §4/§12.2) is the same:
   `crates/router-runtime/src/lib.rs:1-4` is a stub, `inject`/`isolate`/`intercept` are parsed
   (`crates/router-core/src/config.rs:825-840`) and validated (`config.rs:1232-1261`) but never consumed —
   the only reader of `config.plugins` outside validation is `/health`'s listing
   (`crates/router-proxy/src/health.rs:34`) — while ADR-013's shadow and canary rails are built on
   `ctx.isolate`/`ctx.intercept`.
5. **The converse: an implemented capability with no name.** Inbound admission (spec §4.7) landed in R6 as a
   guard above the path split that reads headers, writes no bytes and leaves one pre-pipeline record
   (`crates/router-proxy/src/auth.rs:25,34,49,121`; wiring `crates/router-cli/src/lib.rs:344-375`). It is a
   boundary of the same rank as the byte boundary, and nothing in the vocabulary says so.

Two failure modes follow, and the vocabulary exists to make both visible rather than remembered:

- **Silent drift.** A capability implemented twice does not fail; it disagrees, later, in a way that looks like
  a bug in one place. The `/health` chains are the worked example: they are unit-tested per arm
  (`crates/router-proxy/src/health.rs:437-466`) so they agree *today*, and nothing binds them to the guard's
  order tomorrow.
- **No seam where a seam is wanted.** The upstream-error classifier's pattern tables are constants inside
  `crates/router-core/src/error_class.rs` (`161`, `172`, `180`, `185`, `193`, consumed by
  `classify_upstream_error`, `error_class.rs:226`). This is deliberate — ADR-011 item 11 rules the tables *code,
  not an auto-adoptable artifact*, and ADR-012's L3 covers a change to `crates/` — but the consequence is
  specific: an external decision provider that wants a different taxonomy must edit `router-core` and get a
  human merge. There is no seam *at* the classifier, so a seam has to be placed **above** it: a provider may
  consume classes, never contribute patterns.

This ADR names the three layers, registers the primitives with their invariants, maps the modules onto them,
records the leaks with `file:line`, and freezes the one seam the layering implies but does not yet have: a
**decision provider**.

## Decision

### 1. The three layers, defined by the question each answers

| Layer | Answers | Unit of work | Owns | May a new one be added without a contract change? |
|---|---|---|---|---|
| **Primitive** (a system capability) | "what is true of this gateway whatever it is being used for?" | one capability with one invariant, one owner module, one contract home | an invariant of the system | **No.** A new primitive is a human decision with an ADR (this one is written under that rule) |
| **Mode** (an organisational ability) | "how does the gateway behave over time, across requests, under a condition?" | a named composition of primitives: an order, a state, a policy — never new content | how requests are routed, retried, spilled or experimented on | **No.** A new mode is an ADR or a spec section (ADR-013 and ADR-014 are modes stated as contracts) |
| **Workflow** (completing a concrete task) | "what is someone trying to get done?" | a sequence a user or the analysis loop performs | config, and its own steps | **Yes** — and that is the test of the vocabulary |

Three composition rules, stated so they can be enforced by review:

1. **Dependencies run one way: workflow → mode → primitive.** A mode composes primitives; a workflow composes
   modes and config. Nothing may implement a primitive inside a mode, and nothing may implement a mode inside a
   workflow. A workflow that needs a primitive it cannot name is evidence that the register is incomplete, not
   licence to implement it in place.
2. **A primitive has exactly one implementation of its invariant.** A second implementation is a **leak**, it is
   named by `<primitive id> re-derived at <file:line>`, and it is registered in DESIGN §13.3 with the same
   standing as any other defect. (This is the rule R5-G5 should have been expressed as.)
3. **Everything a mode does must be attributable to a primitive.** If a mode's behaviour cannot be traced to a
   registered primitive, the register is missing a capability — which is how item 5 of the Background (inbound
   admission) becomes `P2` rather than a footnote.

### 2. The primitive register

Nine primitives: five carried by the boundary and the decision path, four by the substrate. Each row states what
it guarantees, where the contract lives, where the code lives, who may change it, and what a violation causes.
`state` distinguishes **wired** (implemented and exercised at `HEAD`) from **contract-only** (the contract is
frozen, the code is not written).

| id | primitive | what it guarantees (the invariant) | contract home | code home | who may change it | a violation causes |
|---|---|---|---|---|---|---|
| **P1** | `byte-fidelity` | For every request, the bytes the upstream sees are the client's bytes **modulo exactly two mutations** (ADR-015): removing router-owned top-level fields, and replacing the *value* of the top-level `model` member. Both are span edits over `RawBody`; a parse → reserialize round trip is forbidden anywhere on the path. | AGENTS 1; ADR-007; ADR-015; spec §2; DESIGN §12.3.1, §12.10.7 | `crates/router-core/src/body.rs:29,56,90,197`; call sites `forward.rs:223,528`, `stream_forward.rs:365`, `prefix.rs:397` | the contract files only: a third mutation or a new whitelist key is a human contract change (ADR-015), never a local decision | **silent** cache loss (a re-prefill is charged and never explained) and a protocol-fidelity gate failure; state: wired |
| **P2** | `inbound-admission` | Exactly one guard, above the path split and above the pipeline, decides whether a request enters: it reads headers only, touches no byte of the request, and a refusal leaves exactly one pre-pipeline record (`event_id: 0`) and no store row. | spec §4.7, §9.1; DESIGN §12.11 | `crates/router-proxy/src/auth.rs:25,34,49,121`; wiring `crates/router-cli/src/lib.rs:314,344-375` | spec §4.7 (human); a second admission rule (rate limiting, lockout) is a different capability with its own contract | an unauthenticated request entering the pipeline, or an admitted request whose bytes changed; state: wired |
| **P3** | `resolution` | Exactly one route per request, derived from the roster by explicit `provider/model` or by an alias; the resolved provider-native id is what the outbound body carries (P1's mutation (b)); anything else is a 404, `auto` is a 400, and an inbound protocol outside the route's `supports` is a 400 (no best-effort translation). | spec §3, §4, §8; DESIGN §3, §7, §12.3 | `crates/router-core/src/config.rs:226` (`RouteSpec`), `:746` (`supports`), `:872` (aliases); `forward.rs:403-407,491-507,1270-1306`; `stream_forward.rs:332-345,981-1017` | spec §3/§4 (human): a new selection form (including `auto` becoming real) is a contract change | two transports resolving the same request differently, so `decision.model` depends on which path served it; state: wired, **with the leak L2/L3/L4** |
| **P4** | `policy-guard` | Every route choice or refusal is decided by a **pure** predicate over (the resolved route, the projections, stable config, one clock read), whose evaluation order is normative, whose vocabulary names a route or a refusal and nothing else, and where a state transition may only follow upstream evidence (ADR-014 item 2). | spec §4.2, §4.6, §8; ADR-011; ADR-014; DESIGN §12.3, §12.4, §12.10.8 | `crates/router-core/src/plan.rs:109,151,183` (`PlanFirstRule`), `error_class.rs:226,288,359` (the classifier), `quota.rs` (`charge`), `breakeven.rs` (`decide_switch`); consumers `forward.rs:1121-1200` | spec §4.6/§8 + ADR-011/ADR-014 (human); the pattern tables are code by ADR-011 item 11 and may not become a config surface | two evaluations disagreeing about *why* — the operator reads a different system from the one running; or a refusal the local counter was never allowed to make (ADR-014 item 2); state: wired, **with the leak L1** |
| **P5** | `decision-record` | Every request leaves exactly one record on the trace: additive fields keep `schema_version` (present-and-null, never omitted), the record joins the state truth on `request_id` + `identity.event_id`, a write failure degrades the observation and never the request, and the record is the **only** product → analysis-loop channel. | ADR-005; spec §6, §7; DESIGN §8, §12.6 | `crates/router-core/src/trace.rs:22,27,88,311`; the record's single writer `crates/router-proxy/src/accounting.rs:358`; the sink `crates/router-store/src/trace_sink.rs:40,73` | spec §6 (human): a field group is a contract change; ADR-005's boundary has no code-local exception | an unreplayable decision, or two writers disagreeing about one request; state: wired |
| **P6** | `transform-chain` | (contract) Every content change is a pure function of (content, stable config) — never of turn number, clock or RNG — individually accounted with its cache impact and its `verified`/`inferred` label, with an inverse, and with a prefix that stays a prefix. | ADR-003; ADR-008; spec §4.4, §6, §7; DESIGN §6, §12.3 | **none** — `crates/router-plugins/src/lib.rs:1-4` is a stub; every record carries `transforms: Vec::new()` (`accounting.rs:483`, `auth.rs:170`) | ADR-003 + AGENTS 1 and 2 (human) | an unaccounted saving, a prefix break attributed to nobody, or a transform that cannot be rolled back; state: **contract-only** |
| **P7** | `state-truth` | The event log is the truth and the only writer of it is the serving process; every projection is rebuildable and is never the truth; an intent commit precedes the effect it authorizes; the store is a startup prerequisite (no in-memory degraded mode); one state directory has one writer. | ADR-009; ADR-010; spec §4.5; DESIGN §8, §12.10.4 | `crates/router-core/src/store.rs:26,180,361,412,442`; implementation `crates/router-store/src/lib.rs:218` (`PRAGMA locking_mode = EXCLUSIVE`) | ADR-009/ADR-010 (human): a schema or migration decision is not a code-local choice | a projection believed, a charge with no intent row, or a second writer interleaving; state: wired |
| **P8** | `accounting` | Money is integer NanoUsd on the five tiers plus the peak multiplier; every figure carries `verified` or `inferred`; only `verified` may enter a gate or an external report; an absent measurement is never read as 0; no price or quota enters config without its source. | ADR-006; spec §7, §4.0; DESIGN §5, §12.4 | `crates/router-core/src/cost.rs:11,44,55`, `peak.rs`, `quota.rs`, `trace.rs:297` | spec §7 + ADR-006 (human); a price enters config only with its source URL and date | an inferred number in a gate, or a fabricated price presented as measured; state: wired |
| **P9** | `plugin-runtime` | (contract) Every registration carries its own inverse (accumulated LIFO; unloading a fiber runs them in reverse); a provider going away deactivates its dependents first; a realm gives the same key two independent binding sets; an intercept changes *how* a key is used without rebinding it; a config change applies as a keyed diff. | ADR-002; DESIGN §4, §12.2 | **none** — `crates/router-runtime/src/lib.rs:1-4` is a stub, `crates/router-plugin-sdk/src/lib.rs:1-4` is a stub; `inject`/`isolate`/`intercept` are parsed (`config.rs:825-840`) and validated (`config.rs:1232-1261`) but never consumed | ADR-002 + DESIGN §4 (human) | an unload that leaves half a registration; a realm leak that lets an experiment touch production traffic; state: **contract-only** |

Three facts the register makes explicit because each one changes what may be built today:

- **P6 and P9 are contract-only, and the modes that need them are named in item 5.** ADR-013's shadow and canary
  rails compose `ctx.isolate`/`ctx.intercept` (`crates/router-core/src/config.rs:816-840`); with P9 unwritten they
  cannot be built. This is not a defect of the register, but it *is* the reason the register must mark
  `state`: a mode whose primitives are contract-only is a plan, not a capability.
- **A primitive's contract home is not always its code home.** P4 is the case that matters: the classifier's
  tables are code by decision (ADR-011 item 11), so the primitive's *substitutable* part is the routing decision
  and not the taxonomy. A seam placed at the classifier would contradict an accepted ADR; the seam belongs above
  it (item 4).
- **P2 is a boundary of the same rank as P1.** It is listed here because item 5 of the Background is exactly the
  failure a register prevents: a capability that arrived without a name gets designed around twice.

### 3. The mapping and the leaks live in DESIGN §13

DESIGN is the single source of truth for implementation structure, so the **module → primitive map** and the
**leak register** (every leak with both `file:line` sites, its primitive id, why it matters, and its status) are
written there as the new **§13**, not duplicated here. This ADR owns the naming and the rules; §13 owns the
enumeration, and every change that adds or removes a re-derivation updates §13 **in the same round** — a register
that is allowed to drift is the leak it claims to name.

The leak *classes*, named here because they are the reason this layer exists (the full table, with sites, is
§13.3): **L1** the probe predicate and its inputs re-derived in the reporting surface (four sites; R5-G5
generalised); **L2** route resolution written twice (the buffered and the streaming path); **L3** the two
reporting consumers reading state through two different mechanisms (`/health` on the writer's own connection,
`router stats` on a separate read-only open, `crates/router-cli/src/stats.rs:221`); **L4** the reserved
`auto`/`Selector` slot described as structural but absent from the type system; **L5** the two contract-only
primitives whose absence is load-bearing for two accepted modes; **L6** the guard's vocabulary (`GuardOutcome`)
named in DESIGN §12.3 and absent from code, so a second rule would invent its own move type.

### 4. The decision-provider seam (DP-1)

The register implies one seam that does not exist: a way to supply **the policy decision** — which candidate route
this request takes — from something other than the compiled-in rules, without touching any primitive's invariant.
The seam is specified here as a contract. **No code lands with this ADR** (it is a docs-only change), and no
conformance case is allocated (DESIGN §12.8's rule: IDs are allocated by a human decision, the ADR-010/011/014
precedent).

**Where to start if you arrived here to evaluate a decision model for this gateway** (a feasibility or
shadow-evaluation question rather than an implementation one): the interface is `DecisionProvider` below, the
admissibility rules are DP-1.1…DP-1.5, and the rule that decides whether *inline* use is possible at all is
DP-1.4 — which also registers the finding that the latency envelope currently has no contract home. Read item 4
before proposing a placement, and item 5's M6 for the mode such a proposal promotes into.

**Placement.** Between P3 and P4: after resolution has produced exactly one route and the eligible candidates,
before the guard chain's rules run. It adds **no pipeline stage** (ADR-014 item 9's precedent: the guard chain
already answers "may this request go on this route?"), no new event kind, and no new `error.type`.
The deterministic rules remain the decider; the provider is consulted for an **advice**, and its absence, failure
or silence restores exactly today's behaviour.

```rust
/// The seam's input: what the deterministic rules already compute, handed over read-only.
/// Every field is a read from the record, the projections or stable config — nothing here is
/// derived from the request's message content (see DP-1.2).
pub struct DecisionRequest<'a> {
    pub request_id: &'a str,             // the reproducibility anchor (DP-1.5)
    pub session: Option<&'a str>,        // `prompt_cache_key` preferred (spec §6)
    pub turn_index: u32,
    pub resolved: &'a RouteSpec,         // P3's answer for this request
    pub candidates: &'a [RouteSpec],     // the routes the deterministic rules would consider
    pub plan: Option<PlanView>,          // P4's plan state: account, since, probe deadline
    pub demoted: &'a [String],           // providers ADR-011 refuses right now
    pub features: &'a DecisionFeatures,  // structure only: protocol, byte lengths, tier names
    pub budget: Duration,                // the wait the caller permits (DP-1.4)
}

/// What a provider may answer with — and nothing more.
pub enum DecisionAdvice {
    /// No opinion: the deterministic rules decide. The answer on every failure (DP-1.4).
    Abstain,
    /// Prefer this route. It must be one of `candidates`; anything else is `Invalid`.
    Prefer { route: RouteSpec, reason: &'static str },
    /// Spend nothing on this request: the rules must then find a no-cost route or refuse it.
    Refuse { code: ErrorCode, message: String },
}

pub trait DecisionProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn version(&self) -> &'static str;                       // recorded; a decision is only reproducible with it
    fn advise(&self, req: &DecisionRequest<'_>) -> Result<DecisionAdvice, ProviderError>;
}

pub enum ProviderError { Timeout, Unavailable, Invalid(&'static str) }
```

**The invariants.**

- **DP-1.1 — bounded advice.** The provider may choose among `candidates` and nowhere else: it cannot invent a
  route, cannot reach a **demoted** route (ADR-011's route availability is a correctness constraint, not a
  preference), cannot redefine an `ErrorCode`'s meaning, and cannot express anything about the response. An
  answer outside `candidates` is `Invalid` and is treated as `Abstain`.
- **DP-1.2 — a provider never touches bytes.** It has no access to the request body, no access to headers, and no
  access to the outbound `model` rewrite: **P1 remains the only writer of the outbound bytes**, and a provider
  cannot rewrite prompt content, cannot reorder messages, cannot touch tool schemas, and cannot influence the
  outbound byte stream *without that influence being recorded* (DP-1.5). `DecisionFeatures` carries **structure
  only** — protocol, byte lengths, session/turn, the candidate routes, the projections — never message text. A
  provider that needs prompt content is a different change with its own ADR, because handing content to a model
  creates a rewrite path, which is precisely what constraint 1 and constraint 2 exist to prevent.
- **DP-1.3 — no mid-session flips.** The provider is consulted **only where a policy change is free**: the session
  is absent, or `turn_index == 1`. At any other turn the sticky binding decides and the provider is not asked.
  This is **the same admission rule ADR-014 item 3 imposes on the plan's probe**, deliberately reused rather than
  reinvented: a mid-session policy change re-prefills the whole conversation at the miss price (the first-order
  lever, AGENTS gotchas) and confounds the very comparison the provider was added to serve. Content determinism
  (constraint 2) is untouched by this rule — a provider may route two sessions differently; it may not make the
  payload of one session depend on anything but (content, stable config).
- **DP-1.4 — the failure mode is "today's behaviour".** The call is bounded by `budget`; on
  `Timeout`/`Unavailable`/`Invalid` the advice is `Abstain`, the deterministic rules answer, and the request
  succeeds. A provider can never turn a routing opinion into an outage, a retry, or a second paid attempt. The
  budget is a **required config key with no default** (`decision_provider.timeout_ms`), and the ADR requires its
  value to be justified against the *measured* overhead of the path it sits on — `result.overhead_ms` and its p99
  (spec §9.2) — rather than copied from a round file. **Finding, registered rather than fixed here:** the
  operative latency envelope (`p50 < 15 ms / p99 < 50 ms`) is cited as ADR-009's in four round records and in
  the loop state record, but ADR-009 states no such numbers — it says the budget must name *measured* numbers
  (ADR-009 "Re-measurement is owed"), so the envelope has no contract home. Giving it one is a human decision
  (AGENTS 9 / ADR-012); until then an advisor's timeout is a declared, per-config value the operator states and
  the trace measures, and this ADR invents no number.
- **DP-1.5 — the decision must be replayable, and its effect on bytes must be recorded.** Provenance rides in
  fields that already exist: the advisor's identity goes into the existing free-form
  `decision.plugin_chain[]` with the variant suffix convention ADR-013 item 1 established for the shadow fiber
  (`decision-provider@<id>@<version>`), the wait goes into `decision.decision_ms`, the chosen route into
  `decision.provider` / `decision.model`, an account move into `result.plan_switch`, a refusal into the ordinary
  error record, and an advisor **failure** into the existing `errors[]` as
  `{kind: "internal", plugin: Some("decision-provider@…"), message: "<fallback taken>"}`. No new trace field is
  taken: an optional field is an additive spec §6 change, and this ADR changes no contract (the same stance
  ADR-013 item 1 took when it refused a dedicated shadow field).
  - **The reproducibility rule, stated as a test:** the provider must be a pure function of `DecisionRequest`.
    A provider whose answer depends on state the trace does not carry is **inadmissible**, because
    `router replay` (DESIGN §9) could not reproduce the decision and the report would be a re-roll rather than a
    measurement. A provider that is *stochastic* is admissible only when its randomness is derived from a
    recorded input — the reproducible form is a seed computed from `request_id` and the advisor's identity, so a
    replay recomputes the same draw without a new field. "No unrecorded influence on outbound bytes" is therefore
    a mechanical rule, not a promise: **any effect on outbound bytes is either the recorded route decision or it
    does not exist.**

**Alignment with the hard constraints, clause by clause (the checklist this seam was written against):**

| Constraint | Verdict | Why |
|---|---|---|
| AGENTS 1 — byte boundary | **holds by construction** | the seam sits above P1 and has no handle on the body (DP-1.2); the only mutations remain ADR-015's two, applied by `RawBody` |
| AGENTS 2 — content determinism | **holds, with DP-1.3 as the load-bearing rule** | the advisor is admitted only at a session boundary, so it cannot rewrite a conversation's payload mid-session; a routing difference between sessions is routing, not payload rewriting (ADR-013 item 2's own distinction) |
| AGENTS 3 — observation boundary | **holds** | the provider is product-side code or config; it reads nothing from the loop's tree, and what it did is visible only through the trace (DP-1.5). The loop influences it exactly as it influences any other policy: a config artifact (or an L2 process, if a later ADR takes that form) |
| AGENTS 4 — no unverified savings | **holds** | the provider may not produce a money figure at all: it returns a route, not a price; P8's ledger remains the only source of cost, and a `Refuse` is a refusal, never an estimate |
| AGENTS 5 — no fabricated prices | **holds** | the seam touches no price and no quota; DP-1.4's required, sourced budget is the one number it introduces, and it is a latency cap, not a price |
| latency envelope | **holds only as far as it is stated** | DP-1.4 bounds the call and defaults to "off"; the envelope itself has no contract home (the registered finding above) |

**What the seam refuses, stated so a later round cannot read this ADR as authorizing it:** it does not authorize
per-request advice; it does not authorize an advisor that sees message content; it does not authorize a
provider-supplied error taxonomy (ADR-011 item 11 stands — a provider *consumes* classes); it does not authorize
a new trace field, a new event kind, a new `error.type`, a new pipeline stage or a new config section beyond the
one block below; it does not authorize automatic model selection (spec §1's non-goal and spec §3's `auto`
refusal stand until a separate decision changes them — the seam is how such a decision would *land*, not the
decision); and it allocates no conformance case.

**Config shape (additive, and the only key block this ADR introduces):**

```yaml
decision_provider:           # absent ⇒ no provider is installed and the behaviour is bit-for-bit today's
  kind: builtin/table        # the first form is a compiled, table-driven advisor (see Alternatives)
  timeout_ms: 5              # required, no default: an operator states the wait it permits (DP-1.4)
  admit: session_boundary    # the only value in v0.x (DP-1.3); anything else is a load-time error
```

### 5. The mode register

A mode is a composition of primitives with an order, a state and a policy. Every mode below exists today except
where marked, and each row names the primitives it composes — which is what makes "why can this not be built
yet?" answerable without reading a round file.

| id | mode | composes | trigger | state | audit record | failure mode (fail by design) | state |
|---|---|---|---|---|---|---|---|
| **M1** | `failover` | P3 (candidates), P4 (classification + demotion), P5 (`failover_from`, `errors[]`), P7 (the cooldown projection), P8 (the priced switch) | a classified upstream failure that fails over | the demotion projection (ADR-011) | `error.classified`, `failover.triggered`, `result.failover_from` | a chain that re-attempts a known-dead provider — removed by ADR-011 item 4 | wired (ADR-011) |
| **M2** | `plan-first` (spill + session-boundary probe) | P3, P4 (`PlanFirstRule`), P5 (`result.plan_switch`), P7 (`plan.switched` FULL; `plan_state` is DDL v2 and rebuildable), P8 (marginal 0 in-plan vs the real price out) | upstream `403 quota_exhausted` (the only authority), then the probe predicate | `plan_state` + the probe deadline | `plan.switched` + `result.plan_switch` | stale or lost state → one off-preference attempt, never a wrong charge | wired (ADR-014) |
| **M3** | `shadow` | P9 (`ctx.isolate`, `ctx.intercept`), P6 (a candidate transform), P5 (the variant-suffixed fiber id in `plugin_chain[]`) | a candidate exists and must be checked for divergence without spending | none (a shadow writes nothing upstream) | the shadowed request's own record, `verdict: inferred` | none by construction: a shadow cannot enter the cost gate (ADR-013 item 1) | **contract-only** — depends on P9 (and P6 for anything to shadow) |
| **M4** | `canary` | P4 (a parameter inside the L1 envelope), P7 (the artifact write, and its revert), P8 (the verified-only cost trigger), P9 (the keyed config diff), P5 (the trace's cost/error figures) | a pre-registered envelope entry with evidence | the artifact + the loop's L1 envelope | the round file + the loop state record + the trace | a canary that reaches its horizon without the minimum sample → **rollback**, never promotion by neglect | **contract-only** for its P9 half; the L1 machinery is loop-side and does not exist yet (ADR-012) |
| **M5** | `rollback` | P7 (write the previous artifact back), the mode it reverses, P5 (the round-file line) | a declared trigger fires | the artifact | the round file + the loop state record | an experiment whose only exit is a human waking up — refused as an L1 precondition (ADR-013 item 4) | **contract-only** (loop-side) |
| **M6** | `advised-decision` (new, **proposed**) | DP-1 (item 4), P4 (the deterministic rules remain the fallback and the fallback's owner), P5 (the advisor's identity in `plugin_chain[]`), P8 (an advisor has no right to produce a figure) | an advisor is configured, and the request is at a session boundary | the sticky binding (P7) records what the decision was | the record's existing decision fields + the advisor's identity | a provider that times out, fails or answers outside `candidates` → `Abstain` → today's behaviour (DP-1.4) | **not implemented**; contract frozen here, promotion path is ADR-012's ladder (L0 artifact first) |

M6 is named deliberately rather than left implicit: without it the seam has no mode home, and the honest fact —
"the gateway has no advisory decisions today" — would be invisible in exactly the way the `auto` refusal made it
invisible until now.

### 6. The workflow register

A workflow is what someone is trying to get done. Each row names the primitives it depends on and the modes it
uses; a workflow that needs a primitive which is contract-only cannot be completed today, and saying so is the
point of the table.

| id | workflow | what the person is doing | primitives it depends on | modes | what it must not touch |
|---|---|---|---|---|---|
| **W1** | `connect a client` (codex / hermes / claude code) | point a client's base_url at the gateway and have every request behave — with admission on or off | P2 (when `auth_token_env` is set), P1, P3, P5, P7 (the startup prerequisites), P4 (only if a policy is configured) | M1, M2 | the byte path, the pipeline order, the roster's semantics |
| **W2** | `save tokens` | pay less for the same work on a long session | P1 (**the first-order lever**: prefix stability), P8 (the ledger that proves it), P4, P3 | M2 (in-plan vs metered), M1 | a transform is a P6 change (ADR-003/ADR-008), never a client-side or gateway-local hack. **Honest statement at `HEAD`: today's savings come from prefix stability and plan-first; P6 is contract-only, so no transform contributes** |
| **W3** | `read the report` | audit what was spent, what was refused, where the allowance went | P5 (the record), P7 (the log), P8 (the labels), P4 (the state the report explains) | — | the two consumers must not re-derive a capability: their independent reads are leak **L3** and their re-derived predicate is **L1** |
| **W4** | `keep the plan preferred` | configure the subscription as the preferred account and decide the spending policy | P3, P4, P7, P8 + config | M2 | only an upstream `403 quota_exhausted` may move the account; `recover: none` means "no automatic probe", not "no policy" |
| **W5** | `iterate a policy` (the analysis loop's own workflow) | shadow → canary → adopt → roll back, under the gates | P5 (the only channel out), P7, plus the loop-side artifacts that are outside the product | M3, M4, M5 | the measurement (ADR-012's never-mutable paths). **Blocked today on P6 and P9** |

## Alternatives considered

| Alternative | Why rejected |
|---|---|
| no vocabulary — keep describing the system as spec + DESIGN + ADRs (the status quo) | the two failure modes in the Background are already priced: four re-derivations where R5 found one, and a seam wanted at a place where an accepted ADR forbids it. A contributor has no unit smaller than "the product" |
| name the layers in a DESIGN section only, no ADR | DESIGN is the structural truth and will carry the map and the leak register (§13), but the load-bearing statements here — what may never be re-implemented, who owns a primitive, that a decision provider may not touch bytes — are *why* choices. The repository's own split (AGENTS 8: the book is user-facing, spec + DESIGN are engineering contracts, ADRs are WHY) puts them in an ADR |
| register the primitives without naming the leaks | a register that does not name `file:line` decays into an aspiration; the leaks *are* the evidence that the layer was missing, so §13.3 is part of the deliverable rather than a follow-up |
| fix the leaks in this change | this change is docs-only. Extracting the probe predicate is a code change whose contract is asserted today by unit tests per arm (`health.rs:437-466`) and by spec §9.1's frozen `blocked_by` vocabulary — it needs its own card and its own evidence that the surface's words did not move |
| build the seam now (a real `DecisionProvider` plus a table-driven first implementation) | the loop execution model's rule 2: the interface is frozen by a card before the fan-out, and a seam with no admission rule would invite the per-request flip that DP-1.3 exists to forbid. A seam is cheaper to write after its refuse-list is written |
| place the seam **at** the classifier (let a provider contribute patterns) | it contradicts ADR-011 item 11 (the pattern tables are code, not an auto-adoptable artifact) and would give one fact two owners. A provider **consumes** classes; only a human changes the taxonomy |
| implement the advisor as a tier-A plugin (in P9) | P9 has no implementation, and a plugin-shaped advisor would sit *inside* the runtime rather than in front of the rules; a compiled, table-driven seam is smaller, removable, and needs no runtime |
| implement the advisor as a tier-B process immediately (ADR-002/ADR-012 L2) | it adds a process, a protocol and a timeout to a decision with no measured benefit yet. It is the right transport once a model is involved (crash and timeout isolation are exactly what L2 is for), and DP-1.4's failure semantics are written so that changing the transport is not a change of contract |
| give the advisor a new trace field (`decision.advice`) | an optional field is still a spec §6 change, and this ADR changes no contract. ADR-013 item 1 refused a dedicated shadow field for the same reason and the existing fields carry the same facts (DP-1.5) |
| let the advisor see prompt content | it creates a rewrite path — the one thing constraints 1 and 2 exist to prevent — and it would make the advisor's cost a function of the payload it is allowed to change. `DecisionFeatures` is structure only |
| allow advice on every request | a mid-session policy change re-prefills the conversation at the miss price and confounds the measurement (ADR-013 item 2's rationale, one level up). Reuse ADR-014 item 3's boundary rule instead of inventing a second one |
| make the advisor's timeout default to a number | the envelope it would be derived from has no contract home (DP-1.4's registered finding), and a default would hide that from an operator. Required, no default |

## Rationale

- **The vocabulary is defined by what fails silently.** Every primitive in the register is one whose violation
  does not throw: a third mutation costs cache, a re-derived predicate costs an operator's trust in the report, a
  projection believed costs a charge. A layer whose contents are chosen by "what breaks loudly" would have
  registered nothing here.
- **The register is deliberately smaller than the repository.** Nine primitives cover eight crates: the SSE relay
  is P1 + P5 applied to a streaming shape, the two forwarding paths are two media carrying the same primitives,
  and config parsing is P3/P4's input rather than a capability. A register that mirrors the module tree would
  document the code instead of the system.
- **DP-1.3 is the whole seam.** Everything else in item 4 is bookkeeping; the admission rule is what makes an
  advisory decision affordable, and it is borrowed from ADR-014 rather than invented, so the two policies cannot
  drift on "when may the route change".
- **A seam above the classifier is not a compromise, it is the correct altitude.** ADR-011 keeps the taxonomy at
  code rank for a good reason (a taxonomy an experiment can move is a taxonomy that measures nothing). What a
  decision provider can honestly own is *which of the legitimate routes this request takes* — a question with a
  small answer space, a recorded effect, and a bounded cost when answered wrongly.
- **Naming the leaks is what makes the register testable.** Each entry is a claim someone can falsify with one
  `grep`; the alternative (prose about "the guard's order lives in one place") is unfalsifiable and was already
  written once, in R5.

## Consequences

- `design/DESIGN.md` gains **§13** ("the primitive register, the module map and the leak register"), additive:
  the register (13.1), the module → primitive map (13.2), the leak register with both sites per entry (13.3), the
  seam's placement (13.4), and the mode/workflow → primitive map (13.5). No existing clause's semantics change.
- **No code lands, no conformance case is allocated** (DESIGN §12.8's rule). The leaks become card-able work, and
  this ADR recommends the split rather than doing it: (a) extract the probe predicate and its three inputs to one
  owner and let both the guard and `/health` call it, preserving spec §9.1's exact `blocked_by` vocabulary; (b) one
  resolution implementation, shared by both forwarding paths; (c) one read seam for the two report consumers (the
  store's read-only open fails while `serve` holds `PRAGMA locking_mode = EXCLUSIVE`, `router-store/src/lib.rs:218`
  — R6-G3 — so the seam must state which facts each consumer can honestly obtain); (d) decide the fate of the
  reserved `Plugin` selection value (keep the word and define it, or remove it) — a human decision, because
  spec §3's list does not contain it; (e) the `GuardOutcome` vocabulary, when a second rule arrives, becomes the
  shared type `PlanMove` is today's local stand-in for.
- The register is **dated**: it describes `76afd81`. A change that adds a second implementation of a primitive
  must update §13.3 in the same round, and a change that adds a primitive is a human decision with an ADR. That
  obligation is the difference between a register and another document that drifts.
- ADR-012's ladder gains its vocabulary from item 5: L0/L1/L2/L3 are promotion levels applied to **modes**, and
  the admissibility rules of item 4 are what an L1 adoption of an advisor would have to satisfy before any of it
  is worth measuring.
- ADR-013's rails are recorded as depending on P9 (contract-only), and ADR-003's whole subject is P6
  (contract-only): both are now dependencies in the open rather than assumptions in a plan.
- The book gains nothing: this is architecture, and the book is user-facing (AGENTS 8). The one user-visible
  consequence — an advisor is off unless configured, and its timeout is the operator's own number — belongs to
  the round that implements the seam.
- Honest boundaries: this ADR does not make any leak harmless (it names them); it does not claim a vocabulary
  prevents drift (it makes drift nameable and reviewable); it does not claim the latency envelope exists (the
  finding is registered, and giving it a contract home is a human decision); and it does not claim the seam is
  sufficient for automatic selection — it is the place such a decision would land, with the content and latency
  guarantees around it written first.

## Publication note (2026-10-03, R62-2)

The `R<n>` labels and finding ids in this document name iterations of the project's own
private analysis loop — a loop that is not part of this repository, so no label here is
resolvable by a reader of it; they are kept as the provenance of the decision. This
publication pass removed only the dead-pointer class: every reference into that loop's
working tree (its file paths and round-record names, its state record, charter, execution
model and replay contract, its scripts and module names, and the kanban card ids), each
replaced by the neutral phrase the sentence needs. Nothing else moved — no figure,
threshold, `§`/`ADR`/`CONF` id, code sample or contract sentence.
