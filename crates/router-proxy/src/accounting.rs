//! End-of-request accounting: build the `DecisionRecord` from the
//! forwarding outcome, append it to the trace, then commit the accounting
//! events with `trace_ref` naming that line — DESIGN §12.10.5 note R2's
//! order: normalize usage → compute cost (and the pure quota `charge`) →
//! **append the trace line** → `cost.computed` + `quota.charged` → release.
//!
//! Also owns the wiring points that precede the outcome: `session.bound`
//! (row 4, the `sessions` projection riding on it) and the `cache_ledger`
//! put anchored on the intent's event id, which is what makes the *next*
//! request's `prefix_continuity` computable — including after a restart
//! (§12.10.6).
//!
//! Usage missing ⇒ zero usage, nothing charged, `usage_missing: true`
//! (spec §8: never guessed). A trace write failure does not block
//! anything: `trace_ref` stays null and the reported record carries
//! `errors[].kind = trace_write_failed`.

use std::time::Instant;

use router_core::config::{AccountKind, QuotaCfg, RouteSpec, RouterConfig};
use router_core::cost::{cost, select_band, CostBreakdown, Currency, Nano, TierTable};
use router_core::prefix::PrefixBlock;
use router_core::quota::{charge, window_start_for, OverQuota, QuotaPlan, QuotaState, QuotaWindow};
use router_core::store::{EventId, EventKind, NewEvent, ProjectionWrite, Query, QueryRow, Store};
use router_core::trace::{
    CostRec, DecisionRec, DecisionRecord, IdentityRec, PlanSwitchRec, PrefixBlockRec, PrefixRec,
    ProtocolRec, QuotaAfter, ResultRec, StateRec, TraceError, TraceWriter, TransformRecord,
    TRACE_SCHEMA_VERSION,
};
use router_core::transform::TransformMode;
use router_core::Usage;
use serde_json::json;

use crate::forward::{ForwardFailure, ForwardSuccess};

/// The route-resolved accounting inputs: the model's price table and the
/// provider's quota plans covering that model, both converted once per
/// request from the config types (the float price conversion never re-runs).
/// `in_plan` mirrors the provider entry's `account` (spec §4.6): the
/// account is a property of the provider, so the roster lookup here is
/// the single place it becomes an accounting fact.
pub struct RouteAccounting {
    /// The route's price block as one `TierTable` per band (spec §4.10):
    /// exactly one element for an entry written flat (the degenerate
    /// one-band table), so the vector's shape — not a second code path —
    /// is what carries backward compatibility. The band is selected per
    /// request by the accounting call site (`select_band` on measured
    /// `usage.input_total`); pre-response figures use the **first** band
    /// (§4.10 rule 8).
    pub prices: Vec<TierTable>,
    pub quota_plans: Vec<QuotaPlan>,
    /// True when the route's provider declares `account: coding_plan`:
    /// every request served by it books the plan's marginal cost 0 while
    /// its `quota_after` is still recorded (spec §4.6 rule 4 / ADR-014
    /// item 6). A quota-less coding_plan provider is still a plan.
    pub in_plan: bool,
    /// The serving entry's currency (spec §4.8): the unit of every
    /// amount this route prices — `cost.currency` on the record and
    /// `"currency"` on the `cost.computed` row.
    pub currency: Currency,
}

/// Resolves a route's accounting inputs from the config (roster lookup +
/// the two conversion points). `None` when the route is not on the roster
/// — callers answer their own 404 before reaching the accountant.
pub fn route_accounting(config: &RouterConfig, route: &RouteSpec) -> Option<RouteAccounting> {
    let provider = config.providers.iter().find(|p| p.name == route.provider)?;
    let model = provider.models.iter().find(|m| m.id == route.model)?;
    let prices = model.price.to_tier_tables(provider.currency).ok()?;
    let quota_plans = provider
        .quota
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .filter(|q| q.models.iter().any(|m| m == &route.model))
        .map(quota_plan_from_cfg)
        .collect();
    let in_plan = provider.account == AccountKind::CodingPlan;
    Some(RouteAccounting {
        prices,
        quota_plans,
        in_plan,
        currency: provider.currency,
    })
}

