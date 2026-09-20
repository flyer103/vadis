//! The streaming forwarding engine (R2-2e): byte-faithful SSE
//! passthrough per DESIGN §12.10.3 R1–R11, with the same closing stages
//! as the buffered path (R2G8).
//!
//! The relay is a byte-level operation: the bytes that reach the client
//! are the bytes the upstream sent, in arrival order (R1); each read is
//! written as soon as it is available (R2); the head goes first and is
//! never given an invented `content-length` (R3); an idle gap past
//! `upstream_attempt_timeout` ends the relay (R4); dropping the response
//! future drops the upstream stream, closing the connection (R5); a
//! mid-stream failure **after the first relayed byte** truncates —
//! recorded, never retried, never masked with a fabricated terminal
//! event (R6); the usage tap reads a copy of the relayed bytes off the
//! relay path (R7, R8); usage no carrier delivered is
//! `usage_missing: true`, zero usage, nothing charged (spec §8).
//!
//! R2G8 — the same books (§12.10.5 note R3): the pre-flight runs the
//! buffered path's rules (parse → session resolution → route →
//! capability → outbound bytes → the same event vocabulary), and the
//! relay's end runs the buffered path's closing stages — the
//! `upstream.responded` event (the stream-specific observations
//! `stream_completed` / `bytes_relayed` ride on it), then note R2's
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
//! exactly like the buffered path (R2G5/R2G8): a connection failure is
//! `connect_failure`, never `timeout`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures::stream::{unfold, Stream};
use router_core::config::{ProviderCfg, RouteSpec, WireApi};
use router_core::error::ErrorCode;
use router_core::error_class::{classify_upstream_error, ErrorEvidence, TransportCause};
use router_core::prefix::{attribute_tokens, extract_prefix_blocks, PrefixBlock};
use router_core::store::{EventId, EventKind, NewEvent, Query, QueryRow, Store};
use router_core::trace::TraceError;
use router_protocol::sse::{SseUsageExtractor, SseUsageOutcome};
use serde_json::{json, Value};

use crate::accounting::{AccountCtx, Accountant, RouteAccounting};
use crate::forward::{
    resolve_session_key, rewrite_outbound_model, turn_index_for, ForwardFailure, Forwarder,
    RequestFacts, REASON_PRIMARY_COOLING_DOWN,
};
use router_core::plan::{
    route_in_family, PlanAccount, REASON_PRIMARY_EXHAUSTED, REASON_PRIMARY_RECOVERED,
};
use router_core::trace::PlanSwitchRec;

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
    wire: WireApi,
    base_url: String,
    api_key: String,
}

/// The relay's own context, fully owned (`'static`): what the stream
/// needs to account, classify and fail over after the handler returned.
/// R2G8: it carries the request's accounting facts (the `AccountCtx`
/// inputs) and the route-resolved price/quota tables, so the relay's end
/// can run the buffered path's closing stages without borrowing the
/// engine.
struct RelayCtx {
    store: Option<Arc<dyn Store>>,
    trace: Option<Arc<dyn router_core::TraceWriter>>,
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
    plan_switch: Option<router_core::trace::PlanSwitchRec>,
    /// spec §6 `state.sticky_hit`: the session already had a binding row
    /// at pre-flight (the same value bind_session received).
    sticky_hit: bool,
}

