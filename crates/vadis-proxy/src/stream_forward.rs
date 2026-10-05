//! The streaming forwarding engine: byte-faithful SSE
//! passthrough per DESIGN §12.10.3 R1–R11, with the same closing stages
//! as the buffered path.
//!
//! The relay is a byte-level operation: the bytes that reach the client
//! are the bytes the upstream sent, in arrival order (§12.10.3 R1); each
//! read is written as soon as it is available (§12.10.3 R2); the head goes
//! first and is never given an invented `content-length` (§12.10.3 R3); an
//! idle gap past `upstream_attempt_timeout` ends the relay (§12.10.3 R4);
//! dropping the response future drops the upstream stream, closing the
//! connection (§12.10.3 R5); a
//! mid-stream failure **after the first relayed byte** truncates —
//! recorded, never retried, never masked with a fabricated terminal
//! event (R6); the usage tap reads a copy of the relayed bytes off the
//! relay path (R7, R8); usage no carrier delivered is
//! `usage_missing: true`, zero usage, nothing charged (spec §8).
//!
//! The same books (§12.10.5 note R3): the pre-flight runs the
//! buffered path's rules (parse → session resolution → route →
//! capability → outbound bytes → the same event vocabulary), and the
//! relay's end runs the buffered path's closing stages — the
//! `upstream.responded` event (the stream-specific observations
//! `stream_completed` / `bytes_relayed` ride on it), then §12.10.5 note R2's
//! order: normalize usage → cost → the `DecisionRecord` trace line →
//! `cost.computed` + `quota.charged` → the ledger put that makes the
//! *next* request's `prefix_continuity` computable. A stream that died
//! mid-way says so in `errors[]` and is never billed twice. A terminal
//! failure before the relay (parse, route, capability, a failure head,
//! a connect failure, `unknown_outcome`) writes its trace line through
//! the same `Accountant::finish_failure` seam as the buffered path.
//!
//! The zero-byte branch of R6: a mid-stream failure with **no byte relayed
//! yet** may fail over (the client has observed only the 200 SSE head, no
//! event), walking the same fallback chain as the buffered path with the
//! same provider-exclusion and cooldown state. Once any byte is relayed,
//! no retry of any kind happens.
//!
//! Pre-relay connect failures classify through `transport_cause` evidence
//! exactly like the buffered path: a connection failure is
//! `connect_failure`, never `timeout`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures::stream::{unfold, Stream};
use serde_json::{json, Value};
use vadis_core::config::{PlanPolicyCfg, ProviderCfg, RouteSpec, WireApi};
use vadis_core::error::ErrorCode;
use vadis_core::error_class::{classify_upstream_error, ErrorEvidence, TransportCause};
use vadis_core::prefix::{attribute_tokens, extract_prefix_blocks, PrefixBlock};
use vadis_core::store::{EventId, EventKind, NewEvent, Store};
use vadis_core::trace::TraceError;
use vadis_protocol::sse::{SseUsageExtractor, SseUsageOutcome};

use crate::accounting::{AccountCtx, Accountant, RouteAccounting};
use crate::forward::{
    resolve_session_key, rewrite_outbound_model, turn_index_for, ForwardFailure, Forwarder,
    RequestFacts, REASON_PRIMARY_COOLING_DOWN,
};
use vadis_core::plan::{
    account_of_route, displacement_reason, plan_tier, route_in_family, REASON_PRIMARY_RECOVERED,
};
use vadis_core::trace::PlanSwitchRec;

/// The streaming answer: the head already decided (status + content
/// type) and a body stream of the upstream's bytes, verbatim.
pub struct StreamSuccess {
    pub status: u16,
    pub content_type: Option<String>,
    /// The route that answered.
    pub route: RouteSpec,
    /// Set when the request switched away from a failed route before any
    /// byte was relayed (spec §6).
    pub failover_from: Option<RouteSpec>,
    pub body: std::pin::Pin<Box<dyn Stream<Item = Bytes> + Send>>,
}

pub enum StreamOutcome {
    Success(StreamSuccess),
    Failure(ForwardFailure),
}

/// One failover candidate, owned by the relay so it borrows nothing from
/// the engine: the route plus everything needed to open it.
#[derive(Clone)]
struct Candidate {
    route: RouteSpec,
    /// This candidate's position in the offered chain (resolved route
    /// first, then the fallback entries in config order, spec §4.2) — the
    /// position the walk's `demoted` skip entry must carry so `skipped[]`
    /// serialises in the chain's own order on both media (ADR-024 ruling 2;
    /// the buffered walk is in chain order by construction because its
    /// eligibility and its skipping share one pass).
    chain_pos: usize,
    wire: WireApi,
    /// The URL this candidate POSTs to, resolved from the entry's `urls` map
    /// for `wire` (spec §4.9, ADR-020). Resolved once, when the candidate is
    /// built, so the attempt path composes nothing.
    url: String,
    /// The provider's credential pool (ADR-049 §3): the present names'
    /// values, in rotation order. A single-credential provider has a
    /// one-entry pool — its relay is exactly what it was before the
    /// pool existed.
    api_keys: Vec<String>,
    /// The pool cursor this candidate's walk is on: the index of the key
    /// the NEXT attempt presents (ADR-049 §3 rule 6 — a request never
    /// re-attempts a key it has already tried).
    key_index: usize,
}

/// The relay's own context, fully owned (`'static`): what the stream
/// needs to account, classify and fail over after the handler returned.
/// It carries the request's accounting facts (the `AccountCtx`
/// inputs) and the route-resolved price/quota tables, so the relay's end
/// can run the buffered path's closing stages without borrowing the
/// engine.
struct RelayCtx {
    store: Option<Arc<dyn Store>>,
    trace: Option<Arc<dyn vadis_core::TraceWriter>>,
    /// Provider name → its accounting inputs, resolved once per request
    /// for every candidate on the chain (the failover answer is priced
    /// at its own provider's table).
    accounting: HashMap<String, RouteAccounting>,
    idle: Duration,
    request_id: String,
    proto_in: WireApi,
    session: Option<String>,
    turn_index: u32,
    requested_model: Option<String>,
    selection_source: &'static str,
    decision_ms: u32,
    started: Instant,
    now_epoch_s: u64,
    received_event: Option<EventId>,
    /// Prefix blocks of the cleaned body (hashes only until usage
    /// attribution runs at stream end).
    blocks: Vec<PrefixBlock>,
    /// chat only: the client asked for `stream_options.include_usage`.
    client_requested_usage: bool,
    /// spec §6 `result.plan_switch` (ADR-014): the displacement the
    /// pre-flight recorded, carried to the relay's terminal record.
    plan_switch: Option<vadis_core::trace::PlanSwitchRec>,
    /// spec §6 `state.sticky_hit`: the session already had a binding row
    /// at pre-flight (the same value bind_session received).
    sticky_hit: bool,
    /// The transform chain's ledger entries (spec §6 `transforms[]`),
    /// written with the plan at the composition step; kept on a failure
    /// path — an entry describes the plan, never a claim the bytes left
    /// the process (DESIGN §12.12's failure table).
    transform_mode: vadis_core::transform::TransformMode,
    transform_records: Vec<vadis_core::trace::TransformRecord>,
    /// The applier's fail-safe entry, if the plan's edits could not be
    /// spliced (spec §8 / DESIGN §12.12).
    transform_error: Option<vadis_core::trace::TraceError>,
    /// The exact-match response cache (spec §4.17, ADR-042): the one
    /// store's handle plus this request's key, `Some` only when the
    /// capability is mounted AND this request has a session (a
    /// sessionless request is neither looked up nor stored — fail-closed,
    /// §3.3). The relay records ONLY on a completed stream.
    cache: Option<(
        Arc<dyn vadis_core::response_cache::ResponseCache>,
        vadis_core::response_cache::ResponseKey,
    )>,
    /// The answering head's content-type — a recorded response replays it.
    content_type: Option<String>,
}

/// The relay's mutable state, threaded through `unfold`.
struct RelayState {
    /// The unread upstream response being drained.
    head: vadis_providers::stream::StreamHead,
    /// The accounting tap over a copy of the relayed bytes (R7).
    tap: SseUsageExtractor,
    /// Whether any byte has been relayed — R6's boundary.
    relayed: bool,
    /// Candidates not yet attempted (fallback chain minus the primary).
    remaining: Vec<Candidate>,
    /// Providers already attempted in this request (exclusion, §4.2).
    attempted: Vec<String>,
    /// The route currently answering (for events).
    route_label: String,
    /// The answering route's wire protocol (the trace's `protocol_out`).
    wire: WireApi,
    /// The answering attempt's head latency (`result.upstream_ms`).
    head_ms: Option<u32>,
    /// The intent row that carries this attempt's prefix blocks — the
    /// ledger put's anchor (§12.10.6).
    intent: Option<EventId>,
    /// The first route that failed pre-relay, for `failover_from`.
    failover_from: Option<RouteSpec>,
    /// The recording buffer (spec §4.17, ADR-042 §4.4): a copy of the
    /// relayed bytes, kept ONLY while the stream is a recording
    /// candidate — dropped on a spill past the store's byte bound, never
    /// stored on a truncation or an abort (fail-closed, §3.3). `None`
    /// when the capability is off or the request is sessionless, so the
    /// ordinary stream path buffers nothing.
    record_buf: Option<Vec<u8>>,
    /// The answering attempt's pool cursor (ADR-049 §3): the index of
    /// the credential that served — the trace's `decision.key_index`
    /// when the answering provider holds more than one.
    key_index: usize,
    /// Whether the answering provider's pool is real (>1 present key) —
    /// `decision.key_index` is null on the single-credential spelling.
    multi_key: bool,
}