/// `QuotaCfg` (config) → `QuotaPlan` (model), the one conversion point.
pub fn quota_plan_from_cfg(cfg: &QuotaCfg) -> QuotaPlan {
    QuotaPlan {
        models: cfg.models.clone(),
        window: QuotaWindow::Monthly {
            reset_day: cfg.reset_day.0,
        },
        tokens: cfg.tokens.0,
        over_quota: match cfg.over_quota {
            router_core::config::OverQuotaTag::Block => OverQuota::Block,
            router_core::config::OverQuotaTag::Spill => OverQuota::Spill,
        },
        source: cfg.source.clone(),
    }
}

/// Everything the accounting engine needs from the request's own facts.
/// `now_epoch_s` is the request's single wall-clock read, threaded
/// through — never a second read inside a computation (AGENTS
/// constraint 2).
pub struct AccountCtx<'a> {
    pub request_id: &'a str,
    /// The `request.received` row id — the trace's join anchor (CONF-24).
    pub received_event: Option<EventId>,
    pub proto_in: &'a str,
    /// The outbound protocol when a route was resolved; `None` on a
    /// terminal failure that never reached one (spec §6 `protocol_out`
    /// is nullable for exactly that record).
    pub proto_out: Option<&'a str>,
    pub session: Option<&'a str>,
    pub turn_index: u32,
    pub selection_source: &'a str,
    /// The client's own `model` string, verbatim (§12.10.7 / spec §6);
    /// `None` only on a failure path where none was ever parsed.
    pub requested_model: Option<&'a str>,
    pub decision_ms: u32,
    pub started: Instant,
    pub now_epoch_s: u64,
    /// spec §6 `result.plan_switch`: set by the forwarding path when the
    /// plan policy displaced this request's account (ADR-014); `None` on
    /// every other request (present-and-null on the wire).
    pub plan_switch: Option<PlanSwitchRec>,
    /// spec §6 `state.sticky_hit`: the session already had a binding row
    /// when the request arrived (the sticky-hit input forward.rs
    /// computes for bind_session — one value, both sinks).
    pub sticky_hit: bool,
    /// spec §6 `transform_mode`: the mode in effect for this request's
    /// outbound body (ADR-019). `Passthrough` on every request that did
    /// not ask and on every pre-pipeline refusal (a 401 never ran the
    /// chain, so nothing is claimed); `Transform` when the header asked.
    pub transform_mode: TransformMode,
    /// The transform chain's ledger (spec §6 `transforms[]`): one entry
    /// per step that changed the payload. A failure path keeps whatever
    /// steps had already applied — an entry describes the plan, not what
    /// reached the wire (that is `result.status`'s answer).
    pub transforms: Vec<TransformRecord>,
    /// The applier's fail-safe entry (DESIGN §12.12's failure table): when
    /// a plan's edits could not be spliced, the body was forwarded
    /// unedited, `transforms[]` is empty ("asked, not applied" — the
    /// countable state the separate mode field exists for) and the
    /// degradation is declared here as `errors[].kind = transform_error`,
    /// never as a partial edit.
    pub transform_error: Option<TraceError>,
}

/// The per-request accounting outcome the caller reports.
pub struct AccountResult {
    pub record: DecisionRecord,
    /// The durable `"file:line"` pointer the accounting rows carry;
    /// `None` when the trace write failed (spec §8: the request is
    /// unaffected and the failure is explicit in `record.errors`).
    pub trace_ref: Option<String>,
}

pub struct Accountant<'a> {
    pub store: Option<&'a dyn Store>,
    pub trace: Option<&'a dyn TraceWriter>,
    pub accounting: Option<&'a RouteAccounting>,
}

/// The spec §6 trace error for a client-facing failure (the kind
/// vocabulary mapping lives in router-core).
pub fn trace_error_for_failure(f: &ForwardFailure) -> TraceError {
    TraceError {
        kind: TraceError::kind_for_code(f.code).to_string(),
        message: f.message.clone(),
        plugin: None,
        details: f.details.clone(),
    }
}

