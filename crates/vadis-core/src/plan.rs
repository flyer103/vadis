//! Plan-first routing's pure decision core (spec §4.6, ADR-014, DESIGN
//! §12.10.8; ADR-049 §5.1/§5.2 generalize it to the plan tier). One rule
//! of the Guard stage, evaluated **before** the allowance rule, answering
//! with the vocabulary `GuardOutcome` already has: `Pass` (this route) /
//! `Downgrade` (the overflow route) / `Reject`.
//!
//! The rule order is normative (spec §4.6 / ADR-014 item 9) and is the
//! whole decision:
//!
//! ```text
//! for a request inside a plan family:
//!   1. probe admission (the full predicate)     -> Pass on the tier's head  (free)
//!   2. overflow cap reached                      -> Reject{cost_cap_exceeded}
//!   3. active route is in the plan tier          -> Pass on the ACTIVE route (§5.2)
//!   4. active route is in the metered tier       -> Downgrade(`overflow`)  [spill]
//!                                                -> Reject{quota_exceeded} [block]
//! ```
//!
//! Rule 1 before rule 2 is deliberate: a probe is an attempt on the
//! *free* account, so a cap on the metered one must not refuse it.
//!
//! Rule 3 is ADR-049 §5.2's coherence condition, not an optimization:
//! `Pass` returns the family's **active route** whenever that route is in
//! the plan tier — never the policy's `primary` unconditionally. Without
//! it a tier thrashes (the request after a plan moved A→B would be sent
//! back to A, the per-request flip ADR-014 constraint 1 forbids), so a
//! plan that answered `403 quota_exhausted` stays retired for the family
//! until the whole tier is exhausted and a probe re-admits the head. A
//! family with no state row passes `primary` exactly as before (the
//! caller hands the rule `policy.primary` as the active route).
//!
//! Purity (AGENTS constraint 2): every input is a projection read, a
//! config value, or a `now` the caller supplies — the rule compares
//! instants, it never decides content with them. The same state and the
//! same inputs always give the same answer.

use crate::config::{
    AccountKind, OnPrimaryExhausted, OverflowSelection, PlanPolicyCfg, RecoveryMode, RouteSpec,
    VadisConfig,
};
use crate::cost::Nano;
use crate::error::ErrorCode;

/// The family's account state, **derived** from the active route against
/// the revision's plan tier (ADR-049 §5.2): `primary` ⇔ the active route
/// is in the plan tier, `overflow` ⇔ it is in the metered tier. The wire
/// words are unchanged, so every existing reader is untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanAccount {
    Primary,
    Overflow,
}

impl PlanAccount {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Overflow => "overflow",
        }
    }
}

/// What the rule says (the Guard-vocabulary answer plus why, so the trace
/// and the `plan.switched` payload can name the cause without a second
/// evaluation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanMove {
    /// This route is allowed. `probe` is true when the pass was admitted
    /// as a recovery probe on the tier's head (ADR-014 item 3) — the
    /// attempt that, on success, flips the family back and records it.
    Pass {
        route: RouteSpec,
        probe: bool,
    },
    /// A different route must be taken: the family's overflow route
    /// (`on_primary_exhausted: spill`, the active route in the metered
    /// tier).
    Downgrade {
        route: RouteSpec,
    },
    Reject {
        code: ErrorCode,
        message: String,
    },
}

/// Why a probe was **not** admitted although the family is spending
/// metered — the one-condition-per-arm mirror of ADR-014 item 3's
/// predicate, so a test can assert each condition independently (the
/// card's truth-table requirement).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeBlockedBy {
    /// `recover: none` — no automatic probe, ever (spec §4.6).
    RecoveryDisabled,
    /// `session` is null: a sessionless request has no boundary to be
    /// admitted at and never probes (ADR-014 item 3's hard rule).
    NoSession,
    /// `turn_index != 1` — mid-session; a probe happens only at a
    /// session boundary.
    NotSessionBoundary,
    /// The family's active route is still **in the plan tier** (ADR-049
    /// §5.2's generalized clause: a probe is the way back from the
    /// metered tier, and a family draining its plans has nothing to
    /// probe back to), **or** `now < since_us + cooldown`. Both arms
    /// read "not yet back on a plan" from the surface's point of view,
    /// so they share one word.
    Cooldown,
    /// ADR-011's cooldown projection refuses the tier's head (route
    /// availability; deliberately not merged with family intent).
    PrimaryDemoted,
    /// The local counter's only influence (spec §4.6 rule 3): the plan's
    /// own window boundary has not passed, so the attempt is provably
    /// doomed and the probe waits. Deferring an experiment is not
    /// blocking a request — the request itself still goes where the
    /// state says.
    DeferredByWindow,
}

impl ProbeBlockedBy {
    /// The spec §9.1 `probe.blocked_by` word for this arm, or `None` for
    /// the two request-shaped arms (`NoSession`, `NotSessionBoundary`):
    /// those are properties of a request that does not exist when
    /// `/health` renders, and for every such request the claim would be
    /// false, so the surface has no word for them (§9.1's table; DESIGN
    /// §12.10.8). One vocabulary, derived from the guard's own enum, so
    /// the report and the guard cannot disagree about *why* an attempt
    /// is blocked (ADR-016 §13.3 L1a: this method is the single owner).
    pub const fn blocked_by_surface_word(self) -> Option<&'static str> {
        match self {
            Self::RecoveryDisabled => Some("recovery_disabled"),
            Self::NoSession => None,
            Self::NotSessionBoundary => None,
            // The account-state arm and the elapsed-time arm share the
            // surface word: from the surface's point of view (no request
            // in hand) both read "not yet back on the plan tier" — the
            // deadline has not passed, whether because the family never
            // left it or because `since + cooldown` is still in the
            // future.
            Self::Cooldown => Some("cooldown"),
            Self::PrimaryDemoted => Some("primary_cooling_down"),
            Self::DeferredByWindow => Some("window_not_reset"),
        }
    }
}

/// The plan-state row as the rule sees it (read through the store seam).
/// ADR-049 §5.2: the state records **the route the family is currently
/// on** — `account` is derived from it against the revision's tier, never
/// stored as a bare two-valued word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanStateRow {
    /// The route the family is currently on. A family that never switched
    /// has no row; the caller passes `policy.primary` (today's
    /// byte-identical behaviour).
    pub route: RouteSpec,
    /// The last transition's instant (= the `plan.switched` row's
    /// `ts_us`). 0 for a family that never switched.
    pub since_us: i64,
}

/// One member of the family's metered candidate set (ADR-049 §5.4),
/// carrying the exact pair §5.6 ordered on — so `/health`'s
/// `metered_candidates[]` and the trace's `plan_switch.candidates[]` are
/// checkable by hand against the roster (the audit-evidence property
/// §7(b) demands).
#[derive(Debug, Clone, PartialEq)]
pub struct MeteredCandidate {
    pub route: RouteSpec,
    /// The candidate's provider-entry currency (§4.8) — read, never
    /// derived; a mixed-currency set is refused at load (§5.6 rule 3).
    pub currency: crate::cost::Currency,
    /// The §5.6 rank key: the as-written per-1K prices (`price.tiers`
    /// models by their **first band** — the lowest ceiling's prices, the
    /// one band every entry has, and no estimate).
    pub rank_key: RankKey,
}

/// The ranking's key pair (§5.6 rule 1): ascending `input_miss`, then
/// ascending `output`. `input_miss` leads because a route change is paid
/// as a re-prefill — an input event; `output` breaks ties because it is
/// the same request's other real charge. As-written f64 values from the
/// sourced price table (constraint 5: no conversion, no estimate).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RankKey {
    pub input_miss: f64,
    pub output: f64,
}

/// The family's metered candidate set (ADR-049 §5.4): every roster route
/// whose provider is `account: api` and whose model entry carries the
/// family tag (§4.8's tag *is* the set), **in roster declaration order**
/// — the tie-break of §5.6 rule 1 and the order `rank_metered`'s stable
/// sort preserves. Adding a metered provider is a roster edit and
/// nothing else; no config key names the set.
pub fn metered_candidates(config: &VadisConfig, policy: &PlanPolicyCfg) -> Vec<MeteredCandidate> {
    let mut out: Vec<MeteredCandidate> = Vec::new();
    for p in &config.providers {
        if p.account != AccountKind::Api {
            continue;
        }
        for m in &p.models {
            let tag = m.family.clone().unwrap_or_else(|| m.id.clone());
            if tag != policy.family {
                continue;
            }
            // §5.6 rule 4: a `price.tiers` model ranks by its FIRST band
            // (the smallest `up_to` — the loader enforces ascending
            // ceilings with the unceiled band last, so `tiers[0]` is it).
            // Every band states all four prices and the flat shape states
            // all four scalars (`quad_to_table` refuses anything else at
            // load), so the `INFINITY` arms are unreachable through a
            // validated config — answered, never unwrapped.
            let (input_miss, output) = match &m.price.tiers {
                Some(tiers) => match tiers.first() {
                    Some(band) => (
                        band.input_miss.map(|v| v.0).unwrap_or(f64::INFINITY),
                        band.output.map(|v| v.0).unwrap_or(f64::INFINITY),
                    ),
                    None => (f64::INFINITY, f64::INFINITY),
                },
                None => (
                    m.price.input_miss.map(|v| v.0).unwrap_or(f64::INFINITY),
                    m.price.output.map(|v| v.0).unwrap_or(f64::INFINITY),
                ),
            };
            out.push(MeteredCandidate {
                route: RouteSpec {
                    provider: p.name.clone(),
                    model: m.id.clone(),
                },
                currency: p.currency,
                rank_key: RankKey { input_miss, output },
            });
        }
    }
    out
}

/// The §5.6 ranking, a **pure function of (roster, config)** (AGENTS
/// constraint 2): ascending `input_miss`, then ascending `output`, then
/// roster declaration order — the stable sort keeps the set's own
/// declaration order on full ties, and nothing here reads a clock, a
/// turn number or an RNG. Two evaluations on one revision give one
/// order. The ranking orders only; per-wire eligibility (§4.2/ADR-022)
/// is the walk's own rule and resolves after it.
pub fn rank_metered(candidates: &[MeteredCandidate]) -> Vec<MeteredCandidate> {
    let mut ranked = candidates.to_vec();
    ranked.sort_by(|a, b| {
        a.rank_key
            .input_miss
            .total_cmp(&b.rank_key.input_miss)
            .then_with(|| a.rank_key.output.total_cmp(&b.rank_key.output))
    });
    ranked
}

/// The ranked candidate set (§5.4 set + §5.6 order) — the one
/// composition every consumer (the walk, `/health`, the trace) calls,
/// so no second implementation can drift.
pub fn ranked_metered_candidates(
    config: &VadisConfig,
    policy: &PlanPolicyCfg,
) -> Vec<MeteredCandidate> {
    rank_metered(&metered_candidates(config, policy))
}

/// The ranking's head (§5.7): the route a spilled request attempts
/// first under `cheapest`. `None` when the set is empty — the load-time
/// rule (spec §8's last row) refuses that config before it can serve,
/// so a miss here is defensive only and callers keep the policy's
/// `overflow`.
pub fn metered_head(config: &VadisConfig, policy: &PlanPolicyCfg) -> Option<RouteSpec> {
    ranked_metered_candidates(config, policy)
        .first()
        .map(|c| c.route.clone())
}

