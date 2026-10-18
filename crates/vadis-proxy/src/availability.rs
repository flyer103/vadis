//! The single owner of the plan policy's two projection reads (ADR-016
//! §13.3 L1c/L1d, fixed R10): ADR-011's route-availability answer
//! (`Query::Cooldown`, L1c) and the local counter's window verdict
//! (spec §4.6 rule 3, L1d). Both were previously written twice — once
//! on the request path (`forward.rs`) and once for the `/health` report
//! (`health.rs`), on different clocks (`now_us()` vs a passed-in `now`)
//! — so the deferral rule that gates the probe was stated twice and a
//! change to it had to be made twice to stay true. Both consumers now
//! call the functions here; the second implementations are deleted, not
//! wrapped.
//!
//! Purity (AGENTS constraint 2, the same stance as `PlanFirstRule`):
//! every input is a projection read, a config value, or a `now` the
//! caller supplies. The caller takes one clock read and derives both
//! time words from it — `now_us` for the cooldown comparison,
//! `now_epoch_s` for the window calendar — so one instant answers both
//! consumers and no reader owns a private clock.
//!
//! The `Arc<dyn Store>` (not `&dyn Store`) is the shape both consumers
//! already hold: the Forwarder's field and `/health`'s `AppState::store`
//! are the same writer connection; the availability question is a
//! property of that store, so it lives on it.

use std::sync::Arc;

use vadis_core::config::{PlanPolicyCfg, ProviderCfg, VadisConfig};
use vadis_core::quota::{next_reset, window_start_for};
use vadis_core::store::{Query, QueryRow, Store};

/// ADR-011's route-availability answer for the provider at `now_us`:
/// a live cooldown row that has not expired refuses it right now
/// (L1c's single owner). The clock is the caller's — `plan_guard`'s
/// own single read on the request path, `/health`'s section-read instant
/// in the report — never a second `now_us()` inside.
pub fn provider_in_cooldown(store: Option<&Arc<dyn Store>>, provider: &str, now_us: i64) -> bool {
    let Some(store) = store else {
        return false;
    };
    matches!(
        store.query(Query::Cooldown {
            provider,
            model: None,
        }),
        Ok(QueryRow::Cooldown(Some(row))) if row.until_us > now_us
    )
}

/// ADR-056 §2.5 (spec §4.20.5): is THIS exact route silence-cooling at
/// `now_us` — a live `provider_cooldown` row with `scope: "route"` and
/// `reason: "stream_silence"`? This is the **advisory** half of the
/// cooldown projection's read side, and deliberately a different
/// question from [`provider_in_cooldown`]: a `stream_silence` row never
/// refuses a candidate (the walk prefers a non-cooling one and leaves
/// the cooling route only when such a candidate exists), never appears
/// in `skipped[]`, and never produces `no_available_route`. The probe
/// gate and `/health` keep asking the provider-wide question and are
/// invisible to this row (ADR-056 §2.5.4).
///
/// The union read (`Query::Cooldown { model: Some(..) }`) answers
/// whichever row blocks the route now, a provider-wide row included;
/// only a row that is itself route-scoped and silence-worded answers
/// `true`, so a billing demotion can never be mistaken for a preference.
pub fn route_silence_cooling(
    store: Option<&Arc<dyn Store>>,
    provider: &str,
    model: &str,
    now_us: i64,
) -> bool {
    let Some(store) = store else {
        return false;
    };
    matches!(
        store.query(Query::Cooldown {
            provider,
            model: Some(model),
        }),
        Ok(QueryRow::Cooldown(Some(row)))
            if row.scope == "route" && row.reason == "stream_silence" && row.until_us > now_us
    )
}