impl<'a> Accountant<'a> {
    /// §12.10.5 row 4: `session.bound` (FULL) when the binding is created
    /// or moved; a sticky hit on an unchanged route writes nothing. The
    /// `sessions` projection rides on the event row.
    pub fn bind_session(
        &self,
        ctx: &AccountCtx<'_>,
        provider: &str,
        model: &str,
        sticky_hit: bool,
        route_changed: bool,
        ttl_us: i64,
    ) {
        let Some(store) = self.store else { return };
        let Some(session) = ctx.session else { return };
        if sticky_hit && !route_changed {
            return;
        }
        let ev = store.append(NewEvent {
            kind: EventKind::SessionBound,
            request_id: Some(ctx.request_id),
            session: Some(session),
            body_hash: None,
            trace_ref: None,
            payload: json!({
                "session_key": session,
                "provider": provider,
                "model": model,
                "ttl_us": ttl_us,
            }),
        });
        if let Ok(ev) = ev {
            let _ = store.project(ProjectionWrite::SessionBound {
                session_key: session,
                provider,
                model,
                ttl_us,
                last_event: ev,
            });
        }
    }

    /// The request's `prefix_continuity` against the session's ledger
    /// (§12.10.6): hashes compared from block 0; `None` when the session
    /// has no previous request — an absent measurement is absent.
    pub fn continuity(&self, session: Option<&str>, blocks: &[PrefixBlock]) -> Option<f64> {
        let store = self.store?;
        let session = session?;
        let QueryRow::CacheLedger(prev) = store
            .query(Query::CacheLedgerBlocks {
                session_key: session,
            })
            .ok()?
        else {
            return None;
        };
        if prev.is_empty() || blocks.is_empty() {
            return None;
        }
        // Only hashes participate (the fidelity signal); the ledger's
        // block kind is metadata for money-side figures.
        let prev_hashes: Vec<&str> = prev.iter().map(|b| b.hash.as_str()).collect();
        let cur_hashes: Vec<&str> = blocks.iter().map(|b| b.hash.as_str()).collect();
        let common = prev_hashes
            .into_iter()
            .zip(cur_hashes)
            .take_while(|(a, b)| a == b)
            .count();
        // Spec §6: the ratio is relative to the previous request, so the
        // denominator is its block count (mirrors `prefix_continuity`,
        // router-core — one formula, two call sites, same shape).
        let denom = prev.len();
        Some(common as f64 / denom as f64)
    }

    /// Replace the session's block set in the `cache_ledger` projection,
    /// anchored on `last_event` — the intent that carried the same blocks
    /// in its payload, which is what the rebuild path reads (CONF-21).
    pub fn put_ledger(&self, session: Option<&str>, blocks: &[PrefixBlock], last_event: EventId) {
        let Some(store) = self.store else { return };
        let Some(session) = session else { return };
        let rows: Vec<(u32, &str, u64, &str)> = blocks
            .iter()
            .map(|b| (b.index, b.kind.as_str(), b.tokens, b.hash.as_str()))
            .collect();
        let _ = store.project(ProjectionWrite::CacheLedgerPut {
            session_key: session,
            blocks: &rows,
            last_event,
        });
    }

    /// End-of-request accounting (§12.10.5 rows 10–11 + note R2). Builds
    /// the record, appends the trace line, then commits `cost.computed`
    /// and `quota.charged` with `trace_ref` naming that line. Returns the
    /// record plus the pointer (for callers that surface the join).
    ///
    /// Success outcomes only — every terminal failure goes through
    /// [`Accountant::finish_failure`], so a `DecisionRecord` lands on
    /// **all** terminal paths (spec §6's one line per request; failures
    /// are when the analysis truth matters most).
    pub fn finish(
        &self,
        ctx: &AccountCtx<'_>,
        outcome: &ForwardSuccess,
        blocks: &[PrefixBlock],
        upstream_ms: Option<u32>,
    ) -> AccountResult {
        let failover_from = outcome
            .failover_from
            .as_ref()
            .map(|r| format!("{}/{}", r.provider, r.model));
        self.commit(
            ctx,
            outcome.status,
            Some(outcome.status),
            &outcome.route.provider,
            &outcome.route.model,
            outcome.usage,
            outcome.usage.is_none(),
            failover_from,
            Vec::new(),
            blocks,
            upstream_ms,
        )
    }