/// The metered tier's walk order (§5.7): under `cheapest` the ranked
/// candidate routes (§5.6), under `declared` the policy's `overflow`
/// alone — today's behaviour, byte-identical. One owner for the guard's
/// `Downgrade` answer, the walk's chain segment and the 403 handler's
/// move target, so no consumer can order the tier a second way.
pub fn metered_walk(config: &VadisConfig, policy: &PlanPolicyCfg) -> Vec<RouteSpec> {
    match policy.overflow_selection {
        OverflowSelection::Cheapest => ranked_metered_candidates(config, policy)
            .into_iter()
            .map(|c| c.route)
            .collect(),
        OverflowSelection::Declared => vec![policy.overflow.clone()],
    }
}

/// The family's plan candidate set (ADR-049 §5.1): every roster route
/// whose provider is `account: coding_plan` and whose model entry carries
/// the family tag (§4.8's tag *is* the set), in roster declaration order
/// with the policy's `primary` moved to the head — the walk starts there
/// and continues through the remaining members in declaration order. No
/// config key orders the tier (the owner's ruling of 2026-10-04): inside
/// a plan the marginal price is 0 (§4.6 rule 4), so price cannot order it
/// and the operator's own preference — roster order plus the explicit
/// `primary` — is the only information. A provider contributes at most
/// one route by construction (§4.8: at most one model entry per tag per
/// provider).
pub fn plan_tier(config: &VadisConfig, policy: &PlanPolicyCfg) -> Vec<RouteSpec> {
    let mut tier: Vec<RouteSpec> = Vec::new();
    for p in &config.providers {
        if p.account != AccountKind::CodingPlan {
            continue;
        }
        for m in &p.models {
            let tag = m.family.clone().unwrap_or_else(|| m.id.clone());
            if tag == policy.family {
                tier.push(RouteSpec {
                    provider: p.name.clone(),
                    model: m.id.clone(),
                });
            }
        }
    }
    // `primary` names the tier's head (§5.1); the remaining members keep
    // the roster's declaration order. Config validation guarantees the
    // primary route satisfies the same two membership conditions, so the
    // miss arm is defensive only — it answers the single-member tier
    // today's code walked.
    match tier.iter().position(|r| r == &policy.primary) {
        Some(0) => {}
        Some(pos) => {
            let head = tier.remove(pos);
            tier.insert(0, head);
        }
        None => tier.insert(0, policy.primary.clone()),
    }
    tier
}

// -- ADR-063: the failover chain's one order owner -----------------------
//
// The ruling (ADR-063 §2): the failover candidate order is a rule, not a
// knob — healthy before cooling, plan before metered, cheap before dear;
// where the configuration cannot be compared (two currencies, or a price
// the config does not declare — a defensive arm, §2.1) the operator's
// declared order decides. Everything below is the **pure** half of that
// rule: a total six-term key and a stable sort over positions `1..`.
// Position 0 is never moved by the sort (it is the resolved route;
// re-ordering the head is selection, not failover — §2.2). The proxy's
// two walks build the declared list, snapshot each candidate's facts
// (config + one cooldown projection read at the instant the caller
// already takes for eligibility), call `order_chain` once, and walk the
// result — the record then explains the same value (§6), never a
// re-derivation.

/// The ruling's own name, the frozen `rule` token of the record (ADR-063
/// §6.2).
pub const ORDER_RULE: &str = "cheapest_healthy_first";

/// ADR-063 §2.1 term 2: which side of the plan boundary a candidate is
/// on. Plan routes rank ahead of metered ones because their marginal
/// price is 0 (spec §4.6 rule 4) — the derivation of ADR-049 §5.1's
/// "plans drain first", not a second statement of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainTier {
    Plan,
    Metered,
}

impl ChainTier {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Metered => "metered",
        }
    }
}

/// How a position in the order was placed (ADR-063 §6.2's `basis`
/// vocabulary, closed): `plan` = term 2 with §5.1's declaration order,
/// `price` = terms 4–5, `declared_currency` = price was not compared
/// across a currency boundary (the candidate sits behind one: term 3,
/// not terms 4–5, decided its position relative to every candidate
/// outside its own currency group), `declared_no_price` = the entry
/// declares no `input_miss` — the defensive arm of §2.1, unreachable
/// through a validated config and present so the key is total and the
/// vocabulary closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderBasis {
    Plan,
    Price,
    DeclaredCurrency,
    DeclaredNoPrice,
}

impl OrderBasis {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Price => "price",
            Self::DeclaredCurrency => "declared_currency",
            Self::DeclaredNoPrice => "declared_no_price",
        }
    }
}

/// One candidate's order facts (ADR-063 §2.1's inputs, one row per key
/// term): everything here is a config read or a cooldown-projection read
/// the caller took at one instant — never a computation, estimate or
/// conversion. Built by [`order_facts_for_route`] from the roster; a
/// caller that cannot reach a roster entry (defensive only: the loader
/// refuses an unpriceable model, and the walk's `unknown_provider` skip
/// owns the off-roster case) gets the all-`None` price arm so the key
/// stays total.
#[derive(Debug, Clone, PartialEq)]
pub struct ChainOrderCandidate {
    pub route: RouteSpec,
    pub tier: ChainTier,
    /// The provider entry's own currency (§4.8) — a group name, never an
    /// amount; never compared with another currency and never converted.
    pub currency: crate::cost::Currency,
    /// The declared per-1K miss price, as written (`price.tiers` models
    /// by their **first band**, §5.6 rule 4). `None` is the defensive
    /// `declared_no_price` arm — the loader refuses that shape
    /// (`config.rs` `quad_to_table` / the empty-band rule), so no
    /// validated roster can produce it.
    pub input_miss: Option<f64>,
    /// The declared per-1K output price, as written. Absent sorts after
    /// every ranked candidate in its currency group (term 5).
    pub output: Option<f64>,
    /// `Some(up_to)` when the entry is a `price.tiers` model — the first
    /// band's own ceiling, so the record's quote cannot be mistaken for
    /// a band-faithful one (ADR-063 §6.2).
    pub band: Option<f64>,
    /// The entry's declared peak multiplier, recorded because a base
    /// price compared in a peak window is not the price the request paid.
    pub peak_multiplier: f64,
    /// ADR-063 §2.1 term 1 / ADR-056 §2.5: a live route-scoped
    /// `stream_silence` row was read for this candidate at the decision
    /// instant. **Advisory**: it moves the candidate to the back of the
    /// order and no further — a sort cannot refuse.
    pub silence: bool,
    /// The advisory row's `until_us`, when `silence` is set — carried so
    /// the record's `advisory` member can name the window it read.
    pub silence_until_us: Option<i64>,
    /// ADR-063 §3.1's binding row as the caller read it at the ONE
    /// decision instant (§7.1's single clock word) — carried on the
    /// snapshot so the `failover.triggered` record's `excluded[]` entry
    /// is the same object the walk's eligibility saw, never a
    /// re-derivation at record time (§12 site 5). Not a sort term: a
    /// bound candidate is excluded by eligibility (§3.2), never
    /// reordered.
    pub binding: Option<crate::store::CooldownRow>,
}

/// One ordered position: the candidate's facts plus the basis that placed
/// it. `declared_index` (the position in the declared list, term 6) is
/// not carried — it is consumed by the sort.
#[derive(Debug, Clone, PartialEq)]
pub struct OrderedCandidate {
    pub facts: ChainOrderCandidate,
    pub basis: OrderBasis,
}

/// Read one route's order facts off the roster (ADR-063 §2.1's "where it
/// comes from" column). Pure in the config; `silence_until_us` and
/// `binding` are the caller's two cooldown-projection reads at one
/// instant (`availability`'s single owner, §7.1's one clock word),
/// `Some`/`Some` meaning a live route-scoped `stream_silence` row / a
/// live binding row. Neither is a sort term: the advisory row is term
/// 1's input, the binding row is carried for the record (§12 site 5).
/// A route with no roster entry gets the USD default and the `None`
/// price arm — the walk's `unknown_provider` skip excludes it before
/// any attempt and the record lists it in `excluded[]`, so the
/// defaults are never serialized as a fact.
pub fn order_facts_for_route(
    config: &VadisConfig,
    route: &RouteSpec,
    silence_until_us: Option<i64>,
    binding: Option<crate::store::CooldownRow>,
) -> ChainOrderCandidate {
    let provider = config.providers.iter().find(|p| p.name == route.provider);
    let Some(provider) = provider else {
        return ChainOrderCandidate {
            route: route.clone(),
            tier: ChainTier::Metered,
            currency: crate::cost::Currency::Usd,
            input_miss: None,
            output: None,
            band: None,
            peak_multiplier: 1.0,
            silence: silence_until_us.is_some(),
            silence_until_us,
            binding,
        };
    };
    let model = provider.models.iter().find(|m| m.id == route.model);
    let (input_miss, output, band, peak_multiplier) = match model {
        // §5.6 rule 4: a `price.tiers` model ranks by its FIRST band
        // (the loader enforces ascending ceilings with the unceiled
        // band last, so `tiers[0]` is it). An empty band list is
        // refused at load, so the `None` arm is the defensive
        // `declared_no_price` one — answered, never hunted in a roster.
        Some(m) => match &m.price.tiers {
            Some(tiers) => match tiers.first() {
                Some(band) => (
                    band.input_miss.map(|v| v.0),
                    band.output.map(|v| v.0),
                    band.up_to.map(|v| v.0),
                    m.price.peak.multiplier.0,
                ),
                None => (None, None, None, m.price.peak.multiplier.0),
            },
            None => (
                m.price.input_miss.map(|v| v.0),
                m.price.output.map(|v| v.0),
                None,
                m.price.peak.multiplier.0,
            ),
        },
        None => (None, None, None, 1.0),
    };
    ChainOrderCandidate {
        route: route.clone(),
        tier: if provider.account == AccountKind::CodingPlan {
            ChainTier::Plan
        } else {
            ChainTier::Metered
        },
        currency: provider.currency,
        input_miss,
        output,
        band,
        peak_multiplier,
        silence: silence_until_us.is_some(),
        silence_until_us,
        binding,
    }
}