fn now_us() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0)
}

impl Forwarder {
    /// Forwards one streaming request. On success the caller receives the
    /// head plus the byte relay; the relay owns the single open upstream
    /// connection and closes it when dropped (§12.10.3 R5). Every terminal
    /// failure writes its `DecisionRecord` before the outcome leaves the
    /// engine — the same `finish_failure` seam as the
    /// buffered path.
    pub async fn forward_stream(
        &self,
        proto_in: WireApi,
        body: &[u8],
        request_id: &str,
        headers: &[(String, String)],
        transform_mode: vadis_core::transform::TransformMode,
    ) -> StreamOutcome {
        // One clock read, two projections — the buffered path's twin.
        let now_us = now_us();
        let mut facts = RequestFacts {
            request_id,
            received_event: None,
            proto_in,
            proto_out: None,
            session: None,
            turn_index: 1,
            requested_model: None,
            selection_source: "explicit",
            decision_ms: 0,
            started: Instant::now(),
            now_epoch_s: (now_us / 1_000_000).max(0) as u64,
            now_us,
            blocks: Vec::new(),
            failover_from: None,
            upstream_ms: None,
            attempted_route: None,
            last_upstream_status: None,
            plan_switch: None,
            // The one sticky read (spec §6): false until the session is
            // resolved (see below) — one value per request.
            sticky_hit: false,
            transform_mode,
            transform_records: Vec::new(),
            transform_error: None,
        };
        let outcome = self
            .forward_stream_inner(&mut facts, proto_in, body, request_id, headers)
            .await;
        if let StreamOutcome::Failure(f) = &outcome {
            self.record_failure_trace(&facts, f);
        }
        outcome
    }