/// The local counter's window verdict (spec §4.6 rule 3, GAP-Q1; L1d's
/// single owner): true when the primary provider declares a quota plan
/// covering the family, that plan's current window has reached its
/// allowance, **and** the plan's own window boundary has not passed yet
/// — the probe waits for the boundary (the reset instant is a config
/// fact with a `source`, unlike the allowance value). A warning that
/// defers an experiment, never a block on a request.
///
/// Every step of the adjudication — plan lookup, `window_start_for`,
/// `next_reset`, `Query::QuotaUsed`, the `used >= tokens` comparison —
/// lives here and nowhere else.
pub fn probe_deferred_by_window(
    store: Option<&Arc<dyn Store>>,
    config: &VadisConfig,
    policy: &PlanPolicyCfg,
    now_epoch_s: u64,
) -> bool {
    let Some(store) = store else {
        return false;
    };
    let Some(provider) = primary_provider(config, policy) else {
        return false;
    };
    let Some(plans) = provider.quota.as_deref() else {
        return false;
    };
    for (plan_idx, q) in plans
        .iter()
        .enumerate()
        .filter(|(_, q)| q.models.iter().any(|m| m == &policy.family))
    {
        let qp = crate::accounting::quota_plan_from_cfg(q);
        let vadis_core::quota::QuotaWindow::Monthly { reset_day } = qp.window;
        let window_start_s = window_start_for(now_epoch_s, reset_day);
        // Next boundary after this window (the next monthly reset).
        let next_boundary_s = next_reset(window_start_s, reset_day);
        if now_epoch_s >= next_boundary_s {
            // The boundary passed: nothing to defer to.
            continue;
        }
        let used = match store.query(Query::QuotaUsed {
            provider: &provider.name,
            plan_idx: plan_idx as u32,
            window_start_us: (window_start_s as i64) * 1_000_000,
        }) {
            Ok(QueryRow::Count(n)) => n.max(0) as u64,
            _ => 0,
        };
        if used >= qp.tokens {
            return true;
        }
    }
    false
}