/// The pure order owner (ADR-063 §2): a stable sort of the declared list
/// under the six-term key, **position 0 pinned** (the resolved route —
/// §2.2's head boundary). The key is lexicographic and ascending:
///
/// 1. `silence` — 0 not cooling, 1 silence-cooling (ADR-056 §2.5's
///    preference, stated as a rank; a sort cannot refuse);
/// 2. `tier` — plan (marginal 0) before metered (ADR-049 §5.1 derived);
/// 3. `currency` — the index of the first appearance of that currency
///    among the declared metered candidates (metered only; groups are
///    ordered by declaration, never compared, never converted);
/// 4. `input_miss` — the declared per-1K miss price; undeclared sorts
///    after every ranked candidate in its currency group;
/// 5. `output` — the declared per-1K output price; absent = after every
///    ranked one;
/// 6. `declared` — the candidate's index in the declared list (the
///    pre-rule order; §5.1's declaration order inside the plan tier).
///
/// Purity (AGENTS constraint 2 / ADR-063 §7): terms 2–6 read the input
/// facts, term 1 reads a fact the caller already took, and nothing else —
/// two evaluations on one revision with one projection state give one
/// order. Terms 3–5 are skipped for plan candidates (constant in the
/// key), so on a plan tier the key reduces to term 6 — §5.1 exactly —
/// and on a single-currency metered set it reduces to §5.6 rule 1.
pub fn order_chain(candidates: Vec<ChainOrderCandidate>) -> Vec<OrderedCandidate> {
    // Term 3's group numbering: the order in which each currency first
    // appears among the declared METERED candidates. Plan candidates
    // never consult it.
    let mut groups: Vec<crate::cost::Currency> = Vec::new();
    for c in &candidates {
        if c.tier == ChainTier::Metered && !groups.contains(&c.currency) {
            groups.push(c.currency);
        }
    }
    let group_of = |c: &ChainOrderCandidate| -> usize {
        match c.tier {
            ChainTier::Plan => 0,
            ChainTier::Metered => groups.iter().position(|g| *g == c.currency).unwrap_or(0),
        }
    };
    let key_of = |c: &ChainOrderCandidate, declared: usize| -> (u8, u8, usize, f64, f64, usize) {
        let silence = u8::from(c.silence);
        match c.tier {
            // Terms 3–5 are constant inside the plan tier: every plan
            // candidate is price-equal at marginal 0 (§4.6 rule 4), so
            // the only information left is term 6 — §5.1's own order.
            ChainTier::Plan => (silence, 0, 0, 0.0, 0.0, declared),
            ChainTier::Metered => (
                silence,
                1,
                group_of(c),
                c.input_miss.unwrap_or(f64::INFINITY),
                c.output.unwrap_or(f64::INFINITY),
                declared,
            ),
        }
    };
    let basis_of = |c: &ChainOrderCandidate| -> OrderBasis {
        match c.tier {
            ChainTier::Plan => OrderBasis::Plan,
            ChainTier::Metered if c.input_miss.is_none() => OrderBasis::DeclaredNoPrice,
            ChainTier::Metered if groups.first().is_some_and(|g| *g != c.currency) => {
                OrderBasis::DeclaredCurrency
            }
            ChainTier::Metered => OrderBasis::Price,
        }
    };
    let declared: Vec<&ChainOrderCandidate> = candidates.iter().collect();
    let mut tail: Vec<(usize, &ChainOrderCandidate)> = declared
        .iter()
        .enumerate()
        .skip(1)
        .map(|(i, c)| (i, *c))
        .collect();
    tail.sort_by(|a, b| {
        key_of(a.1, a.0)
            .partial_cmp(&key_of(b.1, b.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut ordered: Vec<OrderedCandidate> = Vec::with_capacity(candidates.len());
    if let Some(head) = declared.first() {
        ordered.push(OrderedCandidate {
            facts: (**head).clone(),
            basis: basis_of(head),
        });
    }
    for (_, c) in tail {
        ordered.push(OrderedCandidate {
            facts: c.clone(),
            basis: basis_of(c),
        });
    }
    ordered
}

/// The pure rule (DESIGN §12.10.8's `PlanFirstRule`, generalized by ADR-049
/// §5.2 to the tier). Constructed from the validated config **and the
/// tier derived from it**; `decide` reads projections only.
pub struct PlanFirstRule {
    policy: PlanPolicyCfg,
    /// The family's plan tier (§5.1), head-first. A single-member tier is
    /// exactly the pre-ADR-049 shape.
    tier: Vec<RouteSpec>,
    /// The family's metered tier in walk order (ADR-049 §5.7): the
    /// policy's `overflow` under `declared`, the §5.6 ranking under
    /// `cheapest`. The `Downgrade` answer names its head, so the guard
    /// and the walk cannot disagree about where a spill starts.
    metered: Vec<RouteSpec>,
}

/// The per-request inputs, gathered by the caller from the projections:
/// every field is a read, none is computed here.
pub struct PlanRequest<'a> {
    /// The request's session (`prompt_cache_key` preferred); `None` for
    /// a sessionless request, which never probes (ADR-014 item 3).
    pub session: Option<&'a str>,
    /// `requests_seen + 1` from the sticky projection (spec §6).
    pub turn_index: u32,
    /// The family's current state: the **active route** plus the last
    /// transition's instant.
    pub state: PlanStateRow,
    /// The request's single clock read (AGENTS constraint 2), in unix
    /// microseconds.
    pub now_us: i64,
    /// ADR-011's answer for the tier's head route: false when the head
    /// provider's demotion projection refuses it right now.
    pub primary_allowed: bool,
    /// The local counter's window verdict (spec §4.6 rule 3): true when
    /// the plan's declared window boundary has not passed yet and the
    /// allowance reads exhausted — the probe waits for the boundary.
    pub deferred_by_window: bool,
    /// The family's metered spend in the current UTC month (measured,
    /// priced by the config table); compared against
    /// `overflow_monthly_cap_usd` only on the overflow route.
    pub overflow_spend: Nano,
    /// **The session pin** (ADR-049 §6 rule 1, ADR-014 Background
    /// constraint 1 restated for `cheapest`): the route the request's
    /// session is already bound to, when that binding names one of the
    /// revision's metered routes. A session that has already spilled
    /// stays on the route it spilled to — one re-prefill, not two — so
    /// a later reload that re-ranks the metered tier re-ranks **new
    /// sessions only**. `None` when there is no session, no binding, or
    /// the binding names a route the revision's roster no longer
    /// carries (a removed provider is not a pin; the walk re-ranks).
    pub pinned: Option<RouteSpec>,
}

impl PlanFirstRule {
    /// Built from the validated config **and the two tiers derived from
    /// it** (§5.1 plan tier, §5.7 metered walk); `decide` reads
    /// projections only. Callers that hold only the policy (the unit
    /// fixtures) keep the single-member shapes today's code walked.
    pub fn new(policy: PlanPolicyCfg, tier: Vec<RouteSpec>) -> Self {
        Self {
            metered: vec![policy.overflow.clone()],
            policy,
            tier,
        }
    }

    /// The full-tiers constructor (ADR-049 §5.7): the guard that knows
    /// its revision's config derives both tiers once — the walk, the
    /// `Downgrade` answer and `/health` then consume the same order.
    pub fn with_metered(
        policy: PlanPolicyCfg,
        tier: Vec<RouteSpec>,
        metered: Vec<RouteSpec>,
    ) -> Self {
        Self {
            metered: if metered.is_empty() {
                vec![policy.overflow.clone()]
            } else {
                metered
            },
            policy,
            tier,
        }
    }

    /// The family's plan tier, head-first (§5.1).
    pub fn tier(&self) -> &[RouteSpec] {
        &self.tier
    }

    /// The family's metered tier in walk order (§5.7).
    pub fn metered(&self) -> &[RouteSpec] {
        &self.metered
    }

    /// Is this route one of the tier's plan routes?
    pub fn in_plan_tier(&self, route: &RouteSpec) -> bool {
        self.tier.contains(route)
    }

    /// The route's account, derived against the tier (§5.2): a plan-tier
    /// route is `primary`, everything else (the metered tier) is
    /// `overflow`.
    pub fn account_of(&self, route: &RouteSpec) -> PlanAccount {
        account_of_route(&self.tier, route)
    }

    /// ADR-014 item 3's admission predicate, generalized by exactly one
    /// clause (ADR-049 §5.2): it is admitted when the family's active
    /// route is **not** in the plan tier, and it attempts the **tier's
    /// head** (`primary`). Spelled out so two implementations cannot
    /// differ (spec §4.6 hard rule 2 + DESIGN §12.10.8): session
    /// non-null; `turn_index == 1`; the active route outside the plan
    /// tier; `now >= since_us + cooldown`; ADR-011's projection does not
    /// refuse the head; and the local counter's window rule does not
    /// defer the attempt. `recover: none` disables probing outright.
    /// Still one attempt on a free account, never a per-request retry.
    pub fn probe_admitted(&self, req: &PlanRequest<'_>) -> Result<(), ProbeBlockedBy> {
        if self.policy.recover == RecoveryMode::None {
            return Err(ProbeBlockedBy::RecoveryDisabled);
        }
        if req.session.is_none() {
            return Err(ProbeBlockedBy::NoSession);
        }
        if req.turn_index != 1 {
            return Err(ProbeBlockedBy::NotSessionBoundary);
        }
        // §5.2's generalized clause: a family still draining its plan
        // tier has nothing to probe back to. The arm shares the surface
        // word with the elapsed-time arm below — from the surface's
        // point of view both read "not yet back on a plan".
        if self.in_plan_tier(&req.state.route) {
            return Err(ProbeBlockedBy::Cooldown);
        }
        // The gate is recomputed from the current config against the last
        // transition's instant (ADR-014 item 10's mid-flight clause): a
        // knob change moves a future deadline, never history.
        let until_us = req.state.since_us.saturating_add(self.cooldown_us());
        if req.now_us < until_us {
            return Err(ProbeBlockedBy::Cooldown);
        }
        if !req.primary_allowed {
            return Err(ProbeBlockedBy::PrimaryDemoted);
        }
        if req.deferred_by_window {
            return Err(ProbeBlockedBy::DeferredByWindow);
        }
        Ok(())
    }

    /// The normative rule order (spec §4.6 "probe admission → overflow
    /// cap → account state"). `route` is the request's resolved route;
    /// the caller has already established it is inside the family.
    pub fn decide(&self, req: &PlanRequest<'_>) -> PlanMove {
        // Rule 1 — probe admission: an attempt on the free account, and
        // it goes to the TIER'S HEAD (§5.2's probe clause).
        if self.probe_admitted(req).is_ok() {
            return PlanMove::Pass {
                route: self.policy.primary.clone(),
                probe: true,
            };
        }
        // Rule 2 — the overflow cap may refuse only what would spend.
        if let Some(cap) = self.policy.overflow_monthly_cap_usd {
            // `to_nano` re-runs the load-time conversion's guards; the
            // config was validated, so this is infallible in practice —
            // still answered, never unwrapped.
            if let Ok(cap_nano) = cap.to_nano() {
                if req.overflow_spend.0 >= cap_nano.0 {
                    return PlanMove::Reject {
                        code: ErrorCode::CostCapExceeded,
                        message: format!(
                            "plan family '{}' has reached its overflow monthly cap ({} USD of \
                             metered spend); refusing to spend further (spec 4.6)",
                            self.policy.family, cap.0
                        ),
                    };
                }
            }
        }
        // Rules 3/4 — the active route, against the tier.
        match self.account_of(&req.state.route) {
            PlanAccount::Primary => PlanMove::Pass {
                // §5.2's coherence condition: the ACTIVE route, never
                // the policy's `primary` unconditionally. A family that
                // never switched arrives with `state.route == primary`,
                // so today's behaviour is the special case.
                route: req.state.route.clone(),
                probe: false,
            },
            PlanAccount::Overflow => match self.policy.on_primary_exhausted {
                OnPrimaryExhausted::Spill => {
                    // **The session pin** (§6 rule 1): a session that has
                    // already spilled onto a metered route stays on it.
                    // The pin is consulted before the ranking so a reload
                    // that re-ranks the metered tier re-ranks NEW
                    // sessions only — the spilled session pays one
                    // re-prefill, not two. Two scope limits, both from
                    // the rule's own subject (THE RANKING): under
                    // `declared` there is no ranking to pin and today's
                    // per-request displacement shape stands (CONF-44
                    // freezes it); and only a route the revision's
                    // metered walk still carries is a pin — a plan-tier
                    // route is never one (the 403 walk retires those for
                    // the family) and a removed provider re-ranks like a
                    // new session.
                    let pin_eligible = self.policy.overflow_selection
                        == OverflowSelection::Cheapest
                        && req
                            .pinned
                            .as_ref()
                            .is_some_and(|r| self.metered.contains(r));
                    if let Some(pinned) = req.pinned.as_ref().filter(|_| pin_eligible) {
                        PlanMove::Pass {
                            route: pinned.clone(),
                            probe: false,
                        }
                    } else {
                        PlanMove::Downgrade {
                            // §5.7 under `cheapest`: the metered tier's HEAD —
                            // the ranking's cheapest (the pure §5.6 order, never
                            // re-decided per request); under `declared` the head
                            // IS the policy's `overflow`, today's answer.
                            route: self
                                .metered
                                .first()
                                .cloned()
                                .unwrap_or_else(|| self.policy.overflow.clone()),
                        }
                    }
                }
                // `block` refuses while the whole plan tier is exhausted
                // (§5.2: the operator's "fail rather than spend" over
                // every plan, not only the first) — and reaching this
                // arm already means exactly that: the active route is in
                // the metered tier.
                OnPrimaryExhausted::Block => PlanMove::Reject {
                    code: ErrorCode::QuotaExceeded,
                    message: format!(
                        "plan family '{}' is on its overflow account (plan tier exhausted) and \
                         on_primary_exhausted is 'block': the request is refused rather than \
                         served from the metered account (spec 4.6)",
                        self.policy.family
                    ),
                },
            },
        }
    }

    fn cooldown_us(&self) -> i64 {
        self.policy.cooldown_us()
    }
}

/// The route's account against a tier (§5.2's derivation, free of the
/// rule so the writer side can derive the same two words from the same
/// inputs — one owner, no drift between the guard and the event).
pub fn account_of_route(tier: &[RouteSpec], route: &RouteSpec) -> PlanAccount {
    if tier.contains(route) {
        PlanAccount::Primary
    } else {
        PlanAccount::Overflow
    }
}

/// Is this route one of the family's routes? (ADR-014 item 4: a request
/// whose resolution lands on one of the family's routes is inside the
/// family; everything else is untouched. ADR-049 §5.1: the family now
/// spans the whole plan tier, not only the policy's `primary`; §5.4:
/// `metered` is the family's metered tier in walk order — under
/// `cheapest` the whole candidate set (§4.8's tag), under `declared` the
/// policy's `overflow` alone.)
pub fn route_in_family(tier: &[RouteSpec], metered: &[RouteSpec], route: &RouteSpec) -> bool {
    tier.contains(route) || metered.contains(route)
}

/// The `plan.switched` payload's reason words (ADR-014 item 8; ADR-049
/// §5.2 adds the same-tier move's word — exactly one, decided by
/// direction like the existing two).
pub const REASON_PRIMARY_EXHAUSTED: &str = "primary_exhausted";
pub const REASON_PRIMARY_RECOVERED: &str = "primary_recovered";
/// The family moved from one plan member to the next and **stayed
/// in-plan** (ADR-049 §5.2): the metered tier was not reached, `account`
/// is still `primary`.
pub const REASON_PLAN_EXHAUSTED: &str = "plan_exhausted";

/// A displacement's reason, decided by direction (spec §6's producer
/// table, extended by ADR-049 §5.2): a move to the metered tier is
/// `primary_exhausted`; a plan-to-plan move is `plan_exhausted`; a move
/// back into the tier is `primary_recovered`. One owner for the walk and
/// the writer side, so the event and the trace cannot disagree.
/// `metered` is the family's metered tier in §5.7 walk order — under
/// `cheapest` the whole candidate set, under `declared` the policy's
/// `overflow` alone (so the `declared` arm is today's answer,
/// byte-identical).
pub fn displacement_reason(
    _policy: &PlanPolicyCfg,
    tier: &[RouteSpec],
    metered: &[RouteSpec],
    from: &RouteSpec,
    to: &RouteSpec,
) -> &'static str {
    if metered.contains(to) && !metered.contains(from) {
        REASON_PRIMARY_EXHAUSTED
    } else if tier.contains(to) && tier.contains(from) {
        REASON_PLAN_EXHAUSTED
    } else {
        REASON_PRIMARY_RECOVERED
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        CapUsdVal, DurationVal, OnPrimaryExhausted, OverflowSelection, RecoveryMode,
    };

    fn route(provider: &str, model: &str) -> RouteSpec {
        RouteSpec {
            provider: provider.into(),
            model: model.into(),
        }
    }

    /// The two-plan tier of ADR-049 §5.1: `plan-a`/`plan-b` carry the
    /// family tag, `p-api` is the metered member.
    fn tiered_policy() -> PlanPolicyCfg {
        PlanPolicyCfg {
            family: "glm-5.3".into(),
            primary: route("plan-a", "glm-5.3"),
            overflow: route("p-api", "glm-5.3"),
            on_primary_exhausted: OnPrimaryExhausted::Spill,
            recover: RecoveryMode::Probe,
            cooldown: DurationVal(15 * 60 * 1_000),
            overflow_monthly_cap_usd: None,
            overflow_selection: OverflowSelection::Declared,
        }
    }

    /// A minimal config whose roster declares the tier: `plan-b` BEFORE
    /// `plan-a` in declaration order, so a passing test proves the head
    /// pin (`primary`) overrides the roster order and the tail keeps it.
    fn tiered_config(policy: &PlanPolicyCfg) -> VadisConfig {
        use crate::config::{
            AccountKind, CacheCfg, ModelCfg, PriceCfg, ProviderCfg, ServerCfg, SessionCfg, TraceCfg,
        };
        let plan_provider = |name: &str| ProviderCfg {
            name: name.into(),
            region: crate::config::Region::Intl,
            currency: crate::Currency::Usd,
            urls: [(
                crate::config::WireApi::Chat,
                "http://127.0.0.1:1/chat/completions".to_string(),
            )]
            .into_iter()
            .collect(),
            api_key_env: Some("K".to_string()),
            api_keys: None,
            wire_api: crate::config::WireApi::Chat,
            supports: vec![crate::config::WireApi::Chat],
            account: AccountKind::CodingPlan,
            models: vec![ModelCfg {
                id: policy.family.clone(),
                family: Some(policy.family.clone()),
                context: crate::config::ContextVal(128_000),
                price: PriceCfg {
                    input_miss: Some(crate::config::PriceVal(0.002)),
                    input_hit: Some(crate::config::PriceVal(0.0002)),
                    cache_write: Some(crate::config::PriceVal(0.0)),
                    output: Some(crate::config::PriceVal(0.004)),
                    peak: crate::config::PeakCfg {
                        multiplier: crate::config::MultiplierVal(1.0),
                        windows: Vec::new(),
                    },
                    tiers: None,
                },
                source: "fixture: plan tier unit".into(),
            }],
            quota: None,
        };
        let mut api = plan_provider("p-api");
        api.account = AccountKind::Api;
        let mut other_tag = plan_provider("plan-c");
        other_tag.models[0].family = Some("other-family".into());
        VadisConfig {
            server: ServerCfg {
                addr: "127.0.0.1:0".into(),
                upstream_attempt_timeout: DurationVal(60_000),
                request_timeout: DurationVal(600_000),
                auth_token_env: None,
                max_body_bytes: 2_097_152,
                stream_idle_timeout: DurationVal(30_000),
            },
            session: SessionCfg {
                key_sources: vec!["prompt_cache_key".into()],
                ttl: DurationVal(43_200_000),
            },
            cache: CacheCfg {
                sticky: true,
                breakeven: crate::config::BreakevenCfg {
                    enabled: true,
                    min_remaining_turns: 2,
                    safety_factor: crate::config::MultiplierVal(1.1),
                },
            },
            trace: TraceCfg {
                dir: "./state/traces".into(),
                rollover: crate::config::Rollover::Hourly,
            },
            // plan-b declared first: the roster order is deliberately NOT
            // the intended preference — `primary` pins the head.
            providers: vec![
                plan_provider("plan-b"),
                plan_provider("plan-a"),
                api,
                other_tag,
            ],
            aliases: Default::default(),
            plugins: Vec::new(),
            fallback: Vec::new(),
            plan_policy: None,
            plan_policies: None,
            state: None,
        }
    }

    fn policy() -> PlanPolicyCfg {
        tiered_policy()
    }

    /// The pre-ADR-049 shape: a single-member tier (the policy's primary).
    fn single_tier(p: &PlanPolicyCfg) -> Vec<RouteSpec> {
        vec![p.primary.clone()]
    }

    fn req<'a>(p: &PlanPolicyCfg) -> PlanRequest<'a> {
        PlanRequest {
            session: Some("sess-1"),
            turn_index: 1,
            state: PlanStateRow {
                route: p.overflow.clone(),
                since_us: 1_000_000,
            },
            // Well past since_us + 15m.
            now_us: 1_000_000 + 16 * 60 * 1_000_000,
            primary_allowed: true,
            deferred_by_window: false,
            overflow_spend: Nano(0),
            // §6 rule 1: no prior binding by default — the fixtures that
            // exercise the pin set it explicitly.
            pinned: None,
        }
    }

    // ------------------------------------------------------------------
    // §5.1 — the tier's discovery and its order
    // ------------------------------------------------------------------

    #[test]
    fn tier_is_the_tag_set_with_primary_pinned_as_the_head() {
        let p = tiered_policy();
        let cfg = tiered_config(&p);
        let tier = plan_tier(&cfg, &p);
        assert_eq!(
            tier,
            vec![route("plan-a", "glm-5.3"), route("plan-b", "glm-5.3")],
            "primary is the head; the remaining members keep roster order \
             (plan-b was declared first); other-family tags and api \
             accounts are not in the tier"
        );
    }

    #[test]
    fn single_plan_config_keeps_the_single_member_tier() {
        let p = tiered_policy();
        let mut cfg = tiered_config(&p);
        cfg.providers.retain(|c| c.name != "plan-b");
        assert_eq!(plan_tier(&cfg, &p), vec![p.primary.clone()]);
    }

    // ------------------------------------------------------------------
    // §5.2 — the state machine across the tier
    // ------------------------------------------------------------------

    #[test]
    fn account_is_derived_from_the_active_route() {
        let p = tiered_policy();
        let tier = plan_tier(&tiered_config(&p), &p);
        assert_eq!(
            account_of_route(&tier, &route("plan-a", "glm-5.3")),
            PlanAccount::Primary
        );
        assert_eq!(
            account_of_route(&tier, &route("plan-b", "glm-5.3")),
            PlanAccount::Primary,
            "the tier's second member is still the plan account"
        );
        assert_eq!(account_of_route(&tier, &p.overflow), PlanAccount::Overflow);
    }

    // The coherence condition (§5.2): Pass returns the ACTIVE route
    // whenever it is in the plan tier — the request after a plan moved
    // A→B goes to B, never back to A.
    #[test]
    fn pass_returns_the_active_route_not_the_pinned_head() {
        let p = tiered_policy();
        let tier = plan_tier(&tiered_config(&p), &p);
        let rule = PlanFirstRule::new(p.clone(), tier);
        let mut r = req(&p);
        r.session = None; // no probe to consider
        r.state.route = route("plan-b", "glm-5.3");
        match rule.decide(&r) {
            PlanMove::Pass { route: got, probe } => {
                assert_eq!(got, route("plan-b", "glm-5.3"));
                assert!(!probe);
            }
            other => panic!("expected a pass on the active plan, got {other:?}"),
        }
    }

    // The same-tier move's reason word (§6's reason-by-direction rule).
    #[test]
    fn same_tier_move_is_plan_exhausted_tier_leave_is_primary_exhausted() {
        let p = tiered_policy();
        let cfg = tiered_config(&p);
        let tier = plan_tier(&cfg, &p);
        let metered = metered_walk(&cfg, &p);
        assert_eq!(
            displacement_reason(
                &p,
                &tier,
                &metered,
                &route("plan-a", "glm-5.3"),
                &route("plan-b", "glm-5.3")
            ),
            REASON_PLAN_EXHAUSTED,
            "plan → plan stays in-plan"
        );
        assert_eq!(
            displacement_reason(
                &p,
                &tier,
                &metered,
                &route("plan-b", "glm-5.3"),
                &p.overflow
            ),
            REASON_PRIMARY_EXHAUSTED,
            "plan → metered leaves the tier"
        );
        assert_eq!(
            displacement_reason(
                &p,
                &tier,
                &metered,
                &p.overflow,
                &route("plan-a", "glm-5.3")
            ),
            REASON_PRIMARY_RECOVERED,
            "metered → plan is the way back"
        );
    }

    // The probe, generalized by one clause: admitted only off-tier, and
    // it attempts the tier's HEAD.
    #[test]
    fn probe_attempts_the_tiers_head_and_only_from_the_metered_tier() {
        let p = tiered_policy();
        let tier = plan_tier(&tiered_config(&p), &p);
        let rule = PlanFirstRule::new(p.clone(), tier);
        // On the metered tier, past the cooldown: admitted, and the pass
        // is on the HEAD (the policy's primary), not the second member.
        let mut r = req(&p);
        match rule.decide(&r) {
            PlanMove::Pass { route: got, probe } => {
                assert!(probe);
                assert_eq!(got, route("plan-a", "glm-5.3"), "the tier's head");
            }
            other => panic!("expected an admitted probe, got {other:?}"),
        }
        // Still on a plan (the second member): nothing to probe back to.
        let mut r = req(&p);
        r.state.route = route("plan-b", "glm-5.3");
        assert_eq!(rule.probe_admitted(&r), Err(ProbeBlockedBy::Cooldown));
    }

    #[test]
    fn probe_truth_table_each_reason_named() {
        let p = policy();
        let rule = PlanFirstRule::new(p.clone(), single_tier(&p));
        // No session.
        let mut r = req(&p);
        r.session = None;
        assert_eq!(rule.probe_admitted(&r), Err(ProbeBlockedBy::NoSession));
        // Mid-session.
        let mut r = req(&p);
        r.turn_index = 3;
        assert_eq!(
            rule.probe_admitted(&r),
            Err(ProbeBlockedBy::NotSessionBoundary)
        );
        // Cooldown not elapsed.
        let mut r = req(&p);
        r.now_us = 1_000_000 + 14 * 60 * 1_000_000;
        assert_eq!(rule.probe_admitted(&r), Err(ProbeBlockedBy::Cooldown));
        // Exactly at the deadline: admitted (>=).
        let mut r = req(&p);
        r.now_us = 1_000_000 + 15 * 60 * 1_000_000;
        assert_eq!(rule.probe_admitted(&r), Ok(()));
        // Primary demoted by ADR-011's projection.
        let mut r = req(&p);
        r.primary_allowed = false;
        assert_eq!(rule.probe_admitted(&r), Err(ProbeBlockedBy::PrimaryDemoted));
        // Deferred by the plan's own window boundary.
        let mut r = req(&p);
        r.deferred_by_window = true;
        assert_eq!(
            rule.probe_admitted(&r),
            Err(ProbeBlockedBy::DeferredByWindow)
        );
        // Active route still in the plan tier: no probe to admit.
        let mut r = req(&p);
        r.state.route = p.primary.clone();
        assert_eq!(rule.probe_admitted(&r), Err(ProbeBlockedBy::Cooldown));
        // recover: none.
        let mut p2 = policy();
        p2.recover = RecoveryMode::None;
        let rule = PlanFirstRule::new(p2, single_tier(&p));
        assert_eq!(
            rule.probe_admitted(&req(&p)),
            Err(ProbeBlockedBy::RecoveryDisabled)
        );
    }

    #[test]
    fn rule_order_probe_before_cap() {
        // Both the probe predicate and the cap fire: the probe wins — a
        // probe is an attempt on the free account and must not be refused
        // by a cap on the metered one (ADR-014 item 9).
        let mut p = policy();
        p.overflow_monthly_cap_usd = Some(CapUsdVal(0.0));
        let rule = PlanFirstRule::new(p.clone(), single_tier(&p));
        let mut r = req(&p);
        r.overflow_spend = Nano(1);
        match rule.decide(&r) {
            PlanMove::Pass { probe, .. } => assert!(probe),
            other => panic!("expected a probe pass, got {other:?}"),
        }
    }

    #[test]
    fn rule_order_cap_before_state() {
        // Active route metered + spill, but the cap is reached: Reject.
        let mut p = policy();
        p.overflow_monthly_cap_usd = Some(CapUsdVal(20.0));
        let rule = PlanFirstRule::new(p.clone(), single_tier(&p));
        let mut r = req(&p);
        r.session = None; // no probe to consider
        r.overflow_spend = Nano(20_000_000_000); // == 20 USD
        match rule.decide(&r) {
            PlanMove::Reject { code, .. } => assert_eq!(code, ErrorCode::CostCapExceeded),
            other => panic!("expected a cap reject, got {other:?}"),
        }
        // Below the cap: Downgrade to the overflow route.
        r.overflow_spend = Nano(19_999_999_999);
        match rule.decide(&r) {
            PlanMove::Downgrade { route } => assert_eq!(route.provider, "p-api"),
            other => panic!("expected a downgrade, got {other:?}"),
        }
    }

    #[test]
    fn state_in_tier_passes_metered_spills_or_blocks() {
        let p = policy();
        let rule = PlanFirstRule::new(p.clone(), single_tier(&p));
        let mut r = req(&p);
        r.session = None;
        r.state.route = p.primary.clone();
        match rule.decide(&r) {
            PlanMove::Pass { route, probe } => {
                assert_eq!(route.provider, "plan-a");
                assert!(!probe);
            }
            other => panic!("expected a plan pass, got {other:?}"),
        }
        // block mode: the refusal fires while the whole tier is
        // exhausted (the active route being metered IS that state).
        let mut pb = policy();
        pb.on_primary_exhausted = OnPrimaryExhausted::Block;
        let rule = PlanFirstRule::new(pb, single_tier(&p));
        let mut r = req(&p);
        r.session = None;
        match rule.decide(&r) {
            PlanMove::Reject { code, message } => {
                assert_eq!(code, ErrorCode::QuotaExceeded);
                assert!(message.contains("glm-5.3"), "names the family: {message}");
                assert!(message.contains("block"));
                assert!(
                    message.contains("plan tier exhausted"),
                    "block names the whole tier: {message}"
                );
            }
            other => panic!("expected a block reject, got {other:?}"),
        }
    }

    #[test]
    fn mid_session_never_probes_goes_where_state_says() {
        // ADR-014 item 3's hard rule: a session already on the metered
        // tier is never probed mid-session — every condition of the
        // predicate holds except `turn_index == 1`, and the answer is
        // still the state's route, not an experiment.
        let p = policy();
        let rule = PlanFirstRule::new(p.clone(), single_tier(&p));
        let mut r = req(&p);
        r.turn_index = 2;
        assert_eq!(
            rule.probe_admitted(&r),
            Err(ProbeBlockedBy::NotSessionBoundary)
        );
        match rule.decide(&r) {
            PlanMove::Downgrade { route } => {
                assert_eq!(route.provider, "p-api");
                assert_eq!(route.model, "glm-5.3");
            }
            other => panic!("mid-session must downgrade, got {other:?}"),
        }
    }

    #[test]
    fn sessionless_never_probes_follows_current_state() {
        // ADR-014 item 3: a sessionless request has no boundary to be
        // admitted at — it follows the current state and no more.
        let p = policy();
        let rule = PlanFirstRule::new(p.clone(), single_tier(&p));
        let mut r = req(&p);
        r.session = None;
        assert_eq!(rule.probe_admitted(&r), Err(ProbeBlockedBy::NoSession));
        match rule.decide(&r) {
            PlanMove::Downgrade { route } => assert_eq!(route.provider, "p-api"),
            other => panic!("sessionless on the metered tier must downgrade, got {other:?}"),
        }
        // On a plan route it passes on that route — the state, not a bet.
        let mut r = req(&p);
        r.session = None;
        r.state.route = p.primary.clone();
        match rule.decide(&r) {
            PlanMove::Pass { route, probe } => {
                assert_eq!(route.provider, "plan-a");
                assert!(!probe);
            }
            other => panic!("sessionless on a plan must pass, got {other:?}"),
        }
    }

    #[test]
    fn local_counter_is_a_warning_never_a_reject_or_forced_spill() {
        // ADR-014 item 2 / GAP-Q16: the local counter's verdict may not
        // Reject a request and may not force a spill. Its only input to
        // this rule is `deferred_by_window` (a probe deferral), so with
        // the counter reading exhausted (`deferred_by_window: true`):
        let p = policy();
        let rule = PlanFirstRule::new(p.clone(), single_tier(&p));
        // - a family still on a plan keeps serving on that plan (the
        //   placeholder allowance refuses nothing);
        let mut r = req(&p);
        r.session = None; // no probe to defer
        r.state.route = p.primary.clone();
        r.deferred_by_window = true;
        match rule.decide(&r) {
            PlanMove::Pass { route, probe } => {
                assert_eq!(route.provider, "plan-a");
                assert!(!probe);
            }
            other => panic!("counter must not force a spill off the plan, got {other:?}"),
        }
        // - a family on the metered tier keeps going where the state
        //   says — deferring the experiment is not blocking the request.
        let mut r = req(&p);
        r.deferred_by_window = true;
        assert_eq!(
            rule.probe_admitted(&r),
            Err(ProbeBlockedBy::DeferredByWindow)
        );
        match rule.decide(&r) {
            PlanMove::Downgrade { route } => assert_eq!(route.provider, "p-api"),
            other => panic!("counter must not reject a metered request, got {other:?}"),
        }
    }

    #[test]
    fn family_membership_spans_the_tier() {
        let p = tiered_policy();
        let cfg = tiered_config(&p);
        let tier = plan_tier(&cfg, &p);
        let metered = metered_walk(&cfg, &p);
        assert!(route_in_family(
            &tier,
            &metered,
            &route("plan-b", "glm-5.3"),
        ));
        assert!(route_in_family(&tier, &metered, &p.overflow));
        assert!(
            !route_in_family(&tier, &metered, &route("plan-c", "glm-5.3"),),
            "another family's plan route is not this family's"
        );
        assert!(!route_in_family(
            &tier,
            &metered,
            &route("p-api", "other-model")
        ));
    }

    #[test]
    fn account_kind_words() {
        assert_eq!(AccountKind::CodingPlan.as_str(), "coding_plan");
        assert_eq!(AccountKind::Api.as_str(), "api");
    }

    // ------------------------------------------------------------------
    // ADR-049 §5.3–§5.6 — the metered tier's set, rank and walk
    // ------------------------------------------------------------------

    /// A `cheapest` policy over `tiered_config`'s roster (one metered
    /// member, `p-api`) — the base for the ranking fixtures below.
    fn cheapest_policy() -> PlanPolicyCfg {
        let mut p = tiered_policy();
        p.overflow_selection = OverflowSelection::Cheapest;
        p
    }

    /// `tiered_config` plus N metered providers carrying the family tag,
    /// declared in the given order — the §5.4 set builder for the rank
    /// tests. Prices are flat unless `tiers` is given.
    fn metered_config(policy: &PlanPolicyCfg, metered: &[(&str, f64, f64)]) -> VadisConfig {
        let mut cfg = tiered_config(policy);
        for (name, input_miss, output) in metered {
            let mut api = cfg
                .providers
                .iter()
                .find(|p| p.name == "p-api")
                .cloned()
                .expect("the fixture's api entry");
            api.name = name.to_string();
            api.models[0].price.input_miss = Some(crate::config::PriceVal(*input_miss));
            api.models[0].price.output = Some(crate::config::PriceVal(*output));
            cfg.providers.push(api);
        }
        cfg
    }

    #[test]
    fn metered_set_is_the_tag_and_rank_orders_on_input_miss_then_output_then_declaration() {
        let p = cheapest_policy();
        // Declaration order mid, cheap, dear — deliberately NOT the price
        // order (CONF-94's own shape): mid and dear tie on input_miss, so
        // output decides; cheap leads on input_miss alone.
        let cfg = metered_config(
            &p,
            &[
                ("p-mid", 0.003, 0.002),
                ("p-cheap", 0.001, 0.009),
                ("p-dear", 0.003, 0.005),
            ],
        );
        let set = metered_candidates(&cfg, &p);
        // The SET keeps roster declaration order (the tie-break's input).
        let names: Vec<&str> = set.iter().map(|c| c.route.provider.as_str()).collect();
        assert_eq!(names, ["p-api", "p-mid", "p-cheap", "p-dear"]);
        // The RANK is the §5.6 order: cheap's 0.001 leads, the
        // fixture's own p-api (0.002) follows, then the 0.003 tie
        // broken by output (mid 0.002 < dear 0.005).
        let ranked_vec = rank_metered(&set);
        let ranked: Vec<&str> = ranked_vec
            .iter()
            .map(|c| c.route.provider.as_str())
            .collect();
        assert_eq!(
            ranked,
            ["p-cheap", "p-api", "p-mid", "p-dear"],
            "ascending input_miss, then output, then declaration order"
        );
        // The walk under `cheapest` is the rank; under `declared` it is
        // the policy's `overflow` alone (§5.7).
        let walk_vec = metered_walk(&cfg, &p);
        let walk: Vec<&str> = walk_vec.iter().map(|r| r.provider.as_str()).collect();
        assert_eq!(walk, ranked, "the walk IS the ranking under cheapest");
        let mut d = p.clone();
        d.overflow_selection = OverflowSelection::Declared;
        assert_eq!(
            metered_walk(&cfg, &d),
            vec![d.overflow.clone()],
            "declared keeps today's single-route walk"
        );
    }

    #[test]
    fn a_full_tie_is_decided_by_roster_declaration_order() {
        let p = cheapest_policy();
        // p-late ties p-api exactly (the fixture's own prices); being
        // declared later it follows (§5.6 rule 1's final tie-break).
        let cfg = metered_config(&p, &[("p-late", 0.002, 0.004)]);
        let ranked_vec = ranked_metered_candidates(&cfg, &p);
        let ranked: Vec<&str> = ranked_vec
            .iter()
            .map(|c| c.route.provider.as_str())
            .collect();
        let pos_api = ranked.iter().position(|n| *n == "p-api").unwrap();
        let pos_late = ranked.iter().position(|n| *n == "p-late").unwrap();
        assert!(
            pos_api < pos_late,
            "an exact tie keeps roster order: {ranked:?}"
        );
    }

    #[test]
    fn a_banded_model_ranks_on_its_first_band() {
        let p = cheapest_policy();
        let mut cfg = metered_config(&p, &[]);
        // Replace p-api's flat price with a banded block whose FIRST
        // band undercuts p-late's flat price and whose second band
        // exceeds it — §5.6 rule 4: the first band is the rank key, so
        // the banded entry ranks FIRST, not last.
        let api = cfg
            .providers
            .iter_mut()
            .find(|pr| pr.name == "p-api")
            .unwrap();
        api.models[0].price.tiers = Some(vec![
            crate::config::TierCfg {
                up_to: Some(crate::config::CeilingVal(32_000.0)),
                input_miss: Some(crate::config::PriceVal(0.0005)),
                input_hit: Some(crate::config::PriceVal(0.0001)),
                cache_write: Some(crate::config::PriceVal(0.0)),
                output: Some(crate::config::PriceVal(0.006)),
            },
            crate::config::TierCfg {
                up_to: None,
                input_miss: Some(crate::config::PriceVal(0.009)),
                input_hit: Some(crate::config::PriceVal(0.0001)),
                cache_write: Some(crate::config::PriceVal(0.0)),
                output: Some(crate::config::PriceVal(0.012)),
            },
        ]);
        let late = metered_config(&p, &[("p-late", 0.002, 0.004)])
            .providers
            .pop()
            .expect("the late entry");
        // `pop` returns the LAST provider — which is `p-late` itself
        // (the fixture appends); re-attach it to the banded cfg.
        cfg.providers.push(late);
        let ranked_vec = ranked_metered_candidates(&cfg, &p);
        let ranked: Vec<&str> = ranked_vec
            .iter()
            .map(|c| c.route.provider.as_str())
            .collect();
        assert_eq!(
            ranked.first(),
            Some(&"p-api"),
            "the banded entry ranks on its first band (0.0005), not its last (0.009): {ranked:?}"
        );
    }

    #[test]
    fn two_evaluations_on_one_revision_are_identical() {
        // §5.6 rule 5 / AGENTS constraint 2: the ranking is a pure
        // function of (roster, config). Two evaluations of the same
        // revision give one order, element for element — the mechanism:
        // `rank_metered` only copies and stable-sorts the candidate set
        // it was handed; it reads no clock, no counter and no RNG, so
        // there is nothing between two calls that could differ.
        let p = cheapest_policy();
        let cfg = metered_config(
            &p,
            &[
                ("p-mid", 0.003, 0.002),
                ("p-cheap", 0.001, 0.009),
                ("p-dear", 0.003, 0.005),
            ],
        );
        let a = ranked_metered_candidates(&cfg, &p);
        let b = ranked_metered_candidates(&cfg, &p);
        assert_eq!(a, b, "purity: two evaluations, one order");
    }

    // ------------------------------------------------------------------
    // ADR-049 §6 rule 1 — the session pin
    // ------------------------------------------------------------------

    /// A spilled session whose binding names a metered route of this
    /// revision's walk PASSES on that route — even when the ranking's
    /// head is a different candidate. The pin is consulted before the
    /// ranking, so a reload that re-ranks re-ranks new sessions only.
    #[test]
    fn a_pinned_session_passes_on_its_metered_route_not_the_new_head() {
        let p = cheapest_policy();
        // Two metered candidates; the ranking's head is p-cheap (0.002).
        let cfg = metered_config(&p, &[("p-cheap", 0.002, 0.003), ("p-dear", 0.005, 0.006)]);
        let tier = plan_tier(&cfg, &p);
        let metered = metered_walk(&cfg, &p);
        assert_eq!(metered.first().unwrap().provider, "p-cheap");
        let rule = PlanFirstRule::with_metered(p.clone(), tier, metered.clone());
        let mut r = req(&p);
        // Mid-session (turn 2): the probe gate is closed, the pin is
        // the live question — the spilled session's own shape.
        r.turn_index = 2;
        // The session spilled onto p-cheap; the revision now ranks
        // p-cheap first anyway — the pin must return the SAME route the
        // ranking would (not a new displacement).
        r.pinned = Some(route("p-cheap", "glm-5.3"));
        match rule.decide(&r) {
            PlanMove::Pass { route, probe } => {
                assert_eq!(route.provider, "p-cheap");
                assert!(!probe, "a pinned pass is not a probe");
            }
            other => panic!("expected a pinned pass, got {other:?}"),
        }
        // The discriminating half: the pin wins over a DIFFERENT head.
        // Same revision, but the session is bound to p-dear (it spilled
        // there under an earlier ranking) — the pass names p-dear, not
        // the ranking's head p-cheap.
        let mut r2 = req(&p);
        r2.turn_index = 2;
        r2.pinned = Some(route("p-dear", "glm-5.3"));
        match rule.decide(&r2) {
            PlanMove::Pass { route, .. } => {
                assert_eq!(route.provider, "p-dear", "the pin, not the head");
            }
            other => panic!("expected a pinned pass, got {other:?}"),
        }
    }

    /// Only a metered route of THIS revision's walk is a pin: a binding
    /// naming a plan-tier route or an off-roster route (a provider the
    /// reload removed) re-ranks like a new session — the Downgrade names
    /// the ranking's head.
    #[test]
    fn an_off_walk_or_off_roster_binding_is_not_a_pin() {
        let p = cheapest_policy();
        let cfg = metered_config(&p, &[("p-cheap", 0.002, 0.003), ("p-dear", 0.005, 0.006)]);
        let tier = plan_tier(&cfg, &p);
        let metered = metered_walk(&cfg, &p);
        let rule = PlanFirstRule::with_metered(p.clone(), tier, metered);
        // A plan-tier route is never a pin (the 403 walk retires those
        // for the family; re-admission is the probe's job).
        let mut r = req(&p);
        r.turn_index = 2;
        r.pinned = Some(route("plan-a", "glm-5.3"));
        match rule.decide(&r) {
            PlanMove::Downgrade { route } => {
                assert_eq!(route.provider, "p-cheap", "re-ranked: the head");
            }
            other => panic!("expected a downgrade, got {other:?}"),
        }
        // A route off this revision's roster entirely (removed by the
        // reload) re-ranks the same way.
        let mut r2 = req(&p);
        r2.turn_index = 2;
        r2.pinned = Some(route("p-gone", "glm-5.3"));
        match rule.decide(&r2) {
            PlanMove::Downgrade { route } => {
                assert_eq!(route.provider, "p-cheap", "re-ranked: the head");
            }
            other => panic!("expected a downgrade, got {other:?}"),
        }
    }

    /// `recover: probe` re-admission still wins over the pin: the probe
    /// predicate (rule 1) is evaluated before the pin (rules 3/4), so a
    /// session at its boundary whose family is off-plan attempts the
    /// tier's head — the pin does not trap a family on the metered tier
    /// forever.
    #[test]
    fn an_admitted_probe_wins_over_the_pin() {
        let p = cheapest_policy();
        let cfg = metered_config(&p, &[("p-cheap", 0.002, 0.003), ("p-dear", 0.005, 0.006)]);
        let tier = plan_tier(&cfg, &p);
        let metered = metered_walk(&cfg, &p);
        let rule = PlanFirstRule::with_metered(p.clone(), tier, metered);
        // `req` is a session at turn 1, well past cooldown, on the
        // overflow route: the probe gate admits it.
        let mut r = req(&p);
        r.pinned = Some(route("p-cheap", "glm-5.3"));
        match rule.decide(&r) {
            PlanMove::Pass { route, probe } => {
                assert!(probe, "the admitted probe, not the pin");
                assert_eq!(route.provider, "plan-a", "the tier's head");
            }
            other => panic!("expected an admitted probe, got {other:?}"),
        }
    }

    /// Under `declared` there is no ranking to pin: the pin never
    /// fires and the pre-ADR-049 per-request displacement shape stands
    /// (CONF-44's frozen subject). A spilled session's binding names
    /// the only metered route — and the answer is still the
    /// Downgrade, with its record, exactly as before this ADR.
    #[test]
    fn under_declared_there_is_no_pin() {
        let mut p = cheapest_policy();
        p.overflow_selection = OverflowSelection::Declared;
        let cfg = metered_config(&p, &[("p-cheap", 0.002, 0.003)]);
        let tier = plan_tier(&cfg, &p);
        let metered = metered_walk(&cfg, &p);
        let rule = PlanFirstRule::with_metered(p.clone(), tier, metered);
        let mut r = req(&p);
        r.turn_index = 2;
        // The session's binding names the added metered provider —
        // under `cheapest` this would be a pin; under `declared` the
        // answer ignores it and names the policy's `overflow` (p-api,
        // the fixture's base api provider), as it always has.
        r.pinned = Some(route("p-cheap", "glm-5.3"));
        match rule.decide(&r) {
            PlanMove::Downgrade { route } => {
                assert_eq!(
                    route.provider, "p-api",
                    "the declared overflow, not the pin"
                );
            }
            other => panic!("expected the declared downgrade, got {other:?}"),
        }
    }

    #[test]
    fn the_guard_downgrades_to_the_metered_heads_and_403_moves_follow_the_walk() {
        // §5.7: the guard's `Downgrade` and the 403 handler's move both
        // take the metered tier's HEAD — the ranking's cheapest under
        // `cheapest`, the policy's `overflow` under `declared`.
        let p = cheapest_policy();
        let cfg = metered_config(&p, &[("p-dear", 0.005, 0.006)]);
        let tier = plan_tier(&cfg, &p);
        let metered = metered_walk(&cfg, &p);
        let rule = PlanFirstRule::with_metered(p.clone(), tier.clone(), metered.clone());
        let mut r = req(&p);
        r.session = None; // no probe to consider
        match rule.decide(&r) {
            PlanMove::Downgrade { route } => {
                assert_eq!(
                    route.provider, "p-api",
                    "the ranking's head (0.002 input_miss)"
                );
            }
            other => panic!("expected a downgrade to the ranking's head, got {other:?}"),
        }
        // A plan-to-metered move's reason is primary_exhausted to ANY
        // member of the metered walk (the tier, not the one route).
        assert_eq!(
            displacement_reason(
                &p,
                &tier,
                &metered,
                &route("plan-a", "glm-5.3"),
                &route("p-dear", "glm-5.3")
            ),
            REASON_PRIMARY_EXHAUSTED,
            "leaving the plan tier for any metered candidate"
        );
    }

    // ------------------------------------------------------------------
    // The §9.1 `blocked_by` surface, reduced to the guard (ADR-016
    // §13.3 L1a: one owner — the surface word is derived from the
    // guard's own arm, never re-decided at the report).
    // ------------------------------------------------------------------

    /// Build the request that renders the section for a family on the
    /// metered tier: a fresh session (`turn_index == 1`, the surface's
    /// own maximal view of the probe gate — any request it could be
    /// asked about has these or is already blocked by a request-shaped
    /// arm the surface does not name).
    fn surface_req<'a>(p: &PlanPolicyCfg, now_us: i64) -> PlanRequest<'a> {
        PlanRequest {
            session: Some("surface"),
            turn_index: 1,
            state: PlanStateRow {
                route: p.overflow.clone(),
                since_us: 1_000_000,
            },
            now_us,
            primary_allowed: true,
            deferred_by_window: false,
            overflow_spend: Nano(0),
            // §6 rule 1: the surface names no session of record — no
            // pin; it renders the answer a NEW session would take.
            pinned: None,
        }
    }

    /// The word the surface would show, derived the way `/health`
    /// derives it: evaluate the guard on the surface's reduced request
    /// and take the arm's own `blocked_by_surface_word`.
    fn surface_word(rule: &PlanFirstRule, p: &PlanPolicyCfg, now_us: i64) -> Option<&'static str> {
        rule.probe_admitted(&surface_req(p, now_us))
            .err()
            .and_then(|e| e.blocked_by_surface_word())
    }

    #[test]
    fn blocked_by_matrix_follows_the_guard_evaluation_order() {
        let p = policy();
        let rule = PlanFirstRule::new(p.clone(), single_tier(&p));
        let deadline = 1_000_000 + 15 * 60 * 1_000_000;
        // Before the deadline: `cooldown` (the first failing condition).
        assert_eq!(surface_word(&rule, &p, deadline - 1), Some("cooldown"));
        // At/after the deadline, everything else healthy: admitted — the
        // surface's `blocked_by: null` / `admitted: true`.
        assert_eq!(surface_word(&rule, &p, deadline), None);
        assert_eq!(surface_word(&rule, &p, deadline + 1), None);

        // Each later condition alone, evaluated in the guard's order.
        let mut p_none = policy();
        p_none.recover = RecoveryMode::None;
        let rule_none = PlanFirstRule::new(p_none, single_tier(&p));
        assert_eq!(
            surface_word(&rule_none, &p, deadline + 1),
            Some("recovery_disabled"),
            "recover: none wins first, whatever the clock says"
        );
        let rule_demoted = PlanFirstRule::new(p.clone(), single_tier(&p));
        let mut r = surface_req(&p, deadline + 1);
        r.primary_allowed = false;
        assert_eq!(
            rule_demoted
                .probe_admitted(&r)
                .err()
                .and_then(|e| e.blocked_by_surface_word()),
            Some("primary_cooling_down")
        );
        let rule_window = PlanFirstRule::new(p.clone(), single_tier(&p));
        let mut r = surface_req(&p, deadline + 1);
        r.deferred_by_window = true;
        assert_eq!(
            rule_window
                .probe_admitted(&r)
                .err()
                .and_then(|e| e.blocked_by_surface_word()),
            Some("window_not_reset")
        );

        // The state arm and the time arm share the surface word `cooldown`:
        // a family still on its plan tier has nothing to probe back to,
        // and the section renders no `probe` member at all — but the guard
        // arm is still nameable, and its word is `cooldown`, not a new one.
        let mut r = surface_req(&p, deadline + 1);
        r.state.route = p.primary.clone();
        assert_eq!(
            rule.probe_admitted(&r),
            Err(ProbeBlockedBy::Cooldown),
            "the state arm stays the `Cooldown` variant"
        );
        assert_eq!(
            ProbeBlockedBy::Cooldown.blocked_by_surface_word(),
            Some("cooldown")
        );

        // The request-shaped arms have no surface word — they are not
        // columns of the report (§9.1's table), and the compiler enforces
        // the exhaustiveness of that mapping above.
        assert_eq!(ProbeBlockedBy::NoSession.blocked_by_surface_word(), None);
        assert_eq!(
            ProbeBlockedBy::NotSessionBoundary.blocked_by_surface_word(),
            None
        );
    }

    #[test]
    fn cooldown_us_is_the_single_ms_to_us_conversion() {
        // L1b's anchor: 900_000 ms (the store crate's own fixture value)
        // is 900_000_000 µs — ×1_000, never ×1_000_000.
        let mut p = policy();
        p.cooldown = DurationVal(900_000);
        assert_eq!(p.cooldown_us(), 900_000 * 1_000);
        let rule = PlanFirstRule::new(p.clone(), single_tier(&p));
        assert_eq!(rule.cooldown_us(), 900_000 * 1_000);
        // Saturation, not overflow.
        p.cooldown = DurationVal(u64::MAX);
        assert_eq!(p.cooldown_us(), i64::MAX);
        // Zero stays zero (the rig default: deadline == since).
        p.cooldown = DurationVal(0);
        assert_eq!(p.cooldown_us(), 0);
    }

    // ------------------------------------------------------------------
    // order_chain (ADR-063 §2.1's key) — the pure owner's own witnesses.
    // Every candidate here is built DIRECTLY: the facts are the unit of
    // test, and the defensive arms (an unpriceable candidate among them,
    // R75-0-F7) exist so the key stays total — no validated roster can
    // produce them, so no fixture ever hunts for one.
    // ------------------------------------------------------------------

    fn cand(
        provider: &str,
        tier: ChainTier,
        currency: crate::cost::Currency,
        input_miss: Option<f64>,
        output: Option<f64>,
    ) -> ChainOrderCandidate {
        ChainOrderCandidate {
            route: route(provider, "m"),
            tier,
            currency,
            input_miss,
            output,
            band: None,
            peak_multiplier: 1.0,
            silence: false,
            silence_until_us: None,
            binding: None,
        }
    }

    fn routes(ordered: &[OrderedCandidate]) -> Vec<String> {
        ordered
            .iter()
            .map(|o| o.facts.route.provider.clone())
            .collect()
    }

    #[test]
    fn single_candidate_order_is_the_identity() {
        // Term 6 preserves the declared list: one candidate, one
        // position — "identical to before this branch", not "except".
        let ordered = order_chain(vec![cand(
            "only",
            ChainTier::Metered,
            crate::Currency::Usd,
            Some(1.0),
            Some(2.0),
        )]);
        assert_eq!(routes(&ordered), vec!["only"]);
        assert_eq!(ordered[0].basis, OrderBasis::Price);
    }

    #[test]
    fn dearer_declared_first_walks_second_under_price() {
        // The incident's own shape, as a relation: the entry written
        // first declares the dearer input — the rule attempts the
        // cheaper one first, by rule rather than by declaration.
        let ordered = order_chain(vec![
            cand(
                "head",
                ChainTier::Metered,
                crate::Currency::Usd,
                Some(1.0),
                Some(2.0),
            ),
            cand(
                "dear",
                ChainTier::Metered,
                crate::Currency::Usd,
                Some(9.0),
                Some(1.0),
            ),
            cand(
                "cheap",
                ChainTier::Metered,
                crate::Currency::Usd,
                Some(2.0),
                Some(9.0),
            ),
        ]);
        assert_eq!(routes(&ordered), vec!["head", "cheap", "dear"]);
        assert_eq!(ordered[1].basis, OrderBasis::Price);
        assert_eq!(ordered[2].basis, OrderBasis::Price);
        // `output` is the tie-break: same input_miss, cheaper output
        // first (§5.6 rule 1, the metered instance of this key).
        let ordered = order_chain(vec![
            cand(
                "head",
                ChainTier::Metered,
                crate::Currency::Usd,
                Some(1.0),
                Some(1.0),
            ),
            cand(
                "out-dear",
                ChainTier::Metered,
                crate::Currency::Usd,
                Some(1.0),
                Some(5.0),
            ),
            cand(
                "out-cheap",
                ChainTier::Metered,
                crate::Currency::Usd,
                Some(1.0),
                Some(4.0),
            ),
        ]);
        assert_eq!(routes(&ordered), vec!["head", "out-cheap", "out-dear"]);
    }

    #[test]
    fn head_stays_at_zero_even_when_dearest() {
        // §2.2's head boundary: re-ordering position 0 is selection, not
        // failover — the sort runs over positions 1.. only.
        let ordered = order_chain(vec![
            cand(
                "head-dear",
                ChainTier::Metered,
                crate::Currency::Usd,
                Some(99.0),
                Some(99.0),
            ),
            cand(
                "cheap",
                ChainTier::Metered,
                crate::Currency::Usd,
                Some(1.0),
                Some(1.0),
            ),
        ]);
        assert_eq!(routes(&ordered), vec!["head-dear", "cheap"]);
    }

    #[test]
    fn plan_candidates_precede_metered_whatever_the_table_says() {
        // Term 2 derives §5.1: a plan's marginal price is 0, so its
        // declared table never enters the sort — even a plan entry
        // priced like a premium route walks before any metered one.
        let ordered = order_chain(vec![
            cand(
                "head",
                ChainTier::Metered,
                crate::Currency::Usd,
                Some(1.0),
                Some(1.0),
            ),
            cand(
                "metered",
                ChainTier::Metered,
                crate::Currency::Usd,
                Some(0.5),
                Some(0.5),
            ),
            cand(
                "plan-a",
                ChainTier::Plan,
                crate::Currency::Usd,
                Some(50.0),
                Some(50.0),
            ),
            cand(
                "plan-b",
                ChainTier::Plan,
                crate::Currency::Cny,
                Some(99.0),
                Some(99.0),
            ),
        ]);
        assert_eq!(
            routes(&ordered),
            vec!["head", "plan-a", "plan-b", "metered"]
        );
        assert_eq!(ordered[1].basis, OrderBasis::Plan);
        assert_eq!(ordered[2].basis, OrderBasis::Plan);
        // The plan tier keeps its declaration order (term 6 inside the
        // tier — §5.1's own argument: price-equal at marginal 0).
        assert_eq!(ordered[1].facts.route.provider, "plan-a");
        assert_eq!(ordered[2].facts.route.provider, "plan-b");
    }

    #[test]
    fn currency_groups_follow_first_declaration_and_are_never_compared() {
        // Term 3: the first-declared currency's group walks first; a
        // cheap route on the far side of the boundary is NOT preferred
        // over a dear one on this side — no FX exists (§4.8), and the
        // operator's declaration is the only non-invented evidence.
        let ordered = order_chain(vec![
            cand(
                "head",
                ChainTier::Metered,
                crate::Currency::Usd,
                Some(1.0),
                Some(1.0),
            ),
            cand(
                "cny-dear",
                ChainTier::Metered,
                crate::Currency::Cny,
                Some(8.0),
                Some(8.0),
            ),
            cand(
                "cny-cheap",
                ChainTier::Metered,
                crate::Currency::Cny,
                Some(2.0),
                Some(2.0),
            ),
            cand(
                "usd-cheap",
                ChainTier::Metered,
                crate::Currency::Usd,
                Some(0.5),
                Some(0.5),
            ),
        ]);
        assert_eq!(
            routes(&ordered),
            vec!["head", "usd-cheap", "cny-cheap", "cny-dear"]
        );
        // Inside each group the price decided; across the boundary it
        // could not — the basis says so.
        assert_eq!(ordered[1].basis, OrderBasis::Price);
        assert_eq!(ordered[2].basis, OrderBasis::DeclaredCurrency);
        assert_eq!(ordered[3].basis, OrderBasis::DeclaredCurrency);
    }

    #[test]
    fn silence_moves_a_candidate_to_the_back_but_never_removes_it() {
        // Term 1 (ADR-056 §2.5's preference as a rank): a cooling
        // candidate walks behind every healthy one, and a sort cannot
        // refuse — the candidate keeps a position.
        let mut cooling = cand(
            "cooling-cheap",
            ChainTier::Metered,
            crate::Currency::Usd,
            Some(0.1),
            Some(0.1),
        );
        cooling.silence = true;
        cooling.silence_until_us = Some(1_000);
        let ordered = order_chain(vec![
            cand(
                "head",
                ChainTier::Metered,
                crate::Currency::Usd,
                Some(1.0),
                Some(1.0),
            ),
            cooling,
            cand(
                "healthy-dear",
                ChainTier::Metered,
                crate::Currency::Usd,
                Some(5.0),
                Some(5.0),
            ),
        ]);
        assert_eq!(
            routes(&ordered),
            vec!["head", "healthy-dear", "cooling-cheap"]
        );
        // Term 1 outranks term 2: a cooling PLAN candidate walks behind
        // a healthy metered one (the preference is the first term).
        let mut cooling_plan = cand(
            "cooling-plan",
            ChainTier::Plan,
            crate::Currency::Usd,
            None,
            None,
        );
        cooling_plan.silence = true;
        cooling_plan.silence_until_us = Some(1_000);
        let ordered = order_chain(vec![
            cand(
                "head",
                ChainTier::Metered,
                crate::Currency::Usd,
                Some(1.0),
                Some(1.0),
            ),
            cooling_plan,
            cand(
                "healthy-metered",
                ChainTier::Metered,
                crate::Currency::Usd,
                Some(5.0),
                Some(5.0),
            ),
        ]);
        assert_eq!(
            routes(&ordered),
            vec!["head", "healthy-metered", "cooling-plan"]
        );
    }

    #[test]
    fn unpriced_candidate_sorts_last_in_its_group_as_declared_no_price() {
        // R75-0-F7's witness: the defensive arm is a TOTALITY guarantee,
        // unreachable through a validated config — so the candidate is
        // fed to the pure function directly, never hunted in a roster.
        // It sorts after every ranked candidate in its currency group,
        // keeps its declared position against its unranked peers, and
        // its basis names the arm — no stand-in number exists.
        let ordered = order_chain(vec![
            cand(
                "head",
                ChainTier::Metered,
                crate::Currency::Usd,
                Some(1.0),
                Some(1.0),
            ),
            cand(
                "unpriced",
                ChainTier::Metered,
                crate::Currency::Usd,
                None,
                Some(1.0),
            ),
            cand(
                "priced",
                ChainTier::Metered,
                crate::Currency::Usd,
                Some(9.0),
                Some(9.0),
            ),
            cand(
                "unpriced-later",
                ChainTier::Metered,
                crate::Currency::Usd,
                None,
                None,
            ),
        ]);
        assert_eq!(
            routes(&ordered),
            vec!["head", "priced", "unpriced", "unpriced-later"]
        );
        assert_eq!(ordered[2].basis, OrderBasis::DeclaredNoPrice);
        assert_eq!(ordered[3].basis, OrderBasis::DeclaredNoPrice);
        // The undeclared price never became a number the sort compared:
        // `priced` (input 9.0) still walks before `unpriced` (no input
        // at all) — INFINITY-ordering, not a zero substituted in.
        assert!(ordered[2].facts.input_miss.is_none());
    }

    #[test]
    fn order_is_a_pure_function_of_its_facts() {
        // Constraint 2 / §7: two evaluations on one fact set give one
        // order — no clock, no RNG, no turn number inside.
        let facts = vec![
            cand(
                "head",
                ChainTier::Metered,
                crate::Currency::Usd,
                Some(1.0),
                Some(1.0),
            ),
            cand(
                "b",
                ChainTier::Metered,
                crate::Currency::Cny,
                Some(1.0),
                Some(1.0),
            ),
            cand(
                "c",
                ChainTier::Metered,
                crate::Currency::Usd,
                Some(3.0),
                Some(1.0),
            ),
            cand(
                "d",
                ChainTier::Plan,
                crate::Currency::Usd,
                Some(2.0),
                Some(2.0),
            ),
        ];
        assert_eq!(order_chain(facts.clone()), order_chain(facts));
    }

    #[test]
    fn order_facts_read_the_rosters_own_numbers() {
        // The reader half: a banded model reports its FIRST band's
        // prices and ceiling, the flat model its own scalars, and the
        // peak multiplier travels with both — every number the config
        // states, none computed (§6.2).
        use crate::config::{CeilingVal, ModelCfg, PriceCfg, ProviderCfg, TierCfg};
        let banded = ModelCfg {
            id: "banded".into(),
            family: None,
            context: crate::config::ContextVal(128_000),
            price: PriceCfg {
                input_miss: None,
                input_hit: None,
                cache_write: None,
                output: None,
                peak: crate::config::PeakCfg {
                    multiplier: crate::config::MultiplierVal(1.5),
                    windows: Vec::new(),
                },
                tiers: Some(vec![TierCfg {
                    up_to: Some(CeilingVal(128_000.0)),
                    input_miss: Some(crate::config::PriceVal(0.001)),
                    input_hit: Some(crate::config::PriceVal(0.0001)),
                    cache_write: Some(crate::config::PriceVal(0.0)),
                    output: Some(crate::config::PriceVal(0.002)),
                }]),
            },
            source: "fixture: order facts unit".into(),
        };
        let p = tiered_policy();
        let mut cfg = tiered_config(&p);
        let mut provider = ProviderCfg {
            name: "p-band".into(),
            region: crate::config::Region::Intl,
            currency: crate::Currency::Cny,
            urls: [(
                crate::config::WireApi::Chat,
                "http://127.0.0.1:1/chat/completions".to_string(),
            )]
            .into_iter()
            .collect(),
            api_key_env: Some("K".to_string()),
            api_keys: None,
            wire_api: crate::config::WireApi::Chat,
            supports: vec![crate::config::WireApi::Chat],
            account: crate::config::AccountKind::Api,
            models: vec![banded],
            quota: None,
        };
        provider.currency = crate::Currency::Cny;
        cfg.providers.push(provider);
        let facts = order_facts_for_route(&cfg, &route("p-band", "banded"), None, None);
        assert_eq!(facts.tier, ChainTier::Metered);
        assert_eq!(facts.currency, crate::Currency::Cny);
        assert_eq!(facts.input_miss, Some(0.001));
        assert_eq!(facts.output, Some(0.002));
        assert_eq!(facts.band, Some(128_000.0));
        assert_eq!(facts.peak_multiplier, 1.5);
        assert!(!facts.silence);
        // A silence row read by the caller is carried, never re-derived.
        let facts = order_facts_for_route(&cfg, &route("p-band", "banded"), Some(9_999), None);
        assert!(facts.silence);
        assert_eq!(facts.silence_until_us, Some(9_999));
        // A binding row read by the caller travels the same way —
        // carried for the record (§12 site 5), never a sort term.
        let row = crate::store::CooldownRow {
            scope: "provider".into(),
            provider: "p-band".into(),
            model: String::new(),
            until_us: 9_999,
            reason: "quota_exhausted".into(),
        };
        let facts = order_facts_for_route(&cfg, &route("p-band", "banded"), None, Some(row));
        assert_eq!(facts.binding.as_ref().unwrap().reason, "quota_exhausted");
        assert!(!facts.silence);
        // An off-roster route: the all-None defensive arm — the walk's
        // `unknown_provider` skip owns it, so no fact is invented.
        let facts = order_facts_for_route(&cfg, &route("nobody", "m"), None, None);
        assert_eq!(facts.input_miss, None);
        assert_eq!(facts.tier, ChainTier::Metered);
    }
}
