//! Plan-first routing's pure decision core (spec §4.6, ADR-014, DESIGN
//! §12.10.8). One rule of the Guard stage, evaluated **before** the
//! allowance rule, answering with the vocabulary `GuardOutcome` already
//! has: `Pass` (this route) / `Downgrade` (the overflow route) /
//! `Reject`.
//!
//! The rule order is normative (spec §4.6 / ADR-014 item 9) and is the
//! whole decision:
//!
//! ```text
//! for a request inside a plan family:
//!   1. probe admission (the full predicate)     -> Pass on `primary`  (free)
//!   2. overflow cap reached                      -> Reject{cost_cap_exceeded}
//!   3. account state is `primary`                -> Pass on `primary`
//!   4. account state is `overflow`               -> Downgrade(`overflow`)  [spill]
//!                                                -> Reject{quota_exceeded} [block]
//! ```
//!
//! Rule 1 before rule 2 is deliberate: a probe is an attempt on the
//! *free* account, so a cap on the metered one must not refuse it.
//!
//! Purity (AGENTS constraint 2): every input is a projection read, a
//! config value, or a `now` the caller supplies — the rule compares
//! instants, it never decides content with them. The same state and the
//! same inputs always give the same answer.

use crate::config::{OnPrimaryExhausted, PlanPolicyCfg, RecoveryMode, RouteSpec};
use crate::cost::NanoUsd;
use crate::error::ErrorCode;

/// The family's account state (`plan_state`, DESIGN §12.10.8; 'primary'
/// is also the state of a family that has never switched).
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
    /// as a recovery probe on the primary (ADR-014 item 3) — the attempt
    /// that, on success, flips the family back and records it.
    Pass {
        route: RouteSpec,
        probe: bool,
    },
    /// A different route must be taken: the family's overflow route
    /// (`on_primary_exhausted: spill`, state `overflow`).
    Downgrade {
        route: RouteSpec,
    },
    Reject {
        code: ErrorCode,
        message: String,
    },
}

/// Why a probe was **not** admitted although the account state is
/// `overflow` — the one-condition-per-arm mirror of ADR-014 item 3's
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
    /// `now < since_us + cooldown`.
    Cooldown,
    /// ADR-011's cooldown projection refuses the primary route (route
    /// availability; deliberately not merged with family intent).
    PrimaryDemoted,
    /// The local counter's only influence (spec §4.6 rule 3): the plan's
    /// own window boundary has not passed, so the attempt is provably
    /// doomed and the probe waits. Deferring an experiment is not
    /// blocking a request — the request itself still goes where the
    /// state says.
    DeferredByWindow,
}

/// The plan-state row as the rule sees it (read through the store seam).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanStateRow {
    pub account: PlanAccount,
    /// The last transition's instant (= the `plan.switched` row's
    /// `ts_us`). 0 for a family that never switched.
    pub since_us: i64,
}

/// The pure rule (DESIGN §12.10.8's `PlanFirstRule`). Constructed from
/// the validated config; `decide` reads projections only.
pub struct PlanFirstRule {
    policy: PlanPolicyCfg,
}

/// The per-request inputs, gathered by the caller from the projections:
/// every field is a read, none is computed here.
pub struct PlanRequest<'a> {
    /// The request's session (`prompt_cache_key` preferred); `None` for
    /// a sessionless request, which never probes (ADR-014 item 3).
    pub session: Option<&'a str>,
    /// `requests_seen + 1` from the sticky projection (spec §6).
    pub turn_index: u32,
    /// The family's current account state.
    pub state: PlanStateRow,
    /// The request's single clock read (AGENTS constraint 2), in unix
    /// microseconds.
    pub now_us: i64,
    /// ADR-011's answer for the primary route: false when the primary
    /// provider's demotion projection refuses it right now.
    pub primary_allowed: bool,
    /// The local counter's window verdict (spec §4.6 rule 3): true when
    /// the plan's declared window boundary has not passed yet and the
    /// allowance reads exhausted — the probe waits for the boundary.
    pub deferred_by_window: bool,
    /// The family's metered spend in the current UTC month (measured,
    /// priced by the config table); compared against
    /// `overflow_monthly_cap_usd` only on the overflow route.
    pub overflow_spend: NanoUsd,
}

