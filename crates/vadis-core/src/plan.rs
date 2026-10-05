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
    AccountKind, OnPrimaryExhausted, PlanPolicyCfg, RecoveryMode, RouteSpec, VadisConfig,
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

/// The pure rule (DESIGN §12.10.8's `PlanFirstRule`, generalized by ADR-049
/// §5.2 to the tier). Constructed from the validated config **and the
/// tier derived from it**; `decide` reads projections only.
pub struct PlanFirstRule {
    policy: PlanPolicyCfg,
    /// The family's plan tier (§5.1), head-first. A single-member tier is
    /// exactly the pre-ADR-049 shape.
    tier: Vec<RouteSpec>,
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
}

impl PlanFirstRule {
    pub fn new(policy: PlanPolicyCfg, tier: Vec<RouteSpec>) -> Self {
        Self { policy, tier }
    }

    /// The family's plan tier, head-first (§5.1).
    pub fn tier(&self) -> &[RouteSpec] {
        &self.tier
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
                OnPrimaryExhausted::Spill => PlanMove::Downgrade {
                    route: self.policy.overflow.clone(),
                },
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
/// spans the whole tier, not only the policy's `primary`.)
pub fn route_in_family(policy: &PlanPolicyCfg, tier: &[RouteSpec], route: &RouteSpec) -> bool {
    route == &policy.overflow || tier.contains(route)
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
pub fn displacement_reason(
    policy: &PlanPolicyCfg,
    tier: &[RouteSpec],
    from: &RouteSpec,
    to: &RouteSpec,
) -> &'static str {
    if to == &policy.overflow {
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
        let tier = plan_tier(&tiered_config(&p), &p);
        assert_eq!(
            displacement_reason(
                &p,
                &tier,
                &route("plan-a", "glm-5.3"),
                &route("plan-b", "glm-5.3")
            ),
            REASON_PLAN_EXHAUSTED,
            "plan → plan stays in-plan"
        );
        assert_eq!(
            displacement_reason(&p, &tier, &route("plan-b", "glm-5.3"), &p.overflow),
            REASON_PRIMARY_EXHAUSTED,
            "plan → metered leaves the tier"
        );
        assert_eq!(
            displacement_reason(&p, &tier, &p.overflow, &route("plan-a", "glm-5.3")),
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
        let tier = plan_tier(&tiered_config(&p), &p);
        assert!(route_in_family(&p, &tier, &route("plan-b", "glm-5.3"),));
        assert!(route_in_family(&p, &tier, &p.overflow));
        assert!(
            !route_in_family(&p, &tier, &route("plan-c", "glm-5.3"),),
            "another family's plan route is not this family's"
        );
        assert!(!route_in_family(&p, &tier, &route("p-api", "other-model")));
    }

    #[test]
    fn account_kind_words() {
        assert_eq!(AccountKind::CodingPlan.as_str(), "coding_plan");
        assert_eq!(AccountKind::Api.as_str(), "api");
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
}