/// The relay's mutable state, threaded through `unfold`.
struct RelayState {
    /// The unread upstream response being drained.
    head: router_providers::stream::StreamHead,
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
    /// connection and closes it when dropped (R5). Every terminal
    /// failure writes its `DecisionRecord` before the outcome leaves the
    /// engine (R2G4/R2G8) — the same `finish_failure` seam as the
    /// buffered path.
    pub async fn forward_stream(
        &self,
        proto_in: WireApi,
        body: &[u8],
        request_id: &str,
        headers: &[(String, String)],
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
        // Session resolution — the shared helper (R2G8), moved ahead of
        // the plan guard (which reads `turn_index` from the sticky
        // projection, ADR-014 item 3).
        let session = resolve_session_key(&self.config, &parsed, headers);
        let turn_index = turn_index_for(&self.store, session.as_deref());
        facts.session = session.clone();
        facts.turn_index = turn_index;
        // The plan policy's Guard stage (spec §4.6) — the same rule the
        // buffered path runs, before any attempt.
        let mut plan_guard_out: Option<crate::forward::PlanGuardOutcome> = None;
        match self.plan_guard(
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
                    // (spec §6's producer table; the buffered path's
                    // twin): a move to the family's overflow route is an
                    // exhaustion displacement, a move to the primary is
                    // a recovery — never the pre-request account state.
                    let to_overflow = self
                        .config
                        .plan_policy
                        .as_ref()
                        .is_some_and(|p| p.overflow == g.route);
                    facts.plan_switch = Some(PlanSwitchRec {
                        from: primary.to_string(),
                        to: g.route.to_string(),
                        reason: if to_overflow {
                            REASON_PRIMARY_EXHAUSTED
                        } else {
                            REASON_PRIMARY_RECOVERED
                        },
                        probe: g.probe,
                        reprefill_tokens: None,
                        switch_cost_nano: None,
                    });
                } else if g.probe {
                    // The admitted probe's return trip (spec §6): the
                    // same record the buffered path writes — the
                    // displacement is from the STATE's route (overflow),
                    // the way back costs 0 (in-plan destination).
                    if let Some(policy) = self.config.plan_policy.as_ref() {
                        facts.plan_switch = Some(PlanSwitchRec {
                            from: policy.overflow.to_string(),
                            to: policy.primary.to_string(),
                            reason: REASON_PRIMARY_RECOVERED,
                            probe: true,
                            reprefill_tokens: None,
                            switch_cost_nano: Some(0),
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
        // router-owned top-level keys — mutation (a), once per request. The
        // `model` rewrite (mutation (b)) is per attempt: the primary below,
        // each failover candidate inside the relay.
        let cleaned = match router_core::RawBody::new(body.to_vec())
            .remove_top_level_keys(router_core::ROUTER_OWNED_TOP_LEVEL_KEYS)
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
        let outbound_hash = router_core::prefix::body_sha16(cleaned.as_bytes());
        facts.blocks = extract_prefix_blocks(&cleaned).unwrap_or_default();

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
        // empty (parity with the buffered path's event vocabulary).
        self.append_event(
            EventKind::TransformApplied,
            request_id,
            None,
            json!({
                "plugin": null,
                "chain": [],
                "changed": false,
            }),
            session.as_deref(),
        );
        // Row 4 — session.bound through the accountant's one writer, the
        // same call the buffered path makes (R2G8).
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
                    decision_ms,
                    started,
                    now_epoch_s,
                    plan_switch: facts.plan_switch.clone(),
                    sticky_hit: crate::forward::session_sticky_hit(&self.store, session.as_deref()),
                },
                &primary.provider,
                &primary.model,
                crate::forward::session_sticky_hit(&self.store, session.as_deref()),
                false,
                self.session_ttl_us,
            );
        }

        // The primary candidate plus the fallback chain (spec §4.2),
        // filtered to native routes with a key present at startup. For a
        // request inside a plan family the family's `overflow` route is
        // the first candidate after primary (spec §4.2 refined by §4.6).
        let mut candidates: Vec<Candidate> = Vec::new();
        {
            let mut routes: Vec<RouteSpec> = vec![primary.clone()];
            if let Some(policy) = &self.config.plan_policy {
                if route_in_family(policy, &primary) && !routes.contains(&policy.overflow) {
                    routes.insert(1, policy.overflow.clone());
                }
            }
            for route in routes
                .into_iter()
                .chain(self.config.fallback.iter().cloned())
            {
                if candidates.iter().any(|c| c.route == route) {
                    continue;
                }
                if let Some(cfg) = self
                    .config
                    .providers
                    .iter()
                    .find(|p| p.name == route.provider)
                {
                    if cfg.wire_api == proto_in && self.api_keys.contains_key(&cfg.name) {
                        candidates.push(Candidate {
                            route: route.clone(),
                            wire: cfg.wire_api,
                            base_url: cfg.base_url.clone(),
                            api_key: self.api_keys[&cfg.name].clone(),
                        });
                    }
                }
            }
        }
        if candidates.is_empty() {
            return StreamOutcome::Failure(ForwardFailure {
                status: 502,
                code: ErrorCode::UpstreamError,
                message: "no available route: the primary provider is keyless or unavailable"
                    .into(),
                details: Some(json!({"stream": true})),
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
        let client = match router_providers::stream::ReqwestStreamClient::new(idle) {
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
        // has left the router until a 2xx head is relayed.
        let mut attempted: Vec<String> = Vec::new();
        let mut attempt_index: u32 = 0;
        let mut failover_from: Option<RouteSpec> = None;
        let mut last_upstream_status: Option<u16> = None;
        let mut last_class: Option<router_core::error_class::ErrorClass> = None;

        // The family's primary refused by ADR-011's cooldown projection
        // before any attempt (spec §6's producer table, CONF-42): set by
        // the skip below, consumed by the first candidate actually
        // attempted.
        let mut cooling_abandoned: Option<RouteSpec> = None;

        for (ci, cand) in candidates.iter().enumerate() {
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
                if let Some(policy) = &self.config.plan_policy {
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
                    });
                }
            }

            // Mutation (b), per attempt (§12.10.7): this route's native id.
            let outbound = match rewrite_outbound_model(&cleaned, &cand.route.model) {
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
            let attempt_hash = router_core::prefix::body_sha16(&outbound);

            // Row 5 — the intent, FULL, committed before the wire
            // (CONF-20). The payload carries the session's prefix blocks
            // (the ledger rebuild reads them, §12.10.6); `body_hash` is
            // this attempt's byte-final bytes (note R4).
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
            match open_head(&client, cand, &outbound).await {
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
                            if let Some(policy) = self.config.plan_policy.as_ref() {
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
                                    });
                                }
                                self.plan_probe_succeeded(request_id, policy, session.as_deref());
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
                            sticky_hit: crate::forward::session_sticky_hit(
                                &self.store,
                                session.as_deref(),
                            ),
                        };
                        let state = RelayState {
                            tap: SseUsageExtractor::new(cand.wire, client_requested_usage),
                            head,
                            relayed: false,
                            remaining: candidates[ci + 1..].to_vec(),
                            attempted,
                            route_label,
                            wire: cand.wire,
                            head_ms,
                            intent: intent_id,
                            failover_from: failover_from.clone(),
                        };
                        facts.failover_from = failover_from.clone();
                        let relay =
                            relay_stream(ctx, state, Bytes::from(cleaned.as_bytes().to_vec()));
                        return StreamOutcome::Success(StreamSuccess {
                            status: 200,
                            content_type,
                            route: cand.route.clone(),
                            failover_from,
                            body: Box::pin(relay),
                        });
                    }
                    // A failure status head before any relayed byte is the
                    // ordinary error path (R6 column 1): classify with the
                    // same evidence inputs as the buffered path, record,
                    // act — the §8 body answers.
                    let status = head.status;
                    let resp = head.as_upstream_response();
                    let evidence = ErrorEvidence {
                        status: Some(status),
                        retry_after: resp.retry_after.as_deref(),
                        body: b"",
                        wrote_full_request: true,
                        transport_cause: None,
                    };
                    let cls = classify_upstream_error(&evidence);
                    last_class = Some(cls.class);
                    let classified_id =
                        self.record_classification(request_id, attempt_index, Some(status), &cls);
                    self.apply_demotion(&cand.route.provider, &cls, classified_id);
                    // The plan policy's only account-moving signal
                    // (ADR-014 item 2): 403 `quota_exhausted` on the
                    // family's primary flips the family to overflow and
                    // records `plan.switched` before the next intent.
                    if cls.class.demotes_provider() {
                        if let Some(policy) = self.config.plan_policy.as_ref() {
                            if cand.route == policy.primary {
                                self.record_plan_switch(
                                    request_id,
                                    policy,
                                    &PlanAccount::Primary,
                                    &PlanAccount::Overflow,
                                    REASON_PRIMARY_EXHAUSTED,
                                    false,
                                    session.as_deref(),
                                );
                                if policy.on_primary_exhausted
                                    == router_core::config::OnPrimaryExhausted::Block
                                {
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
                                if facts.plan_switch.is_none() {
                                    let (reprefill, cost_nano) =
                                        self.failover_cost(session.as_deref(), &policy.overflow);
                                    facts.plan_switch = Some(PlanSwitchRec {
                                        from: policy.primary.to_string(),
                                        to: policy.overflow.to_string(),
                                        reason: REASON_PRIMARY_EXHAUSTED,
                                        probe: false,
                                        reprefill_tokens: reprefill,
                                        switch_cost_nano: cost_nano,
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
                    if let Some(next) = self.next_candidate_for(&candidates, &attempted, cls.class)
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
                        continue;
                    }
                    let deterministic = matches!(
                        cls.class,
                        router_core::error_class::ErrorClass::FormatError
                            | router_core::error_class::ErrorClass::ContentPolicyBlocked
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
                    // `connect_failure` on both paths (R2G5/R2G8). No
                    // upstream.responded row: the attempt never reached
                    // the upstream.
                    let evidence = ErrorEvidence {
                        status: None,
                        retry_after: None,
                        body: b"",
                        wrote_full_request: false,
                        transport_cause: Some(match kind {
                            router_providers::TransportKind::Connect => TransportCause::Connect,
                            router_providers::TransportKind::Timeout => TransportCause::Timeout,
                            router_providers::TransportKind::Other => TransportCause::Other,
                        }),
                    };
                    let cls = classify_upstream_error(&evidence);
                    last_class = Some(cls.class);
                    let classified_id =
                        self.record_classification(request_id, attempt_index, None, &cls);
                    self.apply_demotion(&cand.route.provider, &cls, classified_id);
                    if let Some(next) = self.next_candidate_for(&candidates, &attempted, cls.class)
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
                        continue;
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
        }

        // Every candidate was skipped or exhausted.
        StreamOutcome::Failure(ForwardFailure {
            status: 502,
            code: ErrorCode::UpstreamError,
            message:
                "no available route: every candidate provider is demoted, keyless or unavailable"
                    .into(),
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
        class: router_core::error_class::ErrorClass,
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

/// Route resolution (spec §3) — free function so the relay path shares
/// the buffered path's rules without borrowing the engine.
fn resolve_route(
    config: &router_core::config::RouterConfig,
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
    config: &router_core::config::RouterConfig,
    route: &RouteSpec,
) -> Option<ProviderCfg> {
    config
        .providers
        .iter()
        .find(|p| p.name == route.provider)
        .cloned()
}

enum OpenHead {
    Head(router_providers::stream::StreamHead),
    /// No request bytes went out: the transport kind is the
    /// classification evidence (R2G5/R2G8).
    NotSent(router_providers::TransportKind, ErrorCode, String),
    /// Full write, no head — `unknown_outcome` (ADR-011 item 6 row 3).
    UnknownOutcome(String),
}

async fn open_head(
    client: &router_providers::stream::ReqwestStreamClient,
    cand: &Candidate,
    outbound: &[u8],
) -> OpenHead {
    let plan = router_providers::UpstreamPlan {
        provider: &cand.route.provider,
        model: &cand.route.model,
        protocol_out: cand.wire,
        base_url: &cand.base_url,
        api_key: &cand.api_key,
        attempt: 0,
    };
    match client.open(&plan, outbound).await {
        Ok(router_providers::stream::StreamOpen::Head(h)) => OpenHead::Head(h),
        Ok(router_providers::stream::StreamOpen::NotSent(kind, code, message)) => {
            OpenHead::NotSent(kind, code, message)
        }
        Err(message) => OpenHead::UnknownOutcome(message),
    }
}

/// The byte relay (R1/R2) with the accounting tap and R6's truncation
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
            match router_providers::stream::read_chunk(&mut st.head, ctx.idle).await {
                router_providers::stream::StreamRead::Chunk(b) => {
                    if !b.is_empty() {
                        // R2 write-through: this chunk is the item; the
                        // tap reads a copy (R7).
                        st.tap.feed(&b);
                        st.relayed = true;
                        return Some((b, (ctx, st, cleaned)));
                    }
                    // An empty keep-alive chunk: nothing to relay,
                    // poll again within this same call.
                }
                router_providers::stream::StreamRead::Ended => {
                    record_terminal(&ctx, &st, None);
                    return None;
                }
                router_providers::stream::StreamRead::Failed { message, timed_out } => {
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
        let Ok(client) = router_providers::stream::ReqwestStreamClient::new(ctx.idle) else {
            return false;
        };
        let outbound = match crate::forward::rewrite_outbound_model(
            &router_core::RawBody::new(cleaned.to_vec()),
            &cand.route.model,
        ) {
            Ok(b) => Bytes::from(b.into_owned()),
            Err(_) => continue,
        };
        let attempt_hash = router_core::prefix::body_sha16(&outbound);
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

fn in_cooldown(ctx: &RelayCtx, provider: &str) -> bool {
    let Some(store) = &ctx.store else {
        return false;
    };
    matches!(
        store.query(Query::Cooldown {
            provider,
            model: None,
        }),
        Ok(QueryRow::Cooldown(Some(row))) if row.until_us > now_us()
    )
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
/// path's closing stages (R2G8): usage from the tap or
/// `usage_missing: true`; then note R2's order — cost, the trace line,
/// `cost.computed` + `quota.charged`, the ledger put (§12.10.5 R3: the
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
        decision_ms: ctx.decision_ms,
        started: ctx.started,
        now_epoch_s: ctx.now_epoch_s,
        plan_switch: ctx.plan_switch.clone(),
        sticky_hit: ctx.sticky_hit,
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