    /// Pre-flight + relay setup: the same parse → session → route →
    /// capability → outbound bytes rules as the buffered path (R11), the
    /// same event vocabulary (rows 1–5 of §12.10.5), and a pre-relay
    /// candidate walk that classifies with the same evidence inputs.
    async fn forward_stream_inner(
        &self,
        facts: &mut RequestFacts<'_>,
        proto_in: WireApi,
        body: &[u8],
        request_id: &str,
        headers: &[(String, String)],
    ) -> StreamOutcome {
        let started = facts.started;
        let now_epoch_s = facts.now_epoch_s;
        let decision_start = Instant::now();
        // Parse only to read `model` and `stream_options`; the forwarded
        // bytes are the raw original, never a reserialization.
        let parsed: Value = match serde_json::from_slice(body) {
            Ok(v) => v,
            Err(e) => {
                return StreamOutcome::Failure(ForwardFailure {
                    status: 400,
                    code: ErrorCode::InvalidRequest,
                    message: format!("request body is not parsable JSON: {e}"),
                    details: None,
                })
            }
        };
        let Some(model) = parsed.get("model").and_then(|m| m.as_str()) else {
            return StreamOutcome::Failure(ForwardFailure {
                status: 400,
                code: ErrorCode::InvalidRequest,
                message: "request body is missing the string field 'model'".into(),
                details: None,
            });
        };
        facts.requested_model = Some(model.to_string());
        if model == "auto" {
            return StreamOutcome::Failure(ForwardFailure {
                status: 400,
                code: ErrorCode::AutoNotSupported,
                message: "model 'auto' is not supported in v0.1 (a plugin takes it over; the slot is reserved)"
                    .into(),
                details: None,
            });
        }
        let (mut primary, selection_source) = match resolve_route(&self.config, model) {
            Ok(r) => r,
            Err(f) => return StreamOutcome::Failure(f),
        };
        facts.selection_source = selection_source;
        // Session resolution — the shared helper, moved ahead of
        // the plan guard (which reads `turn_index` from the sticky
        // projection, ADR-014 item 3).
        let session = resolve_session_key(&self.config, &parsed, headers);
        let turn_index = turn_index_for(&self.store, session.as_deref());
        // The one sticky read (spec §6), the buffered path's twin: fixed
        // here, before the binding write below and before the relay
        // carries the facts — the trace record's value and
        // `bind_session`'s early return are THIS value, never a second
        // read taken after the write (R11-F2). The row is retained
        // whole: its `(provider, model)` is the prior `route_changed`
        // compares against (note R6 — one read, both inputs).
        let prior_binding = crate::forward::prior_session_binding(&self.store, session.as_deref());
        let sticky_hit = prior_binding.is_some();
        facts.session = session.clone();
        facts.turn_index = turn_index;
        facts.sticky_hit = sticky_hit;
        // The plan policy's Guard stage (spec §4.6) — the same rule the
        // buffered path runs, before any attempt. The family key is the
        // resolution's own tag (§4.8; ADR-049 §4) — the buffered path's
        // twin resolves it the same way.
        let mut plan_guard_out: Option<crate::forward::PlanGuardOutcome> = None;
        let plan_policy: Option<PlanPolicyCfg> =
            self.config.family_policy_for_route(&primary).cloned();
        match self.plan_guard(
            plan_policy.as_ref(),
            &primary,
            session.as_deref(),
            turn_index,
            now_epoch_s,
            facts.now_us,
        ) {
            Ok(None) => {}
            Ok(Some(g)) => {
                if g.route != primary {
                    // Reason by DIRECTION — the destination account
                    // (spec §6's producer table, extended by ADR-049
                    // §5.2's reason-by-direction rule; the buffered
                    // path's twin): a move to the metered tier is
                    // `primary_exhausted`, a move within the plan tier
                    // is `plan_exhausted`, a move back into the tier is
                    // `primary_recovered` — never the pre-request
                    // account state.
                    if let Some(policy) = plan_policy.as_ref() {
                        let tier = plan_tier(&self.config, policy);
                        facts.plan_switch = Some(PlanSwitchRec {
                            from: primary.to_string(),
                            to: g.route.to_string(),
                            reason: displacement_reason(policy, &tier, &primary, &g.route),
                            probe: g.probe,
                            reprefill_tokens: None,
                            switch_cost_nano: None,
                            // §4.8: the destination route's unit (the buffered
                            // path's twin writes the same value).
                            cost_currency: self.route_currency(&g.route),
                        });
                    }
                } else if g.probe {
                    // The admitted probe's return trip (spec §6): the
                    // same record the buffered path writes — the
                    // displacement is from the STATE's route (the
                    // metered tier), the way back costs 0 (in-plan
                    // destination).
                    if let Some(policy) = plan_policy.as_ref() {
                        facts.plan_switch = Some(PlanSwitchRec {
                            from: policy.overflow.to_string(),
                            to: policy.primary.to_string(),
                            reason: REASON_PRIMARY_RECOVERED,
                            probe: true,
                            reprefill_tokens: None,
                            switch_cost_nano: Some(0),
                            cost_currency: self.route_currency(&policy.primary),
                        });
                    }
                }
                primary = g.route.clone();
                plan_guard_out = Some(g);
            }
            Err(f) => return StreamOutcome::Failure(f),
        }
        let Some(provider) = provider_cfg(&self.config, &primary) else {
            return StreamOutcome::Failure(ForwardFailure {
                status: 404,
                code: ErrorCode::UnknownProvider,
                message: format!("unknown provider '{}'", primary.provider),
                details: None,
            });
        };
        if !provider.supports.contains(&proto_in) {
            let supports: Vec<&str> = provider.supports.iter().map(|w| w.as_str()).collect();
            return StreamOutcome::Failure(ForwardFailure {
                status: 400,
                code: ErrorCode::CapabilityUnsupported,
                message: format!(
                    "inbound protocol '{}' is not declared in supports {:?} of provider '{}'",
                    proto_in, supports, provider.name
                ),
                details: Some(json!({
                    "provider": provider.name,
                    "protocol_in": proto_in.as_str(),
                    "supports": supports,
                })),
            });
        }
        if provider.wire_api != proto_in {
            return StreamOutcome::Failure(ForwardFailure {
                status: 501,
                code: ErrorCode::NotImplemented,
                message: format!(
                    "translation {} -> {} is not implemented in v0.1; only native routes are served",
                    proto_in, provider.wire_api
                ),
                details: None,
            });
        }

        // The outbound base (DESIGN §12.10.7): the client's bytes minus
        // vadis-owned top-level keys — mutation (a), once per request. The
        // `model` rewrite (mutation (b)) is per attempt: the primary below,
        // each failover candidate inside the relay.
        let cleaned = match vadis_core::RawBody::new(body.to_vec())
            .remove_top_level_keys(vadis_core::VADIS_OWNED_TOP_LEVEL_KEYS)
        {
            Ok(b) => b,
            Err(e) => {
                return StreamOutcome::Failure(ForwardFailure {
                    status: 400,
                    code: ErrorCode::InvalidRequest,
                    message: format!(
                        "request body is not a well-formed top-level JSON object: {e:?}"
                    ),
                    details: None,
                })
            }
        };
        // The transform chain stage, the buffered path's twin (DESIGN
        // §12.12: the same request with a different relay — one shared
        // composition step, one set of invariants, no second copy of the
        // edit path).
        let (base, ledger, applier_error) = crate::forward::compose_transform_stage(
            self.transform_engine.as_deref(),
            facts.transform_mode,
            &cleaned,
            proto_in,
        );
        facts.transform_records = ledger;
        facts.transform_error = applier_error;
        let outbound_hash = vadis_core::prefix::body_sha16(base.as_bytes());
        facts.blocks = extract_prefix_blocks(&base).unwrap_or_default();

        // Rows 1 + 3 of the §12.10.5 wiring (FULL + NORMAL), same payload
        // shape as the buffered path plus the stream marker.
        facts.received_event = self.append_event(
            EventKind::RequestReceived,
            request_id,
            Some(&outbound_hash),
            json!({
                "protocol_in": proto_in.as_str(),
                "protocol_out": null,
                "client": null,
                "session": session,
                "turn_index": turn_index,
                "body_hash": outbound_hash,
                "stream": true,
            }),
            session.as_deref(),
        );
        let decision_ms = decision_start.elapsed().as_millis() as u32;
        facts.decision_ms = decision_ms;
        facts.proto_out = Some(provider.wire_api);
        self.append_event(
            EventKind::DecisionMade,
            request_id,
            None,
            json!({
                "provider": primary.provider,
                "model": primary.model,
                "requested_model": model,
                "selection_source": selection_source,
                "protocol_out": provider.wire_api.as_str(),
                "decision_ms": decision_ms,
                "stream": true,
            }),
            session.as_deref(),
        );
        // Row 2 — the transform chain point, kept even when the chain is
        // empty (parity with the buffered path's event vocabulary and
        // payload shape: §12.10.5 row 2's own fields, nothing more —
        // DESIGN §12.12 adds no event-payload field).
        self.append_event(
            EventKind::TransformApplied,
            request_id,
            None,
            json!({
                "plugin": facts.transform_records.first().map(|r| r.plugin.clone()),
                "chain": facts.transform_records.iter().map(|r| json!({
                    "plugin": r.plugin,
                    "added_input_tokens": r.added_input_tokens,
                    "saved_input_tokens": r.saved_input_tokens,
                    "cache_impact": r.cache_impact,
                    "verdict": r.verdict,
                })).collect::<Vec<_>>(),
                "changed": !facts.transform_records.is_empty(),
            }),
            session.as_deref(),
        );
        // Row 4 — session.bound through the accountant's one writer, the
        // same call the buffered path makes. `route_changed` (note R6)
        // is measured from the same session-resolution read's row
        // against the route resolved AFTER the guard chain (`primary`
        // here) and before the attempt — the buffered path's twin value,
        // element for element.
        let route_changed = match &prior_binding {
            Some(prior) => prior.provider != primary.provider || prior.model != primary.model,
            None => false,
        };
        if session.is_some() {
            crate::accounting::Accountant {
                store: self.store.as_deref(),
                trace: self.trace.as_deref(),
                accounting: None,
            }
            .bind_session(
                &AccountCtx {
                    request_id,
                    received_event: facts.received_event,
                    proto_in: proto_in.as_str(),
                    proto_out: Some(provider.wire_api.as_str()),
                    session: session.as_deref(),
                    turn_index,
                    selection_source,
                    requested_model: Some(model),
                    // No attempt yet: the pool's credential is a walk
                    // fact (ADR-049 §3).
                    key_index: None,
                    decision_ms,
                    started,
                    now_epoch_s,
                    plan_switch: facts.plan_switch.clone(),
                    // The one value (spec §6): the read from session
                    // resolution, before this binding write.
                    sticky_hit,
                    transform_mode: facts.transform_mode,
                    transforms: facts.transform_records.clone(),
                    transform_error: facts.transform_error.clone(),
                },
                &primary.provider,
                &primary.model,
                sticky_hit,
                route_changed,
                self.session_ttl_us,
            );
        }

        // The exact-match response cache (spec §4.17; ADR-042 §4.4), the
        // buffered path's twin: the lookup is the **last step before the
        // upstream attempt** — after admission, the bounded body read,
        // route resolution, the guard chain and the transform compose
        // step — and nowhere earlier. A hit replaces exactly one thing:
        // the call. The digest is taken over the client's inbound body
        // bytes **as received** (§3.1); a sessionless request has no key
        // and is neither looked up nor stored (§3.3).
        let cache_key = match (&self.response_cache, session.as_deref()) {
            (Some(_), Some(session_key)) => {
                Some(vadis_core::response_cache::ResponseKey::for_request(
                    &vadis_core::response_cache::RequestFacts {
                        protocol_in: proto_in.as_str(),
                        config_digest: self.trace.as_ref().map(|t| t.config_digest()).unwrap_or(""),
                        session: session_key,
                        transform_mode: facts.transform_mode,
                        body,
                    },
                ))
            }
            _ => None,
        };
        if let (Some(cache), Some(key)) = (&self.response_cache, &cache_key) {
            if let Some(recorded) = cache.lookup(key) {
                // A hit is a **replay, not a prediction** (ADR-042 §3.2):
                // the recorded response bytes are returned verbatim, as
                // one stream item — no claim about what the upstream
                // would answer now, no upstream call at all.
                let hit = vadis_core::trace::CacheRec {
                    verdict: "inferred",
                    key_digest: key.key_digest_hex(),
                    replayed: vadis_core::trace::ReplayedRef {
                        request_id: recorded.source.request_id.clone(),
                        session: recorded.source.session.clone(),
                        turn_index: recorded.source.turn_index,
                    },
                    replayed_bytes: recorded.body.len() as u64,
                };
                let ctx = AccountCtx {
                    request_id,
                    received_event: facts.received_event,
                    proto_in: proto_in.as_str(),
                    // No bytes left the process — spec §6's third
                    // `protocol_out` class.
                    proto_out: None,
                    session: session.as_deref(),
                    turn_index,
                    selection_source,
                    requested_model: Some(model),
                    // The replay made no upstream attempt: no
                    // credential served (ADR-042 §4.3).
                    key_index: None,
                    decision_ms,
                    started,
                    now_epoch_s,
                    plan_switch: facts.plan_switch.clone(),
                    sticky_hit,
                    transform_mode: facts.transform_mode,
                    transforms: facts.transform_records.clone(),
                    transform_error: facts.transform_error.clone(),
                };
                crate::accounting::Accountant {
                    store: self.store.as_deref(),
                    trace: self.trace.as_deref(),
                    // No usage exists to price: the record is
                    // `usage_missing`-class, charged nowhere and summed
                    // nowhere (spec §6, ADR-042 §4.3).
                    accounting: None,
                }
                .finish_replay(
                    &ctx,
                    hit,
                    recorded.status,
                    &primary.provider,
                    &primary.model,
                    &facts.blocks,
                );
                let body = Bytes::from(recorded.body);
                return StreamOutcome::Success(StreamSuccess {
                    status: recorded.status,
                    content_type: recorded.content_type.clone(),
                    route: primary.clone(),
                    failover_from: None,
                    body: Box::pin(futures::stream::once(std::future::ready(body))),
                });
            }
        }

        // The primary candidate plus the fallback chain (spec §4.2),
        // filtered to native routes with a key present at startup. For a
        // request inside a plan family the family's `overflow` route is
        // the first candidate after primary (spec §4.2 refined by §4.6).
        // Entries the filter refuses are classified (ADR-022 / DESIGN
        // §12.10.9) so the walk-end refusal can say why — the buffered
        // walk answers "which candidates may serve" with the same rule:
        // provider entry, key, and `wire_api == proto_in`.
        let mut candidates: Vec<Candidate> = Vec::new();
        // Construction-time eligibility skips, each carrying its chain
        // position (ADR-024 ruling 2) so the serialiser can emit the
        // chain's own order whichever stage appended an entry.
        let mut skipped: Vec<ChainSkipped> = Vec::new();
        let mut chain_pos: usize = 0;
        {
            // The offered chain (spec §4.2 refined by §4.6 / ADR-049
            // §5.1/§5.7): the family's ACTIVE route, the plan tier's
            // remaining members in declaration order, the family's
            // `overflow` route, then the global fallback list. The guard
            // already moved `primary` to the active route, and the tier
            // members BEFORE it are the drained ones (§5.2's coherence
            // rule) — deliberately absent, exactly the buffered path's
            // twin chain.
            let mut routes: Vec<RouteSpec> = vec![primary.clone()];
            if let Some(policy) = &plan_policy {
                let tier = plan_tier(&self.config, policy);
                if route_in_family(policy, &tier, &primary) {
                    if let Some(active_pos) = tier.iter().position(|r| r == &primary) {
                        for member in tier.iter().skip(active_pos + 1) {
                            if !routes.contains(member) {
                                routes.push(member.clone());
                            }
                        }
                    }
                    if !routes.contains(&policy.overflow) {
                        routes.push(policy.overflow.clone());
                    }
                }
            }
            for route in routes
                .into_iter()
                .chain(self.config.fallback.iter().cloned())
            {
                // The offered-chain position (ADR-024 ruling 2): every
                // route this loop visits — candidate or skip — advances
                // it, so a skip entry can name where in the chain it
                // was refused (spec §4.2's order).
                let pos = chain_pos;
                chain_pos += 1;
                if candidates.iter().any(|c| c.route == route) {
                    continue;
                }
                if skipped.iter().any(|s| s.skip.0 == route) {
                    continue;
                }
                let Some(cfg) = self
                    .config
                    .providers
                    .iter()
                    .find(|p| p.name == route.provider)
                else {
                    skipped.push(ChainSkipped {
                        pos,
                        skip: (route.clone(), crate::forward::SKIP_UNKNOWN_PROVIDER),
                    });
                    continue;
                };
                if cfg.wire_api != proto_in {
                    // The wire gate (ADR-022): the skip is in the keyless
                    // class — never attempted, never narrated as a
                    // displacement; the refusal reports it at the walk's
                    // end.
                    skipped.push(ChainSkipped {
                        pos,
                        skip: (route.clone(), crate::forward::SKIP_WIRE_MISMATCH),
                    });
                    continue;
                }
                if !self.api_keys.contains_key(&cfg.name) {
                    skipped.push(ChainSkipped {
                        pos,
                        skip: (route.clone(), crate::forward::SKIP_KEYLESS),
                    });
                    continue;
                }
                let Some(url) = cfg.url_for(cfg.wire_api) else {
                    return StreamOutcome::Failure(ForwardFailure {
                        status: 500,
                        code: ErrorCode::Internal,
                        message: format!(
                            "provider '{}' declares '{}' but carries no URL for it",
                            cfg.name,
                            cfg.wire_api.as_str()
                        ),
                        details: Some(json!({"stream": true})),
                    });
                };
                candidates.push(Candidate {
                    route: route.clone(),
                    chain_pos: pos,
                    wire: cfg.wire_api,
                    url: url.to_string(),
                    // ADR-049 §3 rule 1: the provider's **present**
                    // pool, in declaration order — the candidate owns
                    // its pool so the relay's rotation arm can advance
                    // within the provider after the handler returned.
                    api_keys: self.api_keys[&cfg.name].clone(),
                    key_index: 0,
                });
            }
        }
        if candidates.is_empty() {
            // The frozen `no_available_route` refusal (ADR-022 / spec §8):
            // one shape for both media — this arm differs from the
            // buffered walk end by nothing but the pre-existing
            // `"stream": true`.
            return StreamOutcome::Failure(ForwardFailure {
                status: 502,
                code: ErrorCode::UpstreamError,
                message:
                    "no available route: every candidate provider is demoted, keyless or unavailable"
                        .into(),
                details: Some(json!({
                    "stage": "no_available_route",
                    "skipped": chain_ordered_skipped_json(&skipped),
                    "upstream_status": None::<u16>,
                    "error_class": None::<&'static str>,
                    "stream": true,
                })),
            });
        }

        // The chain's accounting inputs, resolved once (the relay's end
        // prices the answering route at its own provider's table).
        let mut accounting: HashMap<String, RouteAccounting> = HashMap::new();
        for c in &candidates {
            if !accounting.contains_key(&c.route.provider) {
                if let Some(a) = crate::accounting::route_accounting(&self.config, &c.route) {
                    accounting.insert(c.route.provider.clone(), a);
                }
            }
        }

        let idle = Duration::from_millis(self.config.server.upstream_attempt_timeout.0);
        let client = match vadis_providers::stream::ReqwestStreamClient::new(idle) {
            Ok(c) => c,
            Err(e) => {
                return StreamOutcome::Failure(ForwardFailure {
                    status: 500,
                    code: ErrorCode::Internal,
                    message: format!("cannot build the streaming transport: {e}"),
                    details: None,
                })
            }
        };

        // The pre-relay candidate walk: classify with the same evidence
        // inputs as the buffered path, record, act. Nothing observable
        // has left the vadis until a 2xx head is relayed.
        let mut attempted: Vec<String> = Vec::new();
        let mut attempt_index: u32 = 0;
        let mut failover_from: Option<RouteSpec> = None;
        let mut last_upstream_status: Option<u16> = None;
        let mut last_class: Option<vadis_core::error_class::ErrorClass> = None;
        // The construction-time eligibility skips (wire/keyless/unknown),
        // carried to the walk-end refusal (ADR-022): a candidate the walk
        // attempts and then demotes is inserted below with `demoted` at its
        // own chain position, so the frozen shape's `skipped[]` holds every
        // candidate the walk refused without attempting, in the chain's own
        // order on both media (ADR-024 ruling 2).
        let mut walk_skipped: Vec<ChainSkipped> = skipped;

        // The family's primary refused by ADR-011's cooldown projection
        // before any attempt (spec §6's producer table, CONF-42): set by
        // the skip below, consumed by the first candidate actually
        // attempted.
        let mut cooling_abandoned: Option<RouteSpec> = None;

        let mut cand_i = 0usize;
        while cand_i < candidates.len() {
            let mut cand = candidates[cand_i].clone();
            cand_i += 1;
            {
                if attempted.iter().any(|p| p == &cand.route.provider) {
                    continue;
                }
                if self.provider_in_cooldown(&cand.route.provider) {
                    // The pre-attempt cooldown skip (ADR-011 item 4; spec §6's
                    // `failover_from` table, row 2): the projection refused
                    // this candidate before any attempt, so the walk moves on
                    // — the skip *abandons* the route and `failover_from`
                    // names it, exactly like a failed attempt would.
                    if failover_from.is_none() {
                        failover_from = Some(cand.route.clone());
                    }
                    walk_skipped.push(ChainSkipped {
                        pos: cand.chain_pos,
                        skip: (cand.route.clone(), crate::forward::SKIP_DEMOTED),
                    });
                    if let Some(policy) = &plan_policy {
                        if cand.route == policy.primary {
                            cooling_abandoned = Some(cand.route.clone());
                        }
                    }
                    continue;
                }
                attempted.push(cand.route.provider.clone());
                let route_label = format!("{}/{}", cand.route.provider, cand.route.model);
                facts.attempted_route = Some(cand.route.clone());

                // The cooling displacement's record (spec §6's producer table,
                // CONF-42), the streaming twin of the buffered walk's fill:
                // the family's primary was refused by the cooldown projection
                // before any attempt and THIS candidate is the one actually
                // serving the request. Trace row only — no `plan.switched`
                // event, no `plan_state` move (§4.6 rule 3); the figures
                // follow the failover's price convention (ADR-011 item 9).
                if let Some(abandoned) = cooling_abandoned.take() {
                    if facts.plan_switch.is_none() {
                        let (reprefill, cost_nano) =
                            self.failover_cost(session.as_deref(), &cand.route);
                        facts.plan_switch = Some(PlanSwitchRec {
                            from: abandoned.to_string(),
                            to: cand.route.to_string(),
                            reason: REASON_PRIMARY_COOLING_DOWN,
                            probe: false,
                            reprefill_tokens: reprefill,
                            switch_cost_nano: cost_nano,
                            cost_currency: self.route_currency(&cand.route),
                        });
                    }
                }

                // Mutation (b), per attempt (§12.10.7): this route's native id.
                let outbound = match rewrite_outbound_model(&base, &cand.route.model) {
                    Ok(b) => Bytes::from(b.into_owned()),
                    Err(e) => {
                        return StreamOutcome::Failure(ForwardFailure {
                            status: 500,
                            code: ErrorCode::Internal,
                            message: format!(
                                "cannot rewrite the outbound model to the native id: {e:?}"
                            ),
                            details: Some(json!({"stream": true})),
                        })
                    }
                };
                let attempt_hash = vadis_core::prefix::body_sha16(&outbound);

                // Row 5 — the intent, FULL, committed before the wire
                // (CONF-20). The payload carries the session's prefix blocks
                // (the ledger rebuild reads them, §12.10.6); `body_hash` is
                // this attempt's byte-final bytes (§12.10.5 note R4).
                let intent = NewEvent {
                    kind: EventKind::UpstreamSubmitted,
                    request_id: Some(request_id),
                    session: session.as_deref(),
                    body_hash: Some(&attempt_hash),
                    trace_ref: None,
                    payload: json!({
                        "route": route_label,
                        "attempt_index": attempt_index,
                        "protocol_out": cand.wire.as_str(),
                        "stream": true,
                        "prefix_blocks": facts.blocks.iter().map(|b| json!({
                            "index": b.index, "kind": b.kind.as_str(),
                            "tokens": b.tokens, "hash": b.hash,
                        })).collect::<Vec<_>>(),
                    }),
                };
                let intent_id = if let Some(store) = &self.store {
                    match store.append(intent) {
                        Ok(id) => Some(id),
                        Err(_) => {
                            // CONF-22: nothing reached the upstream; the
                            // client's retry is safe.
                            return StreamOutcome::Failure(ForwardFailure {
                                status: 500,
                                code: ErrorCode::Internal,
                                message: "the state store rejected the upstream intent".into(),
                                details: Some(json!({"stage": "intent"})),
                            });
                        }
                    }
                } else {
                    None
                };

                let attempt_started = Instant::now();
                // The rotation arm (ADR-049 §3 rule 3), the streaming twin:
                // a credential-class failure whose pool still holds an
                // untried key retries the SAME route on the next key —
                // `rotate_credential` with the index, no demotion,
                // `failover_from` untouched. Only an exhausted pool leaves
                // the provider. `quota_exhausted` never rotates (rule 4).
                'keys: loop {
                    match open_head(&client, &cand, &outbound).await {
                        OpenHead::Head(head) => {
                            let head_ms = Some(attempt_started.elapsed().as_millis() as u32);
                            facts.upstream_ms = head_ms;
                            last_upstream_status = Some(head.status);
                            facts.last_upstream_status = Some(head.status);
                            if (200..300).contains(&head.status) {
                                // A probe succeeded (ADR-014 item 3): a 2xx head on
                                // the primary while the family was on overflow
                                // flips the family back and records it.
                                if plan_guard_out.as_ref().is_some_and(|g| g.probe) {
                                    if let Some(policy) = plan_policy.as_ref() {
                                        // The return trip's trace row (spec §6),
                                        // the buffered path's twin: set HERE —
                                        // only a 2xx head is a recovery — with
                                        // the displacement from the STATE's
                                        // route (overflow) and a 0 cost back.
                                        if facts.plan_switch.is_none() {
                                            facts.plan_switch = Some(PlanSwitchRec {
                                                from: policy.overflow.to_string(),
                                                to: policy.primary.to_string(),
                                                reason: REASON_PRIMARY_RECOVERED,
                                                probe: true,
                                                reprefill_tokens: None,
                                                switch_cost_nano: Some(0),
                                                cost_currency: self.route_currency(&policy.primary),
                                            });
                                        }
                                        self.plan_probe_succeeded(
                                            request_id,
                                            policy,
                                            session.as_deref(),
                                        );
                                    }
                                }
                                // The relay takes it from here: the head is 2xx,
                                // the accounting facts travel with the stream.
                                let content_type = head.content_type.clone();
                                let client_requested_usage = parsed
                                    .get("stream_options")
                                    .and_then(|o| o.get("include_usage"))
                                    .and_then(|b| b.as_bool())
                                    .unwrap_or(false);
                                let ctx = RelayCtx {
                                    store: self.store.clone(),
                                    trace: self.trace.clone(),
                                    accounting,
                                    idle,
                                    request_id: request_id.to_string(),
                                    proto_in,
                                    session: session.clone(),
                                    turn_index,
                                    requested_model: Some(model.to_string()),
                                    selection_source,
                                    decision_ms,
                                    started,
                                    now_epoch_s,
                                    received_event: facts.received_event,
                                    blocks: facts.blocks.clone(),
                                    client_requested_usage,
                                    plan_switch: facts.plan_switch.clone(),
                                    // The one value (spec §6): the read from
                                    // session resolution, carried through — the
                                    // relay's record is not a second opinion.
                                    sticky_hit,
                                    transform_mode: facts.transform_mode,
                                    transform_records: facts.transform_records.clone(),
                                    transform_error: facts.transform_error.clone(),
                                    cache: match (&self.response_cache, &cache_key) {
                                        (Some(cache), Some(key)) => {
                                            Some((Arc::clone(cache), key.clone()))
                                        }
                                        _ => None,
                                    },
                                    content_type: content_type.clone(),
                                };
                                let state = RelayState {
                                    tap: SseUsageExtractor::new(cand.wire, client_requested_usage),
                                    head,
                                    relayed: false,
                                    // The tail of the chain after the answering
                                    // candidate — owned, because this loop
                                    // consumed the vector (into_iter).
                                    remaining: candidates.clone(),
                                    attempted,
                                    route_label,
                                    wire: cand.wire,
                                    head_ms,
                                    intent: intent_id,
                                    failover_from: failover_from.clone(),
                                    // Only a request the capability records buffers a
                                    // copy; the ordinary stream path holds nothing.
                                    record_buf: ctx.cache.is_some().then(Vec::new),
                                    key_index: cand.key_index,
                                    multi_key: cand.api_keys.len() > 1,
                                };
                                facts.failover_from = failover_from.clone();
                                let relay =
                                    relay_stream(ctx, state, Bytes::from(base.as_bytes().to_vec()));
                                return StreamOutcome::Success(StreamSuccess {
                                    status: 200,
                                    content_type,
                                    route: cand.route.clone(),
                                    failover_from,
                                    body: Box::pin(relay),
                                });
                            }
                            // A failure status head before any relayed byte is the
                            // ordinary error path (R6 column 1), classified per
                            // DESIGN §12.10.3 R12: the head's OWN answer — status,
                            // headers and body bytes — through the provider
                            // layer's one reader, the same evidence expression
                            // the buffered path feeds the classifier. The read is
                            // bounded by R4's idle bound alone (no byte cap); a
                            // read that ends short leaves the classifier the
                            // bytes that arrived.
                            let status = head.status;
                            let resp = head.into_upstream_response(idle).await;
                            let evidence = ErrorEvidence {
                                status: Some(status),
                                retry_after: resp.retry_after.as_deref(),
                                body: &resp.body,
                                wrote_full_request: true,
                                transport_cause: None,
                            };
                            let cls = classify_upstream_error(&evidence);
                            last_class = Some(cls.class);
                            // The rotation arm (ADR-049 §3 rule 3, CONF-91(a)),
                            // the streaming twin: rotate within the provider
                            // before any fallover; the index, never the value.
                            if cls.class.is_credential_class()
                                && cand.key_index + 1 < cand.api_keys.len()
                            {
                                let (reprefill, cost_nano) =
                                    self.failover_cost(session.as_deref(), &cand.route);
                                self.record_rotation(
                                    request_id,
                                    attempt_index,
                                    Some(status),
                                    &cls,
                                    (cand.key_index + 1) as u32,
                                    reprefill,
                                    cost_nano,
                                );
                                cand.key_index += 1;
                                attempt_index += 1;
                                continue 'keys;
                            }
                            let classified_id = self.record_classification(
                                request_id,
                                attempt_index,
                                Some(status),
                                &cls,
                            );
                            self.apply_demotion(&cand.route.provider, &cls, classified_id);
                            // The plan policy's only account-moving signal
                            // (ADR-014 item 2; ADR-049 §5.2 across the
                            // tier — the buffered path's twin): a 403
                            // `quota_exhausted` on one of the family's
                            // PLAN routes retires that plan and moves the
                            // family to the walk's next step — the next
                            // plan member (`plan_exhausted`, in-plan), the
                            // metered tier otherwise
                            // (`primary_exhausted`) — before the next
                            // intent. `block` refuses while the WHOLE
                            // tier is exhausted.
                            if cls.class.demotes_provider() {
                                if let Some(policy) = plan_policy.as_ref() {
                                    let tier = plan_tier(&self.config, policy);
                                    if tier.contains(&cand.route) {
                                        let from = cand.route.clone();
                                        let next_plan = tier
                                            .iter()
                                            .position(|r| r == &cand.route)
                                            .and_then(|pos| tier.get(pos + 1))
                                            .cloned();
                                        let to = next_plan
                                            .clone()
                                            .unwrap_or_else(|| policy.overflow.clone());
                                        let to_account =
                                            account_of_route(&tier, &to).as_str().to_string();
                                        self.record_plan_switch(
                                            request_id,
                                            policy,
                                            &from,
                                            &to,
                                            &to_account,
                                            false,
                                            session.as_deref(),
                                        );
                                        if policy.on_primary_exhausted
                                            == vadis_core::config::OnPrimaryExhausted::Block
                                        {
                                            // Block refuses only once every
                                            // plan is drained — i.e. when
                                            // the move above left the tier.
                                            if to_account == "overflow" {
                                                return StreamOutcome::Failure(ForwardFailure {
                                                    status: 429,
                                                    code: ErrorCode::QuotaExceeded,
                                                    message: format!(
                                                        "plan family '{}' is exhausted and \
                                             on_primary_exhausted is 'block': the request is \
                                             refused rather than served from the metered \
                                             account (spec 4.6)",
                                                        policy.family
                                                    ),
                                                    details: Some(json!({
                                                        "family": policy.family,
                                                        "account_state": "overflow",
                                                        "reason": "quota_exhausted",
                                                        "stream": true,
                                                    })),
                                                });
                                            }
                                        }
                                        if facts.plan_switch.is_none() {
                                            let (reprefill, cost_nano) =
                                                self.failover_cost(session.as_deref(), &to);
                                            facts.plan_switch = Some(PlanSwitchRec {
                                                from: from.to_string(),
                                                to: to.to_string(),
                                                reason: displacement_reason(
                                                    policy, &tier, &from, &to,
                                                ),
                                                probe: false,
                                                reprefill_tokens: reprefill,
                                                switch_cost_nano: cost_nano,
                                                cost_currency: self.route_currency(&to),
                                            });
                                        }
                                    }
                                }
                            }
                            self.append_event(
                                EventKind::UpstreamResponded,
                                request_id,
                                None,
                                json!({
                                    "status": status,
                                    "route": route_label,
                                    "attempt_index": attempt_index,
                                    "wrote_full_request": true,
                                    "stream": true,
                                    "usage": Value::Null,
                                }),
                                session.as_deref(),
                            );
                            if let Some(next) =
                                self.next_candidate_for(&candidates, &attempted, cls.class)
                            {
                                let (reprefill, switch_cost) =
                                    self.failover_cost(session.as_deref(), &next);
                                self.record_failover(
                                    request_id,
                                    attempt_index,
                                    &cand.route,
                                    &next,
                                    &cls,
                                    session.as_deref(),
                                    reprefill,
                                    switch_cost,
                                );
                                if failover_from.is_none() {
                                    failover_from = Some(cand.route.clone());
                                }
                                facts.failover_from = failover_from.clone();
                                attempt_index += 1;
                                break 'keys;
                            }
                            let deterministic = matches!(
                                cls.class,
                                vadis_core::error_class::ErrorClass::FormatError
                                    | vadis_core::error_class::ErrorClass::ContentPolicyBlocked
                            );
                            return StreamOutcome::Failure(ForwardFailure {
                                status: 502,
                                code: ErrorCode::UpstreamError,
                                message: if deterministic {
                                    format!(
                                "upstream rejected the request deterministically ({}); it is not retried",
                                cls.class.as_str()
                            )
                                } else {
                                    format!(
                                        "upstream error ({}) and the fallback chain is exhausted",
                                        cls.class.as_str()
                                    )
                                },
                                details: Some(json!({
                                    "upstream_status": status,
                                    "error_class": cls.class.as_str(),
                                    "stream": true,
                                })),
                            });
                        }
                        OpenHead::NotSent(kind, code, message) => {
                            // No request bytes went out: nothing billed. The
                            // transport kind is the classification evidence —
                            // the same inputs the buffered path feeds the
                            // classifier, so a connect failure is
                            // `connect_failure` on both paths. No
                            // upstream.responded row: the attempt never reached
                            // the upstream.
                            let evidence = ErrorEvidence {
                                status: None,
                                retry_after: None,
                                body: b"",
                                wrote_full_request: false,
                                transport_cause: Some(match kind {
                                    vadis_providers::TransportKind::Connect => {
                                        TransportCause::Connect
                                    }
                                    vadis_providers::TransportKind::Timeout => {
                                        TransportCause::Timeout
                                    }
                                    vadis_providers::TransportKind::Other => TransportCause::Other,
                                }),
                            };
                            let cls = classify_upstream_error(&evidence);
                            last_class = Some(cls.class);
                            // The rotation arm, transport twin (ADR-049 §3 rule
                            // 3): no bytes billed, so the rotation is exactly as
                            // safe as the fallover it precedes.
                            if cls.class.is_credential_class()
                                && cand.key_index + 1 < cand.api_keys.len()
                            {
                                let (reprefill, cost_nano) =
                                    self.failover_cost(session.as_deref(), &cand.route);
                                self.record_rotation(
                                    request_id,
                                    attempt_index,
                                    None,
                                    &cls,
                                    (cand.key_index + 1) as u32,
                                    reprefill,
                                    cost_nano,
                                );
                                cand.key_index += 1;
                                attempt_index += 1;
                                continue 'keys;
                            }
                            let classified_id =
                                self.record_classification(request_id, attempt_index, None, &cls);
                            self.apply_demotion(&cand.route.provider, &cls, classified_id);
                            if let Some(next) =
                                self.next_candidate_for(&candidates, &attempted, cls.class)
                            {
                                let (reprefill, switch_cost) =
                                    self.failover_cost(session.as_deref(), &next);
                                self.record_failover(
                                    request_id,
                                    attempt_index,
                                    &cand.route,
                                    &next,
                                    &cls,
                                    session.as_deref(),
                                    reprefill,
                                    switch_cost,
                                );
                                if failover_from.is_none() {
                                    failover_from = Some(cand.route.clone());
                                }
                                facts.failover_from = failover_from.clone();
                                attempt_index += 1;
                                break 'keys;
                            }
                            return StreamOutcome::Failure(ForwardFailure {
                                status: 502,
                                code,
                                message: format!(
                            "upstream connect failed and the fallback chain is exhausted: {message}"
                        ),
                                details: Some(json!({
                                    "upstream_status": Value::Null,
                                    "error_class": cls.class.as_str(),
                                    "stream": true,
                                })),
                            });
                        }
                        OpenHead::UnknownOutcome(message) => {
                            // Full write, no head: `unknown_outcome` (ADR-011
                            // item 6 row 3) — no retry, no failover, deliberately
                            // no closure event.
                            facts.upstream_ms = Some(attempt_started.elapsed().as_millis() as u32);
                            return StreamOutcome::Failure(ForwardFailure {
                                status: 502,
                                code: ErrorCode::UpstreamError,
                                message,
                                details: Some(json!({
                                    "stage": "unknown_outcome",
                                    "error_class": "timeout",
                                    "stream": true,
                                })),
                            });
                        }
                    }
                } // 'keys
            } // the candidate's key-pool arm
        } // while: the chain walk