    /// The **shared terminal-failure record**: one call site per
    /// forwarding path — upstream 4xx/5xx exhausted, connect failure,
    /// timeout / unknown_outcome, parse/route/capability rejections —
    /// landing one `DecisionRecord` with `errors[0]` naming the
    /// client-facing failure (`error_class` / `stage` in `details`) and
    /// `usage_missing: true` (no usage arrived, so nothing is charged and
    /// no cost is invented, spec §8). The trace write itself already
    /// follows §8: a failure there is recorded, never blocking.
    ///
    /// `attempted_route` names the route that actually failed (the
    /// failure-analysis first question — "which provider died") and
    /// `upstream_status` mirrors the upstream's own status into
    /// `result.upstream_status` alongside `errors[].details` (the
    /// field completion: both paths, both fields, no blanks).
    #[allow(clippy::too_many_arguments)]
    pub fn finish_failure(
        &self,
        ctx: &AccountCtx<'_>,
        failure: &ForwardFailure,
        attempted_route: Option<(&str, &str)>,
        upstream_status: Option<u16>,
        blocks: &[PrefixBlock],
        upstream_ms: Option<u32>,
        failover_from: Option<String>,
    ) -> AccountResult {
        // The details carry the upstream status when one exists; it also
        // mirrors into result.upstream_status so the record answers both
        // "what did the client see" and "what did the upstream say".
        let mut failure = failure.clone();
        if let Some(status) = upstream_status {
            let details = failure.details.get_or_insert_with(|| serde_json::json!({}));
            if let Some(obj) = details.as_object_mut() {
                obj.entry("upstream_status")
                    .or_insert_with(|| serde_json::json!(status));
            }
        }
        self.commit(
            ctx,
            failure.status,
            upstream_status,
            attempted_route.map(|(p, _)| p).unwrap_or(""),
            attempted_route.map(|(_, m)| m).unwrap_or(""),
            None,
            true,
            failover_from,
            vec![trace_error_for_failure(&failure)],
            blocks,
            upstream_ms,
        )
    }

    /// The stream path's entry (the relay owns its facts): same §12.10.5 note R2 order,
    /// same events. `extra_errors` carries any truncation record.
    #[allow(clippy::too_many_arguments)]
    pub fn finish_stream(
        &self,
        ctx: &AccountCtx<'_>,
        status: u16,
        provider: &str,
        model: &str,
        usage: Option<Usage>,
        usage_missing: bool,
        failover_from: Option<String>,
        extra_errors: Vec<TraceError>,
        blocks: &[PrefixBlock],
        upstream_ms: Option<u32>,
    ) -> AccountResult {
        self.commit(
            ctx,
            status,
            Some(status),
            provider,
            model,
            usage,
            usage_missing,
            failover_from,
            extra_errors,
            blocks,
            upstream_ms,
        )
    }