impl PlanFirstRule {
    pub fn new(policy: PlanPolicyCfg) -> Self {
        Self { policy }
    }

    /// ADR-014 item 3's admission predicate, spelled out so two
    /// implementations cannot differ (spec §4.6 hard rule 2 + DESIGN
    /// §12.10.8): session non-null; `turn_index == 1`; the account state
    /// is `overflow`; `now >= since_us + cooldown`; ADR-011's projection
    /// does not refuse the primary; and the local counter's window rule
    /// does not defer the attempt. `recover: none` disables probing
    /// outright.
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
        if req.state.account != PlanAccount::Overflow {
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
        // Rule 1 — probe admission: an attempt on the free account.
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
        // Rules 3/4 — the account state.
        match req.state.account {
            PlanAccount::Primary => PlanMove::Pass {
                route: self.policy.primary.clone(),
                probe: false,
            },
            PlanAccount::Overflow => match self.policy.on_primary_exhausted {
                OnPrimaryExhausted::Spill => PlanMove::Downgrade {
                    route: self.policy.overflow.clone(),
                },
                OnPrimaryExhausted::Block => PlanMove::Reject {
                    code: ErrorCode::QuotaExceeded,
                    message: format!(
                        "plan family '{}' is on its overflow account (primary exhausted) and \
                         on_primary_exhausted is 'block': the request is refused rather than \
                         served from the metered account (spec 4.6)",
                        self.policy.family
                    ),
                },
            },
        }
    }

    fn cooldown_us(&self) -> i64 {
        // DurationVal is milliseconds (§12.5); µs = ms × 1_000.
        (self.policy.cooldown.0 as i64).saturating_mul(1_000)
    }
}

/// Is this route one of the family's two routes? (ADR-014 item 4: a
/// request whose resolution lands on either route is inside the family;
/// everything else is untouched.)
pub fn route_in_family(policy: &PlanPolicyCfg, route: &RouteSpec) -> bool {
    &policy.primary == route || &policy.overflow == route
}