        // Every candidate was skipped or exhausted. If nothing was
        // attempted, this is the frozen `no_available_route` refusal
        // (ADR-022 / spec §8; ADR-023 condition N): one shape with the
        // buffered walk end, differing only by the pre-existing
        // `"stream": true`.
        //
        // ADR-024 ruling 1: the walk's `failover_from` — set by the
        // pre-attempt cooldown skip — must reach the request's facts
        // before `record_failure_trace` writes the terminal record, so a
        // condition-N refusal whose chain opened on a cooling route
        // carries the abandoned route exactly as the buffered arm does
        // (the served and continue paths already copied it in-loop). The
        // copy covers both refusal returns below: an attempt-bearing
        // walk never reaches the fall-through with an un-copied origin,
        // and a nothing-attempted walk has no other writer.
        facts.failover_from = failover_from.clone();
        if attempted.is_empty() {
            return StreamOutcome::Failure(ForwardFailure {
                status: 502,
                code: ErrorCode::UpstreamError,
                message:
                    "no available route: every candidate provider is demoted, keyless or unavailable"
                        .into(),
                details: Some(json!({
                    "stage": "no_available_route",
                    "skipped": chain_ordered_skipped_json(&walk_skipped),
                    "upstream_status": None::<u16>,
                    "error_class": None::<&'static str>,
                    "stream": true,
                })),
            });
        }
        // The loop-end fall-through (ADR-023 Decision 4): a walk that got
        // here attempted something and served nothing, so the body is
        // condition E's — the class-based sentence with the last
        // attempt's evidence, and **neither** `stage` nor `skipped[]` (the
        // members that would claim no upstream was contacted). This arm
        // is a safety net: with every attempt outcome returning in-loop,
        // no probe has ever reached it, and no case claims a witness for
        // it — but no path may emit condition N's frozen sentence over a
        // request an upstream was contacted for, even if it is reached.
        let deterministic = matches!(
            last_class,
            Some(vadis_core::error_class::ErrorClass::FormatError)
                | Some(vadis_core::error_class::ErrorClass::ContentPolicyBlocked)
        );
        StreamOutcome::Failure(ForwardFailure {
            status: 502,
            code: ErrorCode::UpstreamError,
            message: match last_class {
                Some(class) if deterministic => format!(
                    "upstream rejected the request deterministically ({}); it is not retried",
                    class.as_str()
                ),
                Some(class) => format!(
                    "upstream error ({}) and the fallback chain is exhausted",
                    class.as_str()
                ),
                None => "upstream error and the fallback chain is exhausted".to_string(),
            },
            details: Some(json!({
                "upstream_status": last_upstream_status,
                "error_class": last_class.map(|c| c.as_str()),
                "stream": true,
            })),
        })
    }

    /// The stream path's candidate lookup: the same exclusion and
    /// cooldown rules as the buffered path, but keyed on api_keys (the
    /// stream candidates were filtered at build time) rather than the
    /// buffered transports map.
    fn next_candidate_for(
        &self,
        candidates: &[Candidate],
        attempted: &[String],
        class: vadis_core::error_class::ErrorClass,
    ) -> Option<RouteSpec> {
        if !class.fails_over() {
            return None;
        }
        candidates
            .iter()
            .find(|c| {
                !attempted.contains(&c.route.provider)
                    && !self.provider_in_cooldown(&c.route.provider)
            })
            .map(|c| c.route.clone())
    }
}