    /// The shared end-of-request commit (§12.10.5 note R2's order).
    #[allow(clippy::too_many_arguments)]
    fn commit(
        &self,
        ctx: &AccountCtx<'_>,
        status: u16,
        upstream_status: Option<u16>,
        provider: &str,
        model: &str,
        usage: Option<Usage>,
        usage_missing: bool,
        failover_from: Option<String>,
        extra_errors: Vec<TraceError>,
        blocks: &[PrefixBlock],
        upstream_ms: Option<u32>,
    ) -> AccountResult {
        let mut errors = extra_errors;
        // The applier's fail-safe, if it fired, is the request's first
        // error: it happened at the transform-chain stage, before anything
        // downstream (DESIGN §12.12's failure-table order).
        if let Some(te) = ctx.transform_error.clone() {
            errors.insert(0, te);
        }
        let overhead_ms = ctx.started.elapsed().as_millis() as u32;
        let usage = usage.unwrap_or_default();

        let continuity = self.continuity(ctx.session, blocks);

        // Cost (row 10's input) and the pure quota charge — both computed
        // before the trace line so `quota_after` reflects the post-charge
        // state without rewriting anything.
        let mut breakdown = CostBreakdown {
            input_miss: Nano(0),
            input_hit: Nano(0),
            cache_write: Nano(0),
            output: Nano(0),
            peak_applied_pct: 100,
            total: Nano(0),
            // The failure/usage-missing default: no table priced this
            // record, so the unit is the USD default — and every amount
            // is 0, which is why the choice cannot mislead (spec §4.8).
            currency: Currency::Usd,
        };
        let mut quota_after: Option<QuotaAfter> = None;
        if !usage_missing {
            if let Some(acc) = self.accounting {
                if acc.in_plan {
                    // Spec §4.6 rule 4 / ADR-014 item 6: the account a
                    // request was billed under follows from the serving
                    // provider's `account` — a coding_plan provider's
                    // requests book the plan's marginal cost 0. Every
                    // bucket stays at the zero initialized above (the
                    // switch's own re-prefill cost is priced separately
                    // in `plan.switched`, not here); the quota charge
                    // below still runs so `quota_after` is recorded
                    // ("…and record `quota_after`").
                } else {
                    // spec §4.10 rule 2: the band is selected from the
                    // request's measured total input tokens (cached
                    // included) — the only place a record's cost.* is
                    // computed prices the whole request with one band.
                    let table = select_band(&acc.prices, usage.input_total);
                    breakdown = cost(&usage, table, ctx.now_epoch_s);
                }
                if let Some(store) = self.store {
                    if let Some((plan_idx, plan)) = acc
                        .quota_plans
                        .iter()
                        .enumerate()
                        .find(|(_, p)| p.models.iter().any(|m| m == model))
                    {
                        let reset_day = match plan.window {
                            QuotaWindow::Monthly { reset_day } => reset_day,
                        };
                        let window_start_us =
                            window_start_for(ctx.now_epoch_s, reset_day) as i64 * 1_000_000;
                        let used = store
                            .query(Query::QuotaUsed {
                                provider,
                                plan_idx: plan_idx as u32,
                                window_start_us,
                            })
                            .ok()
                            .and_then(|q| match q {
                                QueryRow::Count(n) => Some(n.max(0) as u64),
                                _ => None,
                            })
                            .unwrap_or(0);
                        let mut st = QuotaState {
                            plan_idx,
                            window_start_epoch_s: (window_start_us / 1_000_000).max(0) as u64,
                            tokens_used: used,
                        };
                        let verdict = charge(plan, &mut st, &usage, ctx.now_epoch_s);
                        quota_after = Some(QuotaAfter::from_verdict(provider, plan, &st, &verdict));
                    }
                }
            }
        }

        let mut record = DecisionRecord {
            schema_version: TRACE_SCHEMA_VERSION,
            ts: rfc3339_millis(ctx.now_epoch_s),
            identity: IdentityRec {
                request_id: ctx.request_id.to_string(),
                event_id: ctx.received_event.map(|e| e.0).unwrap_or(0),
                client: "other",
                session: ctx.session.map(str::to_string),
                thread_id: None,
                turn_index: ctx.turn_index,
            },
            protocol: ProtocolRec {
                protocol_in: ctx.proto_in.to_string(),
                protocol_out: ctx.proto_out.map(str::to_string),
                // `translated` is the mapper event (ADR-022 / DESIGN
                // §12.10.9): true only when the attempt that carried the
                // request re-encoded the body through a translation
                // mapper. v0.1 has no mapper, so it is false on every
                // record this build writes — a comparison of two words
                // was the R11-F1 fabrication this replaces.
                translated: false,
                lossy: Vec::new(),
            },
            decision: DecisionRec {
                provider: provider.to_string(),
                model: model.to_string(),
                requested_model: ctx.requested_model.map(str::to_string),
                selection_source: ctx.selection_source.to_string(),
                plugin_chain: Vec::new(),
                decision_ms: ctx.decision_ms,
            },
            state: StateRec {
                stateful_inbound: false,
                sticky_hit: ctx.sticky_hit,
                cache_control_breaks: 0,
            },
            prefix: PrefixRec {
                blocks: blocks
                    .iter()
                    .map(|b| PrefixBlockRec {
                        kind: b.kind.as_str(),
                        index: b.index,
                        tokens: b.tokens,
                        hash: b.hash.clone(),
                    })
                    .collect(),
                continuity,
            },
            transform_mode: ctx.transform_mode,
            transforms: ctx.transforms.clone(),
            usage,
            usage_missing,
            cost: CostRec {
                input_miss: breakdown.input_miss,
                input_hit: breakdown.input_hit,
                cache_write: breakdown.cache_write,
                output: breakdown.output,
                peak_applied_pct: breakdown.peak_applied_pct,
                total: breakdown.total,
                // §4.8: the serving entry's own unit, from the accounting
                // the route resolved (a usage-missing record keeps the USD
                // default with all-zero amounts).
                currency: breakdown.currency,
                quota_after: quota_after.clone(),
            },
            result: ResultRec {
                status,
                upstream_status,
                failover_from,
                plan_switch: ctx.plan_switch.clone(),
                overhead_ms,
                upstream_ms,
            },
            errors,
        };

        // The trace line (§12.10.5 note R2: it precedes the accounting rows). One
        // write; the pointer feeds both accounting events.
        let trace_ref = match self.trace.map(|t| t.write(&record)) {
            Some(Ok(p)) => p,
            Some(Err(_)) => None,
            None => None,
        };
        let trace_ref = match trace_ref {
            Some(p) => Some(p),
            None => {
                // Spec §8: the request is unaffected, the missing
                // observation is explicit.
                record.errors.push(TraceError::trace_write_failed(
                    "trace line could not be written",
                ));
                None
            }
        };

        // Rows 10 + 11 (FULL), only when usage arrived — `usage_missing`
        // charges nothing and invents no cost (§12.10.5 row 9).
        if !usage_missing {
            if let Some(store) = self.store {
                let ptr = trace_ref.as_deref();
                let _ = store.append(NewEvent {
                    kind: EventKind::CostComputed,
                    request_id: Some(ctx.request_id),
                    session: ctx.session,
                    body_hash: None,
                    trace_ref: ptr,
                    payload: json!({
                        // `route` names the provider/model this cost was
                        // priced at — the plan policy's overflow-cap spend
                        // query sums on it (DESIGN §12.10.8: measured
                        // usage priced by the config table, no second
                        // counter). `currency` (§4.8) denominates every
                        // `*_nano` here — the row is read on its own, and
                        // the OverflowSpend projection is route-scoped,
                        // hence single-currency by construction.
                        "route": format!("{}/{}", provider, model),
                        "currency": breakdown.currency.as_code(),
                        "input_miss_nano": breakdown.input_miss.0,
                        "input_hit_nano": breakdown.input_hit.0,
                        "cache_write_nano": breakdown.cache_write.0,
                        "output_nano": breakdown.output.0,
                        "peak_applied_pct": breakdown.peak_applied_pct,
                        "total_nano": breakdown.total.0,
                    }),
                });
                if let (Some(acc), Some(qa)) = (&self.accounting, &quota_after) {
                    if let Some((plan_idx, plan)) = acc
                        .quota_plans
                        .iter()
                        .enumerate()
                        .find(|(_, p)| p.models.iter().any(|m| m == &record.decision.model))
                    {
                        let reset_day = match plan.window {
                            QuotaWindow::Monthly { reset_day } => reset_day,
                        };
                        let window_start_us =
                            window_start_for(ctx.now_epoch_s, reset_day) as i64 * 1_000_000;
                        // Blocked charges nothing (the guard rejects before
                        // the wire in the full pipeline; here the charge
                        // event records the verdict with zero tokens).
                        let tokens = if qa.verdict == "blocked" {
                            0
                        } else {
                            usage.input_total.saturating_add(usage.output)
                        };
                        if let Ok(ev) = store.append(NewEvent {
                            kind: EventKind::QuotaCharged,
                            request_id: Some(ctx.request_id),
                            session: ctx.session,
                            body_hash: None,
                            trace_ref: ptr,
                            payload: json!({
                                "provider": record.decision.provider,
                                "plan_idx": plan_idx,
                                "window_start_us": window_start_us,
                                "tokens": tokens,
                                "tokens_used_after": qa.tokens_used,
                                "tokens_limit": qa.tokens_limit,
                                "verdict": qa.verdict,
                            }),
                        }) {
                            let _ = store.project(ProjectionWrite::QuotaCharged {
                                provider: &record.decision.provider,
                                plan_idx: plan_idx as u32,
                                window_start_us,
                                tokens: tokens as i64,
                                last_event: ev,
                            });
                        }
                    }
                }
            }
        }

        AccountResult { record, trace_ref }
    }
}