/// The primary provider's roster entry, if the roster still names it.
fn primary_provider<'a>(
    config: &'a VadisConfig,
    policy: &PlanPolicyCfg,
) -> Option<&'a ProviderCfg> {
    config
        .providers
        .iter()
        .find(|p| p.name == policy.primary.provider)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vadis_core::config::{
        AccountKind, ModelCfg, PriceCfg, ProviderCfg, QuotaCfg, QuotaWindowTag, ResetDayVal,
        TokensVal,
    };
    use vadis_core::store::{CooldownRow, StoreError};

    /// A store double that answers only the two reads these functions
    /// make, from state the test chose — so the adjudication is
    /// unit-pinned without SQLite (the live assemblies are CONF-75..78).
    struct FixedStore {
        cooldown_until_us: Option<i64>,
        quota_used: i64,
    }

    impl Store for FixedStore {
        fn append(
            &self,
            _ev: vadis_core::store::NewEvent<'_>,
        ) -> Result<vadis_core::store::EventId, StoreError> {
            Err(StoreError::Sql("unused".into()))
        }
        fn project(&self, _w: vadis_core::store::ProjectionWrite<'_>) -> Result<(), StoreError> {
            Err(StoreError::Sql("unused".into()))
        }
        fn query(&self, q: Query<'_>) -> Result<QueryRow, StoreError> {
            match q {
                Query::Cooldown { .. } => {
                    Ok(QueryRow::Cooldown(self.cooldown_until_us.map(|until_us| {
                        CooldownRow {
                            scope: "provider".into(),
                            provider: "p-plan".into(),
                            model: String::new(),
                            until_us,
                            reason: "quota_exhausted".into(),
                        }
                    })))
                }
                Query::QuotaUsed { .. } => Ok(QueryRow::Count(self.quota_used)),
                _ => Ok(QueryRow::Count(0)),
            }
        }
        fn rebuild(
            &self,
            _which: vadis_core::store::Projection,
        ) -> Result<vadis_core::store::RebuildStats, StoreError> {
            Err(StoreError::Sql("unused".into()))
        }
        fn schema_version(&self) -> Result<u32, StoreError> {
            Ok(1)
        }
    }

    fn policy(family: &str) -> PlanPolicyCfg {
        PlanPolicyCfg {
            family: family.into(),
            primary: vadis_core::config::RouteSpec {
                provider: "p-plan".into(),
                model: family.into(),
            },
            overflow: vadis_core::config::RouteSpec {
                provider: "p-api".into(),
                model: family.into(),
            },
            on_primary_exhausted: vadis_core::config::OnPrimaryExhausted::Spill,
            recover: vadis_core::config::RecoveryMode::Probe,
            cooldown: vadis_core::config::DurationVal(0),
            overflow_monthly_cap_usd: None,
            overflow_selection: vadis_core::config::OverflowSelection::Declared,
        }
    }

    fn provider(quota: Option<Vec<QuotaCfg>>) -> ProviderCfg {
        ProviderCfg {
            name: "p-plan".into(),
            region: vadis_core::config::Region::Intl,
            currency: vadis_core::Currency::Usd,
            urls: [(
                vadis_core::config::WireApi::Chat,
                "http://127.0.0.1:1/chat/completions".to_string(),
            )]
            .into_iter()
            .collect(),
            api_key_env: Some("K".to_string()),
            api_keys: None,
            wire_api: vadis_core::config::WireApi::Chat,
            supports: vec![vadis_core::config::WireApi::Chat],
            account: AccountKind::CodingPlan,
            models: vec![
                ModelCfg {
                    id: "m1".into(),
                    family: None,
                    context: vadis_core::config::ContextVal(128_000),
                    price: PriceCfg {
                        input_miss: Some(vadis_core::config::PriceVal(0.002)),
                        input_hit: Some(vadis_core::config::PriceVal(0.0002)),
                        cache_write: Some(vadis_core::config::PriceVal(0.0)),
                        output: Some(vadis_core::config::PriceVal(0.004)),
                        peak: peak(),
                        tiers: None,
                    },
                    source: "fixture".into(),
                },
                ModelCfg {
                    id: "m2".into(),
                    family: None,
                    context: vadis_core::config::ContextVal(128_000),
                    price: PriceCfg {
                        input_miss: Some(vadis_core::config::PriceVal(0.002)),
                        input_hit: Some(vadis_core::config::PriceVal(0.0002)),
                        cache_write: Some(vadis_core::config::PriceVal(0.0)),
                        output: Some(vadis_core::config::PriceVal(0.004)),
                        peak: peak(),
                        tiers: None,
                    },
                    source: "fixture".into(),
                },
            ],
            quota,
        }
    }

    fn peak() -> vadis_core::config::PeakCfg {
        vadis_core::config::PeakCfg {
            multiplier: vadis_core::config::MultiplierVal(1.0),
            windows: Vec::new(),
        }
    }

    fn quota(models: &[&str], tokens: u64, reset_day: u8) -> QuotaCfg {
        QuotaCfg {
            models: models.iter().map(|s| s.to_string()).collect(),
            window: QuotaWindowTag::Monthly,
            tokens: TokensVal(tokens),
            reset_day: ResetDayVal(reset_day),
            over_quota: vadis_core::config::OverQuotaTag::Block,
            source: "fixture: availability unit".into(),
        }
    }

    fn config(quota: Option<Vec<QuotaCfg>>) -> VadisConfig {
        VadisConfig {
            server: vadis_core::config::ServerCfg {
                addr: "127.0.0.1:0".into(),
                upstream_attempt_timeout: vadis_core::config::DurationVal(60_000),
                request_timeout: vadis_core::config::DurationVal(600_000),
                auth_token_env: None,
                max_body_bytes: 2_097_152,
                stream_idle_timeout: vadis_core::config::DurationVal(30_000),
            },
            session: vadis_core::config::SessionCfg {
                key_sources: vec!["prompt_cache_key".into()],
                ttl: vadis_core::config::DurationVal(43_200_000),
            },
            cache: vadis_core::config::CacheCfg {
                sticky: true,
                breakeven: vadis_core::config::BreakevenCfg {
                    enabled: true,
                    min_remaining_turns: 2,
                    safety_factor: vadis_core::config::MultiplierVal(1.1),
                },
            },
            trace: vadis_core::config::TraceCfg {
                dir: "./state/traces".into(),
                rollover: vadis_core::config::Rollover::Hourly,
            },
            providers: vec![provider(quota)],
            aliases: Default::default(),
            plugins: Vec::new(),
            fallback: Vec::new(),
            plan_policy: None,
            plan_policies: None,
            state: None,
        }
    }

    // The fixture's price table is the flat 1.0 multiplier with no
    // windows — the same shape every plan rig's YAML declares.
    #[test]
    fn fixture_peak_is_the_flat_multiplier() {
        let p = peak();
        assert_eq!(p.multiplier.0, 1.0);
        assert!(p.windows.is_empty());
    }

    // ------------------------------------------------------------------
    // provider_in_cooldown (L1c)
    // ------------------------------------------------------------------

    #[test]
    fn cooldown_read_compares_against_the_callers_clock() {
        let store: Arc<dyn Store> = Arc::new(FixedStore {
            cooldown_until_us: Some(1_000_000_000),
            quota_used: 0,
        });
        let s = Some(&store);
        // Strictly before, exactly at, and after the stored instant —
        // the boundary is exclusive (`until_us > now`), the caller's
        // `now` decides, and no second clock exists to disagree.
        assert!(provider_in_cooldown(s, "p-plan", 999_999_999));
        assert!(!provider_in_cooldown(s, "p-plan", 1_000_000_000));
        assert!(!provider_in_cooldown(s, "p-plan", 1_000_000_001));
        // No store (an assembly without one): nothing refuses.
        assert!(!provider_in_cooldown(None, "p-plan", 0));
        // A provider with no cooldown row is available.
        let empty: Arc<dyn Store> = Arc::new(FixedStore {
            cooldown_until_us: None,
            quota_used: 0,
        });
        assert!(!provider_in_cooldown(Some(&empty), "p-plan", 0));
    }

    // ------------------------------------------------------------------
    // route_silence_cooling (ADR-056 §2.5, the advisory half)
    // ------------------------------------------------------------------

    /// Answers the cooldown query with a row chosen by the model asked:
    /// the advisory question must see only route-scoped silence rows.
    struct RouteStore {
        row_for_route: Option<CooldownRow>,
        row_for_provider: Option<CooldownRow>,
    }

    impl Store for RouteStore {
        fn append(
            &self,
            _ev: vadis_core::store::NewEvent<'_>,
        ) -> Result<vadis_core::store::EventId, StoreError> {
            Err(StoreError::Sql("unused".into()))
        }
        fn project(&self, _w: vadis_core::store::ProjectionWrite<'_>) -> Result<(), StoreError> {
            Err(StoreError::Sql("unused".into()))
        }
        fn query(&self, q: Query<'_>) -> Result<QueryRow, StoreError> {
            match q {
                // The double must answer the question the *real* store
                // answers: a route row is returned only for the route it
                // names. Answering any `Some(model)` with `row_for_route`
                // would let this double report a provider-wide answer and
                // the test below would pass for the wrong reason.
                Query::Cooldown { provider, model } => Ok(QueryRow::Cooldown(match model {
                    Some(m) => self
                        .row_for_route
                        .clone()
                        .filter(|r| r.provider == provider && r.model == m),
                    None => self
                        .row_for_provider
                        .clone()
                        .filter(|r| r.provider == provider),
                })),
                _ => Ok(QueryRow::Count(0)),
            }
        }
        fn rebuild(
            &self,
            _which: vadis_core::store::Projection,
        ) -> Result<vadis_core::store::RebuildStats, StoreError> {
            Err(StoreError::Sql("unused".into()))
        }
        fn schema_version(&self) -> Result<u32, StoreError> {
            Ok(1)
        }
    }

    fn silence_row(provider: &str, model: &str, until_us: i64) -> CooldownRow {
        CooldownRow {
            scope: "route".into(),
            provider: provider.into(),
            model: model.into(),
            until_us,
            reason: "stream_silence".into(),
        }
    }

    #[test]
    fn silence_cools_only_its_own_route_inside_its_window() {
        let store: Arc<dyn Store> = Arc::new(RouteStore {
            row_for_route: Some(silence_row("p1", "m1", 1_000_000_000)),
            row_for_provider: None,
        });
        let s = Some(&store);
        // The exact route, strictly inside the row's bound: cooling.
        assert!(route_silence_cooling(s, "p1", "m1", 999_999_999));
        // At and past the bound: not cooling (the read is `until_us > now`).
        assert!(!route_silence_cooling(s, "p1", "m1", 1_000_000_000));
        // Another model on the same provider: the union read for that
        // route sees no route row — and no provider-wide row either.
        assert!(!route_silence_cooling(s, "p1", "m2", 999_999_999));
        // No store: nothing cools.
        assert!(!route_silence_cooling(None, "p1", "m1", 0));
    }

    #[test]
    fn a_provider_wide_row_is_not_a_silence_preference() {
        // QuotaExhausted's own shape answering the union read: the row is
        // live and blocks the route, but it is not the advisory word —
        // the binding question (`provider_in_cooldown`) owns it.
        let store: Arc<dyn Store> = Arc::new(RouteStore {
            row_for_route: Some(CooldownRow {
                scope: "provider".into(),
                provider: "p1".into(),
                model: String::new(),
                until_us: 1_000_000_000,
                reason: "quota_exhausted".into(),
            }),
            row_for_provider: Some(CooldownRow {
                scope: "provider".into(),
                provider: "p1".into(),
                model: String::new(),
                until_us: 1_000_000_000,
                reason: "quota_exhausted".into(),
            }),
        });
        let s = Some(&store);
        assert!(!route_silence_cooling(s, "p1", "m1", 999_999_999));
        assert!(provider_in_cooldown(s, "p1", 999_999_999));
    }

    // ------------------------------------------------------------------
    // probe_deferred_by_window (L1d): every adjudication input pinned
    // ------------------------------------------------------------------

    // 2026-09-21T04:06:40Z — inside the monthly window opening 9/1 with
    // reset_day 1 (next boundary 2026-10-01), so `now < boundary` holds.
    const NOW_S: u64 = 1_789_963_600;

    #[test]
    fn window_deferral_requires_exhaustion_inside_an_unreset_window() {
        // Exhausted inside the window: deferred.
        let store: Arc<dyn Store> = Arc::new(FixedStore {
            cooldown_until_us: None,
            quota_used: 100,
        });
        let cfg = config(Some(vec![quota(&["m1"], 100, 1)]));
        let p = policy("m1");
        assert!(probe_deferred_by_window(Some(&store), &cfg, &p, NOW_S));
        // One token short: not deferred.
        let store: Arc<dyn Store> = Arc::new(FixedStore {
            cooldown_until_us: None,
            quota_used: 99,
        });
        assert!(!probe_deferred_by_window(Some(&store), &cfg, &p, NOW_S));
        // No store, no provider quota, a family no plan covers: false.
        assert!(!probe_deferred_by_window(None, &cfg, &p, NOW_S));
        let no_quota = config(None);
        assert!(!probe_deferred_by_window(
            Some(&store),
            &no_quota,
            &p,
            NOW_S
        ));
        let p_m2 = policy("m2");
        assert!(!probe_deferred_by_window(Some(&store), &cfg, &p_m2, NOW_S));
    }

    #[test]
    fn window_deferral_picks_the_plan_covering_the_family() {
        // Two declared plans; only the second covers `m2` (the family).
        // Exhaustion of the first (quota_used) must not defer; the same
        // read against the covering plan's allowance (100) does.
        let store: Arc<dyn Store> = Arc::new(FixedStore {
            cooldown_until_us: None,
            quota_used: 100,
        });
        let cfg = config(Some(vec![quota(&["m1"], 1_000, 1), quota(&["m2"], 100, 1)]));
        let p_m2 = policy("m2");
        assert!(probe_deferred_by_window(Some(&store), &cfg, &p_m2, NOW_S));
        // With the covering plan's allowance raised past the read, the
        // same exhaustion is not deferral — the covering plan decided.
        let cfg = config(Some(vec![quota(&["m1"], 1_000, 1), quota(&["m2"], 101, 1)]));
        assert!(!probe_deferred_by_window(Some(&store), &cfg, &p_m2, NOW_S));
    }

    #[test]
    fn window_deferral_ends_at_the_boundary() {
        // The `now >= next_reset` branch is reachable only in the
        // short-month clamp gap (`quota.rs`): with reset_day 31 the
        // window containing 2026-09-30T12:00:00Z opened 8/31 and its
        // next reset clamps to 9/30 00:00 — already passed — so nothing
        // can be deferred to, whatever the counter reads.
        let gap_s: u64 = 1_790_769_600; // 2026-09-30T12:00:00Z
        let store: Arc<dyn Store> = Arc::new(FixedStore {
            cooldown_until_us: None,
            quota_used: 1_000_000,
        });
        let cfg = config(Some(vec![quota(&["m1"], 100, 31)]));
        let p = policy("m1");
        assert!(!probe_deferred_by_window(Some(&store), &cfg, &p, gap_s));
        // The control one second before the clamped boundary proves the
        // position, not the fixture, decided: 2026-09-29T23:59:59Z sits
        // in the window opening 8/31 with the 9/30 boundary still ahead.
        let inside_s: u64 = gap_s - 12 * 3_600 - 1;
        assert!(probe_deferred_by_window(Some(&store), &cfg, &p, inside_s));
    }
}