/// One walk skip with the chain position it was refused at (ADR-024
/// ruling 2): `skipped[]` is a property of the chain, not of the medium, so
/// the streaming walk's list — seeded with the construction-time skips and
/// grown with the in-walk `demoted` entry — serialises in the chain's own
/// order, exactly the array the buffered walk's single pass produces.
struct ChainSkipped {
    pos: usize,
    skip: (RouteSpec, &'static str),
}

/// `skipped[]` in the chain's own order (spec §8 / ADR-024 ruling 2): the
/// streaming twin of the buffered walk's construction-ordered list — same
/// entries, same reasons, same order, element for element.
fn chain_ordered_skipped_json(skipped: &[ChainSkipped]) -> Vec<Value> {
    let mut ordered: Vec<&ChainSkipped> = skipped.iter().collect();
    ordered.sort_by_key(|s| s.pos);
    ordered
        .iter()
        .map(|s| crate::forward::skipped_entry_json(&s.skip))
        .collect()
}

/// Route resolution (spec §3) — free function so the relay path shares
/// the buffered path's rules without borrowing the engine.
fn resolve_route(
    config: &vadis_core::config::VadisConfig,
    model: &str,
) -> Result<(RouteSpec, &'static str), ForwardFailure> {
    if let Some(route) = config.aliases.get(model) {
        return Ok((route.clone(), "alias"));
    }
    match model.split_once('/') {
        Some((provider, m)) => match config.providers.iter().find(|p| p.name == provider) {
            None => Err(ForwardFailure {
                status: 404,
                code: ErrorCode::UnknownProvider,
                message: format!("unknown provider '{provider}'"),
                details: None,
            }),
            Some(p) if p.models.iter().any(|mm| mm.id == m) => Ok((
                RouteSpec {
                    provider: provider.to_string(),
                    model: m.to_string(),
                },
                "explicit",
            )),
            Some(_) => Err(ForwardFailure {
                status: 404,
                code: ErrorCode::UnknownModel,
                message: format!("unknown model '{m}' on provider '{provider}'"),
                details: None,
            }),
        },
        None => Err(ForwardFailure {
            status: 404,
            code: ErrorCode::UnknownModel,
            message: format!("unknown model or alias '{model}'"),
            details: None,
        }),
    }
}

fn provider_cfg(
    config: &vadis_core::config::VadisConfig,
    route: &RouteSpec,
) -> Option<ProviderCfg> {
    config
        .providers
        .iter()
        .find(|p| p.name == route.provider)
        .cloned()
}

enum OpenHead {
    Head(vadis_providers::stream::StreamHead),
    /// No request bytes went out: the transport kind is the
    /// classification evidence.
    NotSent(vadis_providers::TransportKind, ErrorCode, String),
    /// Full write, no head — `unknown_outcome` (ADR-011 item 6 row 3).
    UnknownOutcome(String),
}

async fn open_head(
    client: &vadis_providers::stream::ReqwestStreamClient,
    cand: &Candidate,
    outbound: &[u8],
) -> OpenHead {
    let plan = vadis_providers::UpstreamPlan {
        provider: &cand.route.provider,
        model: &cand.route.model,
        protocol_out: cand.wire,
        url: &cand.url,
        // The cursor's credential (ADR-049 §3): the pool advances only
        // through the rotation arm — every other path presents the key
        // the candidate's walk is on.
        api_key: cand
            .api_keys
            .get(cand.key_index)
            .map(String::as_str)
            .unwrap_or(""),
        attempt: 0,
    };
    match client.open(&plan, outbound).await {
        Ok(vadis_providers::stream::StreamOpen::Head(h)) => OpenHead::Head(h),
        Ok(vadis_providers::stream::StreamOpen::NotSent(kind, code, message)) => {
            OpenHead::NotSent(kind, code, message)
        }
        Err(message) => OpenHead::UnknownOutcome(message),
    }
}

/// The byte relay (§12.10.3 R1/R2) with the accounting tap and
/// wired in. One item per upstream chunk, verbatim; the stream ends when
/// the upstream ends, and ends without any fabricated terminal marker
/// when it fails after the first relayed byte. `cleaned` is the
/// mutation-(a) base; each attempt's bytes are composed from it with
/// that route's native id (§12.10.7).
fn relay_stream(
    ctx: RelayCtx,
    init: RelayState,
    cleaned: Bytes,
) -> impl Stream<Item = Bytes> + Send + 'static {
    unfold((ctx, init, cleaned), |(ctx, mut st, cleaned)| async move {
        loop {
            match vadis_providers::stream::read_chunk(&mut st.head, ctx.idle).await {
                vadis_providers::stream::StreamRead::Chunk(b) => {
                    if !b.is_empty() {
                        // §12.10.3 R2 write-through: this chunk is the item; the
                        // tap reads a copy (§12.10.3 R7).
                        st.tap.feed(&b);
                        st.relayed = true;
                        // The cache's recording copy (spec §4.17): kept only
                        // within the store's byte bound — a body past it is
                        // never a candidate, so the buffer is dropped rather
                        // than held (fail-closed, ADR-042 §3.4).
                        if let Some(buf) = &mut st.record_buf {
                            if buf.len() as u64 + b.len() as u64
                                <= vadis_core::response_cache::MAX_STORED_BYTES
                            {
                                buf.extend_from_slice(&b);
                            } else {
                                st.record_buf = None;
                            }
                        }
                        return Some((b, (ctx, st, cleaned)));
                    }
                    // An empty keep-alive chunk: nothing to relay,
                    // poll again within this same call.
                }
                vadis_providers::stream::StreamRead::Ended => {
                    record_terminal(&ctx, &st, None);
                    return None;
                }
                vadis_providers::stream::StreamRead::Failed { message, timed_out } => {
                    // R6: after the first relayed byte, failover is
                    // impossible — truncate, record, never retry.
                    if !st.relayed && failover_zero_byte(&ctx, &mut st, &cleaned, &message).await {
                        continue;
                    }
                    let reason = if timed_out {
                        format!("idle bound exceeded: {message}")
                    } else {
                        message
                    };
                    record_classified(&ctx, st.attempted.len() as u32 - 1, &reason);
                    record_terminal(&ctx, &st, Some(reason));
                    return None;
                }
            }
        }
    })
}