/// The `plan.switched` payload's reason words (ADR-014 item 8).
pub const REASON_PRIMARY_EXHAUSTED: &str = "primary_exhausted";
pub const REASON_PRIMARY_RECOVERED: &str = "primary_recovered";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AccountKind, CapUsdVal, DurationVal, OnPrimaryExhausted, RecoveryMode};

    fn policy() -> PlanPolicyCfg {
        PlanPolicyCfg {
            family: "glm-5.3".into(),
            primary: RouteSpec {
                provider: "plan".into(),
                model: "glm-5.3".into(),
            },
            overflow: RouteSpec {
                provider: "api".into(),
                model: "glm-5.3".into(),
            },
            on_primary_exhausted: OnPrimaryExhausted::Spill,
            recover: RecoveryMode::Probe,
            cooldown: DurationVal(15 * 60 * 1_000),
            overflow_monthly_cap_usd: None,
        }
    }

    fn req<'a>() -> PlanRequest<'a> {
        PlanRequest {
            session: Some("sess-1"),
            turn_index: 1,
            state: PlanStateRow {
                account: PlanAccount::Overflow,
                since_us: 1_000_000,
            },
            // Well past since_us + 15m.
            now_us: 1_000_000 + 16 * 60 * 1_000_000,
            primary_allowed: true,
            deferred_by_window: false,
            overflow_spend: NanoUsd(0),
        }
    }

    #[test]
    fn probe_truth_table_each_reason_named() {
        let rule = PlanFirstRule::new(policy());
        // No session.
        let mut r = req();
        r.session = None;
        assert_eq!(rule.probe_admitted(&r), Err(ProbeBlockedBy::NoSession));
        // Mid-session.
        let mut r = req();
        r.turn_index = 3;
        assert_eq!(
            rule.probe_admitted(&r),
            Err(ProbeBlockedBy::NotSessionBoundary)
        );
        // Cooldown not elapsed.
        let mut r = req();
        r.now_us = 1_000_000 + 14 * 60 * 1_000_000;
        assert_eq!(rule.probe_admitted(&r), Err(ProbeBlockedBy::Cooldown));
        // Exactly at the deadline: admitted (>=).
        let mut r = req();
        r.now_us = 1_000_000 + 15 * 60 * 1_000_000;
        assert_eq!(rule.probe_admitted(&r), Ok(()));
        // Primary demoted by ADR-011's projection.
        let mut r = req();
        r.primary_allowed = false;
        assert_eq!(rule.probe_admitted(&r), Err(ProbeBlockedBy::PrimaryDemoted));
        // Deferred by the plan's own window boundary.
        let mut r = req();
        r.deferred_by_window = true;
        assert_eq!(
            rule.probe_admitted(&r),
            Err(ProbeBlockedBy::DeferredByWindow)
        );
        // State not overflow: no probe to admit.
        let mut r = req();
        r.state.account = PlanAccount::Primary;
        assert_eq!(rule.probe_admitted(&r), Err(ProbeBlockedBy::Cooldown));
        // recover: none.
        let mut p = policy();
        p.recover = RecoveryMode::None;
        let rule = PlanFirstRule::new(p);
        assert_eq!(
            rule.probe_admitted(&req()),
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
        let rule = PlanFirstRule::new(p);
        let mut r = req();
        r.overflow_spend = NanoUsd(1);
        match rule.decide(&r) {
            PlanMove::Pass { probe, .. } => assert!(probe),
            other => panic!("expected a probe pass, got {other:?}"),
        }
    }

    #[test]
    fn rule_order_cap_before_state() {
        // State overflow + spill, but the cap is reached: Reject.
        let mut p = policy();
        p.overflow_monthly_cap_usd = Some(CapUsdVal(20.0));
        let rule = PlanFirstRule::new(p);
        let mut r = req();
        r.session = None; // no probe to consider
        r.overflow_spend = NanoUsd(20_000_000_000); // == 20 USD
        match rule.decide(&r) {
            PlanMove::Reject { code, .. } => assert_eq!(code, ErrorCode::CostCapExceeded),
            other => panic!("expected a cap reject, got {other:?}"),
        }
        // Below the cap: Downgrade to the overflow route.
        r.overflow_spend = NanoUsd(19_999_999_999);
        match rule.decide(&r) {
            PlanMove::Downgrade { route } => assert_eq!(route.provider, "api"),
            other => panic!("expected a downgrade, got {other:?}"),
        }
    }

    #[test]
    fn state_primary_passes_state_overflow_spills_or_blocks() {
        let rule = PlanFirstRule::new(policy());
        let mut r = req();
        r.session = None;
        r.state.account = PlanAccount::Primary;
        match rule.decide(&r) {
            PlanMove::Pass { route, probe } => {
                assert_eq!(route.provider, "plan");
                assert!(!probe);
            }
            other => panic!("expected a primary pass, got {other:?}"),
        }
        // block mode.
        let mut p = policy();
        p.on_primary_exhausted = OnPrimaryExhausted::Block;
        let rule = PlanFirstRule::new(p);
        let mut r = req();
        r.session = None;
        match rule.decide(&r) {
            PlanMove::Reject { code, message } => {
                assert_eq!(code, ErrorCode::QuotaExceeded);
                assert!(message.contains("glm-5.3"), "names the family: {message}");
                assert!(message.contains("block"));
            }
            other => panic!("expected a block reject, got {other:?}"),
        }
    }

    #[test]
    fn mid_session_never_probes_goes_where_state_says() {
        // ADR-014 item 3's hard rule: a session already on overflow is
        // never probed mid-session — every condition of the predicate
        // holds except `turn_index == 1`, and the answer is still the
        // state's route, not an experiment.
        let rule = PlanFirstRule::new(policy());
        let mut r = req();
        r.turn_index = 2;
        assert_eq!(
            rule.probe_admitted(&r),
            Err(ProbeBlockedBy::NotSessionBoundary)
        );
        match rule.decide(&r) {
            PlanMove::Downgrade { route } => {
                assert_eq!(route.provider, "api");
                assert_eq!(route.model, "glm-5.3");
            }
            other => panic!("mid-session must downgrade, got {other:?}"),
        }
    }

    #[test]
    fn sessionless_never_probes_follows_current_state() {
        // ADR-014 item 3: a sessionless request has no boundary to be
        // admitted at — it follows the current account state and no more.
        let rule = PlanFirstRule::new(policy());
        let mut r = req();
        r.session = None;
        assert_eq!(rule.probe_admitted(&r), Err(ProbeBlockedBy::NoSession));
        match rule.decide(&r) {
            PlanMove::Downgrade { route } => assert_eq!(route.provider, "api"),
            other => panic!("sessionless on overflow must downgrade, got {other:?}"),
        }
        // On primary it passes on primary — the state, not a bet.
        let mut r = req();
        r.session = None;
        r.state.account = PlanAccount::Primary;
        match rule.decide(&r) {
            PlanMove::Pass { route, probe } => {
                assert_eq!(route.provider, "plan");
                assert!(!probe);
            }
            other => panic!("sessionless on primary must pass, got {other:?}"),
        }
    }

    #[test]
    fn local_counter_is_a_warning_never_a_reject_or_forced_spill() {
        // ADR-014 item 2 / GAP-Q16: the local counter's verdict may not
        // Reject a request and may not force a spill. Its only input to
        // this rule is `deferred_by_window` (a probe deferral), so with
        // the counter reading exhausted (`deferred_by_window: true`):
        let rule = PlanFirstRule::new(policy());
        // - a family still on its primary keeps serving on the primary
        //   (the placeholder allowance refuses nothing);
        let mut r = req();
        r.session = None; // no probe to defer
        r.state.account = PlanAccount::Primary;
        r.deferred_by_window = true;
        match rule.decide(&r) {
            PlanMove::Pass { route, probe } => {
                assert_eq!(route.provider, "plan");
                assert!(!probe);
            }
            other => panic!("counter must not force a spill off primary, got {other:?}"),
        }
        // - a family on overflow keeps going where the state says —
        //   deferring the experiment is not blocking the request.
        let mut r = req();
        r.deferred_by_window = true;
        assert_eq!(
            rule.probe_admitted(&r),
            Err(ProbeBlockedBy::DeferredByWindow)
        );
        match rule.decide(&r) {
            PlanMove::Downgrade { route } => assert_eq!(route.provider, "api"),
            other => panic!("counter must not reject an overflow request, got {other:?}"),
        }
    }

    #[test]
    fn family_membership() {
        let p = policy();
        assert!(route_in_family(
            &p,
            &RouteSpec {
                provider: "api".into(),
                model: "glm-5.3".into()
            }
        ));
        assert!(!route_in_family(
            &p,
            &RouteSpec {
                provider: "api".into(),
                model: "other-model".into()
            }
        ));
        assert!(!route_in_family(
            &p,
            &RouteSpec {
                provider: "third".into(),
                model: "glm-5.3".into()
            }
        ));
    }

    #[test]
    fn account_kind_words() {
        assert_eq!(AccountKind::CodingPlan.as_str(), "coding_plan");
        assert_eq!(AccountKind::Api.as_str(), "api");
    }
}