/// RFC3339 UTC, millisecond precision — from the request's single clock
/// read (`now_epoch_s`), never a second one.
pub fn rfc3339_millis(epoch_s: u64) -> String {
    let ms_total = epoch_s * 1000;
    let secs = ms_total / 1000;
    let millis = ms_total % 1000;
    let (y, mo, d, h, mi, s) = civil_from_epoch(secs as i64);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{millis:03}Z")
}

/// Epoch seconds → civil UTC (Hinnant's algorithm; mirrors router-core's
/// peak module, kept local for the same dependency discipline).
pub fn civil_from_epoch(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (
        if m <= 2 { y + 1 } else { y },
        m,
        d,
        (sod / 3600) as u32,
        ((sod % 3600) / 60) as u32,
        (sod % 60) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_known_instant() {
        // Anchors verified with `date -u -r` (not a snapshot: the epoch →
        // civil relation round-trips through civil_from_epoch below).
        assert_eq!(rfc3339_millis(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339_millis(1_789_256_462), "2026-09-12T23:41:02.000Z");
        assert_eq!(rfc3339_millis(951_827_696), "2000-02-29T12:34:56.000Z");
    }

    #[test]
    fn kind_mapping_is_total() {
        use router_core::error::ErrorCode;
        let f = ForwardFailure {
            status: 502,
            code: ErrorCode::UpstreamError,
            message: "x".into(),
            details: None,
        };
        let te = trace_error_for_failure(&f);
        assert_eq!(te.kind, "upstream_error");
    }

    #[test]
    fn civil_epoch_relations() {
        assert_eq!(civil_from_epoch(0), (1970, 1, 1, 0, 0, 0));
        // 86_400 s later is the next day, same time.
        let a = civil_from_epoch(1_000_000);
        let b = civil_from_epoch(1_000_000 + 86_400);
        assert_eq!(a.3, b.3);
        assert_eq!(a.2 + 1, b.2);
    }

    /// The migration before/after same-price witness (R15-2's acceptance
    /// criterion): a flat entry served through the new `prices` vector
    /// path prices the same usage exactly as the old single-table
    /// arithmetic did — one band, selected at every `n`, identical money.
    #[test]
    fn flat_entry_through_the_band_path_prices_identically() {
        use router_core::cost::{select_band, Usage};
        const AT: u64 = 1_789_992_000;

        let cfg: RouterConfig = serde_json::from_str(
            r#"{"server": {"addr": "127.0.0.1:8790", "upstream_attempt_timeout": "60s", "request_timeout": "10m"},
                "session": {"key_sources": ["prompt_cache_key"], "ttl": "12h"},
                "cache": {"sticky": true, "breakeven": {"enabled": true, "min_remaining_turns": 3, "safety_factor": 1.2}},
                "trace": {"dir": "./state/traces", "rollover": "hourly"},
                "providers": [{
                    "name": "p", "urls": {"chat": "https://x.example/v1/chat/completions"},
                    "api_key_env": "P_KEY", "wire_api": "chat", "supports": ["chat"],
                    "models": [{
                        "id": "m1", "context": "200k",
                        "price": {"input_miss": 0.00066, "input_hit": 0.000022,
                                  "cache_write": 0.0, "output": 0.00198,
                                  "peak": {"multiplier": 1.0, "windows": []}},
                        "source": "https://x.example/pricing @2026-09-19"}],
                    "quota": [{"models": ["m1"], "window": "monthly", "tokens": 100000000,
                               "reset_day": 1, "over_quota": "block", "source": "s"}]
                }],
                "aliases": {"fast": "p/m1"},
                "plugins": [],
                "fallback": ["p/m1"]}"#,
        )
        .expect("parses");
        let acc = route_accounting(
            &cfg,
            &RouteSpec {
                provider: "p".into(),
                model: "m1".into(),
            },
        )
        .expect("on the roster");
        assert_eq!(acc.prices.len(), 1, "a flat entry is one band");
        assert_eq!(acc.prices[0].up_to, None);

        let usage = Usage {
            input_total: 145_000,
            input_cached: 144_000,
            cache_write: 500,
            output: 800,
            reasoning: 12,
        };
        // The serving-path line (accounting.rs:437's shape) vs the old
        // single-table arithmetic: identical breakdown at any n.
        let through_bands = cost(&usage, select_band(&acc.prices, usage.input_total), AT);
        let direct = cost(&usage, &acc.prices[0].table, AT);
        assert_eq!(through_bands, direct);
        // Hand-computed control: the same numbers §4.0's flat block has
        // always produced for this usage.
        // miss 1_000 × 660_000/1000 = 660_000
        // hit 144_000 × 22_000/1000 = 3_168_000
        // write 500 × 0 = 0
        // out 812 × 1_980_000/1000 = 1_607_760
        assert_eq!(through_bands.input_miss, Nano(660_000));
        assert_eq!(through_bands.input_hit, Nano(3_168_000));
        assert_eq!(through_bands.cache_write, Nano(0));
        assert_eq!(through_bands.output, Nano(1_607_760));
        assert_eq!(
            through_bands.total,
            Nano(660_000 + 3_168_000 + 0 + 1_607_760)
        );
    }
}