/// R6's zero-byte branch: with no byte relayed the client's output is not
/// yet observable, so the fallback chain may be walked (provider
/// exclusion §4.2 + cooldown state). Mutates `st` in place on success;
/// returns false when the chain is exhausted (the caller truncates).
async fn failover_zero_byte(
    ctx: &RelayCtx,
    st: &mut RelayState,
    cleaned: &[u8],
    reason: &str,
) -> bool {
    while let Some(idx) = st.remaining.iter().position(|c| {
        !st.attempted.contains(&c.route.provider) && !in_cooldown(ctx, &c.route.provider)
    }) {
        let cand = st.remaining.remove(idx);
        event(
            ctx,
            EventKind::FailoverTriggered,
            json!({
                "attempt_index": st.attempted.len() as u32 - 1,
                "reason": reason,
                "from": st.route_label,
                "to": format!("{}/{}", cand.route.provider, cand.route.model),
                "stream": true,
                "zero_bytes_relayed": true,
                "reprefill_tokens": Value::Null,
                "switch_cost_nano": Value::Null,
            }),
        );
        let route_label = format!("{}/{}", cand.route.provider, cand.route.model);
        // Mutation (b), per attempt: this candidate's own native id. A
        // rewrite failure falls through to the next candidate — no
        // unrewritten bytes may reach a provider. Composed before the
        // intent so the wire still opens only after the commit (CONF-20).
        let Ok(client) = vadis_providers::stream::ReqwestStreamClient::new(ctx.idle) else {
            return false;
        };
        let outbound = match crate::forward::rewrite_outbound_model(
            &vadis_core::RawBody::new(cleaned.to_vec()),
            &cand.route.model,
        ) {
            Ok(b) => Bytes::from(b.into_owned()),
            Err(_) => continue,
        };
        let attempt_hash = vadis_core::prefix::body_sha16(&outbound);
        // Row 5 — the intent, FULL, before the wire; the payload carries
        // the session's blocks (the ledger rebuild reads them).
        let intent = if let Some(store) = &ctx.store {
            store
                .append(NewEvent {
                    kind: EventKind::UpstreamSubmitted,
                    request_id: Some(&ctx.request_id),
                    session: ctx.session.as_deref(),
                    body_hash: Some(&attempt_hash),
                    trace_ref: None,
                    payload: json!({
                        "route": route_label,
                        "attempt_index": st.attempted.len() as u32,
                        "protocol_out": cand.wire.as_str(),
                        "stream": true,
                        "prefix_blocks": ctx.blocks.iter().map(|b| json!({
                            "index": b.index, "kind": b.kind.as_str(),
                            "tokens": b.tokens, "hash": b.hash,
                        })).collect::<Vec<_>>(),
                    }),
                })
                .ok()
        } else {
            None
        };
        st.attempted.push(cand.route.provider.clone());
        let attempt_started = Instant::now();
        match open_head(&client, &cand, &outbound).await {
            OpenHead::Head(h) if (200..300).contains(&h.status) => {
                if st.failover_from.is_none() {
                    st.failover_from = Some(st_route_spec(&st.route_label));
                }
                st.route_label = route_label;
                st.head = h;
                st.wire = cand.wire;
                st.head_ms = Some(attempt_started.elapsed().as_millis() as u32);
                st.intent = intent;
                // Zero bytes were relayed, so re-arming the tap for the
                // new provider's wire format is lossless (R7 invariant).
                st.tap = SseUsageExtractor::new(cand.wire, ctx.client_requested_usage);
                return true;
            }
            // A failed open on the fallback: keep walking the chain.
            _ => continue,
        }
    }
    false
}

/// `"provider/model"` back to a label match — only used for the
/// `failover_from` report, which names routes, not lookups.
fn st_route_spec(label: &str) -> RouteSpec {
    match label.split_once('/') {
        Some((provider, model)) => RouteSpec {
            provider: provider.to_string(),
            model: model.to_string(),
        },
        None => RouteSpec {
            provider: label.to_string(),
            model: String::new(),
        },
    }
}

/// The pre-relay walk's cooldown skip delegates to `availability`'s
/// single owner (ADR-016 §13.3 L1c) — the buffered walk's skip and the
/// probe gate read the same projection through the same function.
fn in_cooldown(ctx: &RelayCtx, provider: &str) -> bool {
    crate::availability::provider_in_cooldown(ctx.store.as_ref(), provider, now_us())
}

fn record_classified(ctx: &RelayCtx, attempt_index: u32, reason: &str) {
    event(
        ctx,
        EventKind::ErrorClassified,
        json!({
            "attempt_index": attempt_index,
            "status": Value::Null,
            "reason": "stream_truncated",
            "action": "abort",
            "matched": false,
            "detail": reason,
            "stream": true,
        }),
    );
}

/// The terminal `upstream.responded` (R8/R11) **and** the buffered
/// path's closing stages: usage from the tap or
/// `usage_missing: true`; then §12.10.5 note R2's order — cost, the trace line,
/// `cost.computed` + `quota.charged`, the ledger put (§12.10.5 note R3: the
/// accounting rows commit at stream end, before the last byte is
/// written through). A truncation rides in `errors[]`, never presented
/// as completeness; nothing is charged twice.
fn record_terminal(ctx: &RelayCtx, st: &RelayState, truncated: Option<String>) {
    let (usage, usage_missing) = match st.tap.finish() {
        SseUsageOutcome::Complete(u) => (Some(u), false),
        SseUsageOutcome::Missing => (None, true),
    };
    event(
        ctx,
        EventKind::UpstreamResponded,
        json!({
            "status": 200,
            "route": st.route_label,
            "attempt_index": st.attempted.len() as u32 - 1,
            "wrote_full_request": true,
            "stream": true,
            "stream_completed": truncated.is_none(),
            "stream_truncated_reason": truncated,
            "bytes_relayed": st.relayed,
            "usage": usage.map(|u| json!({
                "input_total": u.input_total,
                "input_cached": u.input_cached,
                "cache_write": u.cache_write,
                "output": u.output,
                "reasoning": u.reasoning,
            })),
            "usage_missing": usage_missing,
        }),
    );

    // The ledger (§12.10.5 note R2's order): measure continuity against
    // the PREVIOUS block set, compute cost, write the trace line, commit
    // the accounting rows, then replace the session's blocks for the
    // NEXT request.
    let mut blocks = ctx.blocks.clone();
    if let Some(u) = &usage {
        attribute_tokens(&mut blocks, u);
    }
    let mut errors: Vec<TraceError> = Vec::new();
    if let Some(reason) = &truncated {
        errors.push(TraceError {
            kind: "upstream_error".to_string(),
            message: format!("stream truncated before its terminal event; not retried: {reason}"),
            plugin: None,
            details: Some(json!({
                "error_class": "stream_truncated",
                "stream": true,
            })),
        });
    }
    let (provider, model) = split_route_label(&st.route_label);
    let accountant = Accountant {
        store: ctx.store.as_deref(),
        trace: ctx.trace.as_deref(),
        accounting: ctx.accounting.get(provider),
    };
    let acc_ctx = AccountCtx {
        request_id: &ctx.request_id,
        received_event: ctx.received_event,
        proto_in: ctx.proto_in.as_str(),
        proto_out: Some(st.wire.as_str()),
        session: ctx.session.as_deref(),
        turn_index: ctx.turn_index,
        selection_source: ctx.selection_source,
        requested_model: ctx.requested_model.as_deref(),
        // The credential that served the stream (ADR-049 §3): the
        // relay's own cursor — an index, and only for a real pool.
        key_index: st.multi_key.then_some(st.key_index as u32),
        decision_ms: ctx.decision_ms,
        started: ctx.started,
        now_epoch_s: ctx.now_epoch_s,
        plan_switch: ctx.plan_switch.clone(),
        sticky_hit: ctx.sticky_hit,
        transform_mode: ctx.transform_mode,
        transforms: ctx.transform_records.clone(),
        transform_error: ctx.transform_error.clone(),
    };
    let failover_from = st
        .failover_from
        .as_ref()
        .map(|r| format!("{}/{}", r.provider, r.model));
    let _ = accountant.finish_stream(
        &acc_ctx,
        200,
        provider,
        model,
        usage,
        usage_missing,
        failover_from,
        errors,
        &blocks,
        st.head_ms,
    );
    // Measure, then replace (§12.10.6): the ledger put runs after the
    // continuity measurement inside `finish_stream`.
    if let (Some(id), Some(s)) = (st.intent, ctx.session.as_deref()) {
        accountant.put_ledger(Some(s), &blocks, id);
    }
    // The store's write side, the buffered path's twin (spec §4.17,
    // ADR-042 §3.3): the stream COMPLETED (`truncated.is_none()` — a
    // truncation or an abort never stores), so the buffered copy is a
    // complete 2xx body and a recording candidate. The source reference
    // is THIS request's record — the bytes a later hit replays are
    // attributed to the one measurement, and the session component comes
    // from the key, so the reference can never point across sessions.
    if truncated.is_none() {
        if let (Some((cache, key)), Some(buf)) = (&ctx.cache, &st.record_buf) {
            cache.record(
                key.clone(),
                vadis_core::response_cache::RecordedResponse {
                    status: 200,
                    content_type: ctx.content_type.clone(),
                    body: buf.clone(),
                    source: vadis_core::response_cache::SourceRef {
                        request_id: ctx.request_id.clone(),
                        session: key.session().to_string(),
                        turn_index: ctx.turn_index,
                    },
                },
            );
        }
    }
}

/// `"provider/model"` → its parts; a label without a slash (never
/// produced here) degrades to the whole string as the provider.
fn split_route_label(label: &str) -> (&str, &str) {
    match label.split_once('/') {
        Some((p, m)) => (p, m),
        None => (label, ""),
    }
}

fn event(ctx: &RelayCtx, kind: EventKind, payload: Value) -> Option<EventId> {
    let store = ctx.store.as_ref()?;
    store
        .append(NewEvent {
            kind,
            request_id: Some(&ctx.request_id),
            session: ctx.session.as_deref(),
            body_hash: None,
            trace_ref: None,
            payload,
        })
        .ok()
}
