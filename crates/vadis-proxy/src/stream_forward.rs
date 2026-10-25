//! The streaming forwarding engine: byte-faithful SSE
//! passthrough per DESIGN §12.10.3 R1–R11, with the same closing stages
//! as the buffered path.
//!
//! The relay is a byte-level operation: the bytes that reach the client
//! are the bytes the upstream sent, in arrival order (§12.10.3 R1); each
//! read is written as soon as it is available (§12.10.3 R2); the head goes
//! first and is never given an invented `content-length` (§12.10.3 R3); an
//! idle gap past `server.stream_idle_timeout` — the gap bound's own key,
//! spec §4.20 — ends the relay (§12.10.3 R4);
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
    metered_candidates_field, resolve_session_key, rewrite_outbound_model, turn_index_for,
    ForwardFailure, Forwarder, GuardReentry, RequestFacts, REASON_PRIMARY_COOLING_DOWN,
};
use vadis_core::plan::{
    account_of_route, displacement_reason, metered_walk, plan_tier, route_in_family,
    REASON_PRIMARY_RECOVERED,
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
    /// The body's gap bound (spec §4.20.1): `server.stream_idle_timeout`,
    /// read ONCE at the streaming driver's entry — the value this request
    /// drives with (a reload reaches the next request, never this one,
    /// CONF-108 limb (d)). Each relay read's window is
    /// `min(this, time-until-deadline)`, composed below.
    idle: Duration,
    /// ADR-053 §3's single-owner `server.request_timeout` deadline, an
    /// ABSOLUTE instant measured from the moment the request entered
    /// the streaming driver. The relay's own reads trip against it
    /// (`tokio::time::timeout_at`) into R6 column 2's declared
    /// truncation; it is never re-armed per chunk, and the head phase
    /// (outside this struct) enforces the same instant, not a second
    /// timer.
    request_deadline: tokio::time::Instant,
    request_id: String,
    proto_in: WireApi,
    session: Option<String>,
    turn_index: u32,
    requested_model: Option<String>,
    selection_source: &'static str,
    decision_ms: u32,
    started: Instant,
    now_epoch_s: u64,
    /// The same clock read as `now_epoch_s`, untruncated (µs) — the word
    /// the terminal record's `ts` renders (ADR-057 §2.6), carried from
    /// the pre-flight's ONE read (never re-read in the relay).
    now_us: i64,
    received_event: Option<EventId>,
    /// Prefix blocks of the cleaned body (hashes only until usage
    /// attribution runs at stream end).
    blocks: Vec<PrefixBlock>,
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
    /// ADR-060 D1: the cancelled-row guard's slot, shared with the
    /// guard that moved into the response body — the relay's byte
    /// counter and the terminal take both go through it, so a drop at
    /// any chunk boundary reads the facts the relay last recorded.
    cancel: Arc<crate::cancel::CancelSlot>,
}

/// The relay's mutable state, threaded through `unfold`.
struct RelayState {
    /// The unread upstream response being drained.
    head: vadis_providers::stream::StreamHead,
    /// The accounting tap over a copy of the relayed bytes (R7).
    tap: SseUsageExtractor,
    /// How many body bytes this request has written through to the
    /// client — R6's boundary is `== 0`, and spec §4.20.3's
    /// `relayed_bytes` observable is the count itself.
    relayed: u64,
    /// ADR-060 D3: how many upstream attempts this request made across
    /// all routes (`result.attempts`) — the pre-relay walk's count,
    /// carried in because the relay owns the terminal record.
    attempt_count: u32,
    /// The route currently answering (for events).
    route_label: String,
    /// The wire the answering attempt went out on (the trace's
    /// `protocol_out`): the inbound protocol, by construction (ADR-051
    /// §2.1 — a candidate is attempted only if it declares the inbound
    /// cell).
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
    /// The ANSWERING head's content-type (R74-0-F7's repair): set where a
    /// head is adopted — the pre-relay 2xx arm and the zero-byte
    /// failover's answering arm — so the media gate (spec §4.20.3) and
    /// the cache record read the head that answered, never the first
    /// head's label after a failover moved away from it.
    content_type: Option<String>,
    /// When the last body byte arrived from the upstream — spec §4.20.3's
    /// `since_last_byte_ms` is measured from here (from `head_arrived`
    /// while no byte has ever arrived).
    last_byte_at: Option<Instant>,
    /// When the answering head arrived.
    head_arrived: Instant,
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
            upstream_body: None,
            attempts: 0,
        };
        // ADR-053 §3 — `server.request_timeout` becomes real on the
        // stream path, SINGLE OWNER: one deadline measured from the
        // moment the request enters this driver (facts.started), armed
        // here, before the head phase, and carried into the relay's
        // context as the same ABSOLUTE instant. The head wrap below
        // enforces this instant (it is not a second timer); the relay's
        // per-read `timeout_at` trips against it without re-arming.
        let deadline = tokio::time::Instant::from_std(facts.started) + self.request_timeout();
        // ADR-060 D1: the cancelled-row guard, owned by THIS frame for
        // the head phase — hyper dropping the handler future mid-head
        // drops it — and moved into the response body on success (the
        // body owns the relay stream, so dropping the body drops the
        // guard). The slot stays empty until the request commits to
        // being served (filled in `forward_stream_inner`, after the
        // boundary refusals and the replay, before the first attempt).
        let guard = crate::cancel::CancelGuard::new(
            self.store.clone(),
            self.trace
                .clone()
                .map(|t| t as std::sync::Arc<dyn vadis_core::TraceWriter>),
        );
        // The relay's context needs the slot as an `Arc` (it outlives the
        // handler future), so the inner driver takes the Arc form.
        let cancel_arc = guard.slot_arc();
        let outcome = match tokio::time::timeout_at(
            deadline,
            self.forward_stream_inner(
                &mut facts,
                proto_in,
                body,
                request_id,
                headers,
                deadline,
                &cancel_arc,
            ),
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(_elapsed) => {
                // The whole-request bound tripped BEFORE our head was
                // sent (any inner outcome — including a relay-priming
                // success — would have returned by now): R6 column 1's
                // ordinary refusal, conservatively §2.2's
                // `unknown_outcome` shape (the request may have been
                // fully written upstream).
                StreamOutcome::Failure(ForwardFailure {
                    status: 502,
                    code: ErrorCode::UpstreamError,
                    message: "no response head within server.request_timeout \
                         (unknown_outcome): the whole-request bound expired \
                         before any byte was relayed"
                        .to_string(),
                    details: Some(json!({
                        "stage": "unknown_outcome",
                        "error_class": "timeout",
                        "stream": true,
                        // ADR-060 D3, spec §4.2's table row 5: the outer
                        // bound tripped — the outcome is unknown AND no
                        // budget remains for a retry.
                        "retry_refused": "budget",
                    })),
                })
            }
        };
        if let StreamOutcome::Failure(f) = &outcome {
            self.record_failure_trace(&facts, f, Some(guard.slot()));
        }
        match outcome {
            // ADR-060 D1: the SSE medium's arm — the guard moves into
            // the response body, so hyper dropping the body on a client
            // disconnect (or the shutdown drain) is what runs its
            // `Drop` and, when the ordinary terminal writer had not
            // taken the slot, writes the `client_closed` row.
            StreamOutcome::Success(mut s) => {
                s.body = crate::cancel::guard_body(guard, s.body);
                StreamOutcome::Success(s)
            }
            failure => failure,
        }
    }

    /// ADR-053 §3: the stream path's whole-request bound. One owner —
    /// this driver — reading the one configured value.
    fn request_timeout(&self) -> Duration {
        Duration::from_millis(self.config.server.request_timeout.0)
    }

    /// Pre-flight + relay setup: the same parse → session → route →
    /// capability → outbound bytes rules as the buffered path (R11), the
    /// same event vocabulary (rows 1–5 of §12.10.5), and a pre-relay
    /// candidate walk that classifies with the same evidence inputs.
    /// `deadline` is ADR-053 §3's whole-request bound — the SAME
    /// absolute instant the outer wrap enforces, carried through so the
    /// relay the priming hands off to trips against it after our head
    /// is sent.
    #[allow(clippy::too_many_arguments)]
    async fn forward_stream_inner(
        &self,
        facts: &mut RequestFacts<'_>,
        proto_in: WireApi,
        body: &[u8],
        request_id: &str,
        headers: &[(String, String)],
        deadline: tokio::time::Instant,
        cancel: &std::sync::Arc<crate::cancel::CancelSlot>,
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
        // The pin input (ADR-049 §6 rule 1), the buffered path's twin:
        // the binding's route from the ONE sticky read above. The rule
        // filters (a metered route of this revision's walk is a pin; a
        // stale or off-roster binding re-ranks like a new session).
        let session_pin = prior_binding.as_ref().map(|b| RouteSpec {
            provider: b.provider.clone(),
            model: b.model.clone(),
        });
        let guard_ctx = crate::forward::GuardCtx {
            session: session.as_deref(),
            turn_index,
            now_epoch_s,
            now_us: facts.now_us,
            session_pin: session_pin.as_ref(),
        };
        match self.plan_guard(plan_policy.as_ref(), &primary, &guard_ctx) {
            Ok(None) => {}
            Ok(Some(g)) => {
                if g.route != primary && !g.pinned {
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
                        let metered = metered_walk(&self.config, policy);
                        facts.plan_switch = Some(PlanSwitchRec {
                            from: primary.to_string(),
                            to: g.route.to_string(),
                            reason: displacement_reason(
                                policy, &tier, &metered, &primary, &g.route,
                            ),
                            probe: g.probe,
                            reprefill_tokens: None,
                            switch_cost_nano: None,
                            // §4.8: the destination route's unit (the buffered
                            // path's twin writes the same value).
                            cost_currency: self.route_currency(&g.route),
                            candidates: metered_candidates_field(&self.config, policy),
                            chosen: Some(g.route.to_string()),
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
                            candidates: metered_candidates_field(&self.config, policy),
                            chosen: Some(policy.primary.to_string()),
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
        // ADR-051 §2.1: a declared cell is a native cell — the request is
        // served on `urls[inbound]` below, so the resolved route has no
        // `501` branch left to take.

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
        // ADR-051 §2.1: `protocol_out` is the inbound protocol by
        // construction — the wire the bytes leave on is the cell the
        // request came in on. `wire_api` names the diagonal and gates
        // nothing.
        facts.proto_out = Some(proto_in);
        self.append_event(
            EventKind::DecisionMade,
            request_id,
            None,
            json!({
                "provider": primary.provider,
                "model": primary.model,
                "requested_model": model,
                "selection_source": selection_source,
                "protocol_out": proto_in.as_str(),
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
                    proto_out: Some(proto_in.as_str()),
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
                    now_us: facts.now_us,
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
                    now_us: facts.now_us,
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

        // ADR-060 D1: the request now commits to being served — the
        // cancelled-row guard's snapshot is installed (the boundary
        // refusals and the replay above wrote their own row and hold no
        // in-flight attempt); the relay's context carries the slot so
        // the byte counter stays shared with the guard.
        cancel.refresh(crate::cancel::snapshot_facts(
            facts,
            &primary.provider,
            &primary.model,
        ));

        // The primary candidate plus the fallback chain (spec §4.2),
        // filtered to native routes with a key present at startup. For a
        // request inside a plan family the family's `overflow` route is
        // the first candidate after primary (spec §4.2 refined by §4.6).
        // Entries the filter refuses are classified (ADR-051 §2.1 / DESIGN
        // §12.10.9) so the walk-end refusal can say why — the buffered
        // walk answers "which candidates may serve" with the same rule:
        // provider entry, key, and the inbound protocol being a declared
        // cell (`supports ∋ proto_in`).
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
                let metered = metered_walk(&self.config, policy);
                if route_in_family(&tier, &metered, &primary) {
                    if let Some(active_pos) = tier.iter().position(|r| r == &primary) {
                        for member in tier.iter().skip(active_pos + 1) {
                            if !routes.contains(member) {
                                routes.push(member.clone());
                            }
                        }
                    }
                    // §5.7 (the buffered path's twin): the metered tier
                    // in walk order — the policy's `overflow` alone
                    // under `declared`, the §5.6 ranking under
                    // `cheapest`.
                    for member in &metered {
                        if !routes.contains(member) {
                            routes.push(member.clone());
                        }
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
                if !cfg.supports.contains(&proto_in) {
                    // The declared-cell gate (ADR-051 §2.1 / DESIGN
                    // §12.10.9): the entry does not declare the inbound
                    // protocol, so the cell does not exist for this
                    // request. The skip is in the keyless class — never
                    // attempted, never narrated as a displacement; the
                    // refusal reports it at the walk's end.
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
                // ADR-051 §2.1: the URL is the **inbound** protocol's cell
                // — never the entry's `wire_api`. `validate` refused any
                // config whose declared cell has no URL, so a miss here is
                // an internal error to answer — not a case to paper over
                // by sending the request to a guessed endpoint.
                let Some(url) = cfg.url_for(proto_in) else {
                    return StreamOutcome::Failure(ForwardFailure {
                        status: 500,
                        code: ErrorCode::Internal,
                        message: format!(
                            "provider '{}' declares no URL for the inbound protocol '{}'",
                            cfg.name,
                            proto_in.as_str()
                        ),
                        details: Some(json!({"stream": true})),
                    });
                };
                candidates.push(Candidate {
                    route: route.clone(),
                    chain_pos: pos,
                    // The wire this attempt goes out on: the inbound
                    // protocol (ADR-051 §2.1) — a declared cell is a
                    // native cell, whatever `wire_api` names.
                    wire: proto_in,
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

        // ADR-056 §2.1/§6.1.1: the TWO windows, read once at this
        // driver's entry — `head_timeout` (connect + head arrival, on
        // the attempt knob) and `idle` (the body's gap bound, on its own
        // key). Every later consumer — the relay's per-read composition,
        // the zero-byte failover's mid-request client — takes the pair
        // from the request's own frozen context, never from the live
        // config, so a reload reaches the next request only
        // (CONF-108 limb (d)).
        let head_timeout = Duration::from_millis(self.config.server.upstream_attempt_timeout.0);
        let idle = Duration::from_millis(self.config.server.stream_idle_timeout.0);
        let client = match vadis_providers::stream::ReqwestStreamClient::new(head_timeout, idle) {
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
                // ADR-051 §2.4, the buffered path's twin: the guard
                // re-enters at EVERY commitment point — each candidate
                // the walk reaches (a `fallback` entry, a family tier
                // member, a mid-walk landing point) is routed by its
                // family's state through the same two owners, and the
                // walk's own conditions layer on top of the answer. A
                // Redirect swaps the offered candidate for the family's
                // answer; an answer the cooldown projection refuses is
                // skipped `demoted` (CONF-42's shape) and the offered
                // candidate stands. The head's commitment point is the
                // guard's own evaluation above (same route, same state
                // read — the answer is idempotent), so its re-entry
                // pass is the same no-op every confirmed candidate
                // costs: one projection read.
                let mut redirected: Option<Candidate> = None;
                match self.family_guard_reentry(&cand.route, &guard_ctx, &attempted, proto_in) {
                    GuardReentry::Stand => {}
                    GuardReentry::Cooling { route: answer } => {
                        walk_skipped.push(ChainSkipped {
                            pos: cand.chain_pos,
                            skip: (answer, crate::forward::SKIP_DEMOTED),
                        });
                    }
                    GuardReentry::Redirect { policy, outcome } => {
                        let offered = cand.route.clone();
                        let tier = plan_tier(&self.config, &policy);
                        let metered = metered_walk(&self.config, &policy);
                        if facts.plan_switch.is_none() {
                            let (reprefill, cost_nano) =
                                self.failover_cost(session.as_deref(), &outcome.route);
                            facts.plan_switch = Some(PlanSwitchRec {
                                from: offered.to_string(),
                                to: outcome.route.to_string(),
                                // The direction word the existing
                                // single owner picks for THIS move —
                                // the same owner, the same spelling.
                                reason: displacement_reason(
                                    &policy,
                                    &tier,
                                    &metered,
                                    &offered,
                                    &outcome.route,
                                ),
                                probe: outcome.probe,
                                reprefill_tokens: reprefill,
                                switch_cost_nano: cost_nano,
                                cost_currency: self.route_currency(&outcome.route),
                                candidates: metered_candidates_field(&self.config, &policy),
                                chosen: Some(outcome.route.to_string()),
                            });
                        }
                        // §2.4 boundary 4: the settled route is the
                        // single truth for the attempt; the probe flag
                        // stays current for the answering arm below.
                        plan_guard_out = Some(outcome.clone());
                        // The substitute must clear the construction
                        // conditions the offered chain cleared (the
                        // re-entry helper already read provider entry
                        // and the declared inbound cell). A provider
                        // with no URL or no key cannot serve: it
                        // narrates the keyless skip and the offered
                        // candidate stands.
                        let sub = (|| {
                            let cfg = self
                                .config
                                .providers
                                .iter()
                                .find(|p| p.name == outcome.route.provider)?;
                            let url = cfg.url_for(proto_in)?;
                            let keys = self.api_keys.get(&cfg.name)?;
                            Some(Candidate {
                                route: outcome.route.clone(),
                                chain_pos: cand.chain_pos,
                                // ADR-051 §2.1: the wire is the
                                // inbound protocol — a declared cell is
                                // a native cell.
                                wire: proto_in,
                                url: url.to_string(),
                                api_keys: keys.clone(),
                                key_index: 0,
                            })
                        })();
                        match sub {
                            Some(sub) => {
                                if !accounting.contains_key(&sub.route.provider) {
                                    if let Some(a) = crate::accounting::route_accounting(
                                        &self.config,
                                        &sub.route,
                                    ) {
                                        accounting.insert(sub.route.provider.clone(), a);
                                    }
                                }
                                redirected = Some(sub);
                            }
                            None => {
                                walk_skipped.push(ChainSkipped {
                                    pos: cand.chain_pos,
                                    skip: (outcome.route.clone(), crate::forward::SKIP_KEYLESS),
                                });
                            }
                        }
                    }
                }
                if let Some(sub) = redirected {
                    cand = sub;
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
                // ADR-056 §2.5 (spec §4.20.5): the advisory silence skip,
                // the streaming twin of the buffered walk's. A route whose
                // own `stream_silence` row is live is preferred against,
                // never refused — the walk moves past it only when the
                // chain holds another candidate it would actually attempt,
                // and attempts it as usual otherwise. The skip narrates
                // `failover_from` (ADR-024's pre-attempt shape) and writes
                // NO `skipped[]` entry and no `failover.triggered` row: an
                // advisory row must never read binding (§2.5.3).
                if self.route_silence_cooling(&cand.route)
                    && candidates.iter().any(|c| {
                        !attempted.contains(&c.route.provider)
                            && !self.provider_in_cooldown(&c.route.provider)
                            && !self.route_silence_cooling(&c.route)
                    })
                {
                    if failover_from.is_none() {
                        failover_from = Some(cand.route.clone());
                    }
                    continue;
                }
                attempted.push(cand.route.provider.clone());
                let route_label = format!("{}/{}", cand.route.provider, cand.route.model);
                facts.attempted_route = Some(cand.route.clone());
                // ADR-049 §5.6, the streaming twin of the buffered walk's
                // settle: `chosen` is the candidate the walk settles on.
                // The pre-walk writer (the guard's move) can only name the
                // ranking's head; this loop knows the route each attempt
                // opens on, so the label is captured per attempt and the
                // settle happens at the 2xx head / the zero-byte failover's
                // answering head — before the ctx (and the trace line it
                // writes) is built. The event row predates the walk and is
                // untouched.
                let settled_label = route_label.clone();

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
                            candidates: plan_policy
                                .as_ref()
                                .and_then(|p| metered_candidates_field(&self.config, p)),
                            chosen: Some(cand.route.to_string()),
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
                // ADR-060 D3: this attempt now exists — the terminal
                // record's `result.attempts` and the cancelled-row
                // guard's snapshot read this count.
                facts.attempts = attempt_index + 1;
                // ADR-060 D2 (R4): the refusal slice is per attempt —
                // a walk that moved past an error head must not let
                // that attempt's `upstream_body` ride the terminal
                // record of an attempt that answered no head of its
                // own. Cleared here, written only by the answered arm
                // below. `last_upstream_status` is cleared with it:
                // the terminal record's `upstream_status` names the
                // answering head of the attempt that failed, and a
                // previous attempt's 429 is not that.
                facts.upstream_body = None;
                facts.last_upstream_status = None;
                last_upstream_status = None;
                // ADR-060 D1: refresh the cancelled-row guard to this
                // attempt's facts — a drop mid-attempt (head phase)
                // strands exactly this state.
                cancel.refresh(crate::cancel::snapshot_facts(
                    facts,
                    &cand.route.provider,
                    &cand.route.model,
                ));
                cancel.note_attempt(facts.attempts);
                // The rotation arm (ADR-049 §3 rule 3), the streaming twin:
                // a credential-class failure whose pool still holds an
                // untried key retries the SAME route on the next key —
                // `rotate_credential` with the index, no demotion,
                // `failover_from` untouched. Only an exhausted pool leaves
                // the provider. `quota_exhausted` never rotates (rule 4).
                // ADR-060 D3: this route's same-route retries so far —
                // one per route per request, reset by the walk's move.
                let mut retries_used: u32 = 0;
                'keys: loop {
                    match open_head(&client, &cand, &outbound).await {
                        OpenHead::Head(head) => {
                            let head_ms = Some(attempt_started.elapsed().as_millis() as u32);
                            facts.upstream_ms = head_ms;
                            last_upstream_status = Some(head.status);
                            facts.last_upstream_status = Some(head.status);
                            // ADR-060 D1: a head arrived (any status — an
                            // error head being composed when the client
                            // leaves is arm A with its `upstream_status`
                            // intact) and the request was fully written.
                            cancel.note_upstream_status(head.status, head_ms);
                            cancel.note_wrote_full_request();
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
                                                candidates: metered_candidates_field(
                                                    &self.config,
                                                    policy,
                                                ),
                                                chosen: Some(policy.primary.to_string()),
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
                                    request_deadline: deadline,
                                    request_id: request_id.to_string(),
                                    proto_in,
                                    session: session.clone(),
                                    turn_index,
                                    requested_model: Some(model.to_string()),
                                    selection_source,
                                    decision_ms,
                                    started,
                                    now_epoch_s,
                                    now_us: facts.now_us,
                                    received_event: facts.received_event,
                                    blocks: facts.blocks.clone(),
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
                                    cancel: Arc::clone(cancel),
                                };
                                let state = RelayState {
                                    tap: SseUsageExtractor::new(cand.wire, client_requested_usage),
                                    head,
                                    relayed: 0,
                                    // The tail of the chain after the answering
                                    // candidate — owned, because this loop
                                    // consumed the vector (into_iter).
                                    attempt_count: facts.attempts,
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
                                    content_type: content_type.clone(),
                                    last_byte_at: None,
                                    head_arrived: Instant::now(),
                                };
                                // ADR-049 §5.6 (the buffered path's twin):
                                // this 2xx head is where the walk settled —
                                // the settle runs before the ctx (which the
                                // relay's trace line reads) is moved below.
                                if let Some(ps) = facts.plan_switch.as_mut() {
                                    ps.settled_on(&settled_label);
                                }
                                facts.failover_from = failover_from.clone();
                                // ADR-060 D1: the relay is being entered —
                                // a 2xx head is forwarded and the body goes
                                // in flight. From here a drop is arm B,
                                // whatever its producer.
                                cancel.note_relay_entered();
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
                            // ADR-060 D2: the bounded, redacted slice of
                            // the upstream's own error body, from the same
                            // bytes the classifier reads below — attached
                            // to the terminal failure record (never the
                            // client body, which on this arm never sees a
                            // byte of the error head at all).
                            {
                                let creds: Vec<&str> = self
                                    .api_keys
                                    .values()
                                    .flatten()
                                    .map(String::as_str)
                                    .collect();
                                facts.upstream_body =
                                    crate::upstream_body::capture(&resp.body, &creds);
                            }
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
                            // ADR-056 §6.2: the classification row carries its
                            // demotion's one object and lands the projection
                            // from the same `until_us` — one call, no second
                            // writer to drift from the payload.
                            self.record_classification(
                                request_id,
                                attempt_index,
                                Some(status),
                                &cls,
                                Some(&cand.route),
                            );
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
                                    let metered = metered_walk(&self.config, policy);
                                    if tier.contains(&cand.route) {
                                        let from = cand.route.clone();
                                        let next_plan = tier
                                            .iter()
                                            .position(|r| r == &cand.route)
                                            .and_then(|pos| tier.get(pos + 1))
                                            .cloned();
                                        let to = next_plan.clone().unwrap_or_else(|| {
                                            metered
                                                .first()
                                                .cloned()
                                                .unwrap_or_else(|| policy.overflow.clone())
                                        });
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
                                                    policy, &tier, &metered, &from, &to,
                                                ),
                                                probe: false,
                                                reprefill_tokens: reprefill,
                                                switch_cost_nano: cost_nano,
                                                cost_currency: self.route_currency(&to),
                                                candidates: metered_candidates_field(
                                                    &self.config,
                                                    policy,
                                                ),
                                                chosen: Some(to.to_string()),
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
                            // ADR-060 §8.2: the head is the decision — any
                            // status, any body length, complete or truncated.
                            // The attempt is terminal here; the walk below is
                            // its recovery, and no same-route retry is a
                            // candidate (a read that ends short is fewer
                            // evidence bytes, never a retry input — R12/E4).
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
                            let mut details = json!({
                                "upstream_status": status,
                                "error_class": cls.class.as_str(),
                                "stream": true,
                            });
                            if deterministic {
                                // ADR-060 D3, spec §4.2's table row 6: a
                                // re-probe reproduces the refusal (the
                                // buffered twin is exhausted_failure).
                                details["retry_refused"] = json!("deterministic");
                            }
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
                                details: Some(details),
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
                            // ADR-060 D3, spec §4.2's evidence table row 1
                            // (the streaming twin of the buffered call
                            // site): no request bytes went out — nothing
                            // billed, nothing forwarded — so the SAME
                            // route is retried once, 250 ms, BEFORE the
                            // walk. The classification row is written
                            // inside the verdict, its action the recovery
                            // actually taken (ADR-060 §4.3 + conf_29's
                            // owner receipt, 2026-10-24): the buffered
                            // twin's ordered pair, on this medium too.
                            let retry_refused_word = match crate::forward::same_route_retry_verdict(
                                retries_used,
                                started,
                                self.config.server.request_timeout.0,
                            ) {
                                crate::forward::SameRouteRetry::Allowed => {
                                    let _ = self.record_classification_retry(
                                        request_id,
                                        attempt_index,
                                        None,
                                        &cls,
                                        Some(&cand.route),
                                    );
                                    tokio::time::sleep(crate::forward::retry_backoff()).await;
                                    retries_used += 1;
                                    attempt_index += 1;
                                    continue 'keys;
                                }
                                crate::forward::SameRouteRetry::Refused(word) => {
                                    self.record_classification(
                                        request_id,
                                        attempt_index,
                                        None,
                                        &cls,
                                        Some(&cand.route),
                                    );
                                    word
                                }
                            };
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
                                    // ADR-060 D3: the cap or the floor, one
                                    // word (spec §4.2's closed set).
                                    "retry_refused": retry_refused_word,
                                })),
                            });
                        }
                        OpenHead::UnknownOutcome(message) => {
                            // Full write, no head: `unknown_outcome` (ADR-011
                            // item 6 row 3) — no retry, no failover, deliberately
                            // no closure event.
                            facts.upstream_ms = Some(attempt_started.elapsed().as_millis() as u32);
                            // ADR-060 D1: the request bytes were fully
                            // written — the guard's `stage` evidence.
                            cancel.note_wrote_full_request();
                            return StreamOutcome::Failure(ForwardFailure {
                                status: 502,
                                code: ErrorCode::UpstreamError,
                                message,
                                details: Some(json!({
                                    "stage": "unknown_outcome",
                                    "error_class": "timeout",
                                    "stream": true,
                                    // ADR-060 D3: the class table refuses
                                    // this attempt by name (row 3) — the
                                    // request may have been billed.
                                    "retry_refused": "billed",
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
        // ADR-056 §2.5's advisory half, mirroring the walk's own skip: a
        // silence-cooling route is named only when no eligible
        // non-cooling candidate exists, so the narration and the walk
        // cannot disagree about where the request lands.
        let eligible = |c: &Candidate| {
            !attempted.contains(&c.route.provider) && !self.provider_in_cooldown(&c.route.provider)
        };
        candidates
            .iter()
            .find(|c| eligible(c) && !self.route_silence_cooling(&c.route))
            .or_else(|| candidates.iter().find(|c| eligible(c)))
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

/// One truncation's declared shape (spec §4.20.3): the prose reason the
/// entry's message carries beside its fixed prefix, the closed-vocabulary
/// cause word (`idle_timeout` | `upstream_ended_early` | `request_timeout`
/// | `client_closed`), the window that was in force for the tripping
/// read — present **iff** a bound tripped — and §4.2's retry-refusal
/// word: a cut after a `2xx` head is a served attempt (the model was
/// invoked), so it is never retried and the record says why —
/// `forwarded` when the client's output is already on the wire,
/// `billed` when the cut came before the first byte, `budget` when the
/// whole-request bound is what tripped (no budget remains).
struct TruncationInfo {
    reason: String,
    stream_end: &'static str,
    window_ms: Option<u64>,
    retry_refused: &'static str,
}

/// §4.2's word for a cut the relay reports, from the two facts the rule
/// turns on: whether any byte reached the client, and whether our own
/// whole-request bound is what tripped.
fn cut_retry_refused(relayed: u64, request_bound: bool) -> &'static str {
    if relayed > 0 {
        "forwarded"
    } else if request_bound {
        "budget"
    } else {
        "billed"
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
            // ADR-053 §3, the relay's leg of the single owner: each
            // upstream read still runs under R4's idle bound, but the
            // WHOLE request now also trips at the absolute
            // `server.request_timeout` deadline — the same instant
            // the head-phase wrap enforces, never re-armed. The
            // deadline is in the past ⇒ trip now; otherwise the
            // effective window is min(idle, time-until-deadline).
            let window = ctx
                .request_deadline
                .checked_duration_since(tokio::time::Instant::now())
                .unwrap_or_default();
            let idle = std::cmp::min(ctx.idle, window);
            let window_ms = idle.as_millis() as u64;
            match vadis_providers::stream::read_chunk(&mut st.head, idle).await {
                vadis_providers::stream::StreamRead::Chunk(b) => {
                    if !b.is_empty() {
                        // §12.10.3 R2 write-through: this chunk is the item; the
                        // tap reads a copy (§12.10.3 R7).
                        st.tap.feed(&b);
                        st.relayed += b.len() as u64;
                        st.last_byte_at = Some(Instant::now());
                        // ADR-060 D1: the shared byte counter — the
                        // guard reads it at drop time because the
                        // drop destroys the relay's own state.
                        ctx.cancel.note_bytes_relayed(st.relayed);
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
                    // Spec §4.20.3's three-valued completeness predicate,
                    // asked ONLY at this arm (the cut arms are
                    // media-blind truncations by construction). Two
                    // facts, both the wire's own: the answering head's
                    // media type and `terminal_seen`. Not SSE (an
                    // absent header included) ⇒ a body that ends is a
                    // completion — nothing was owed. SSE with the
                    // terminal seen ⇒ completion. SSE without it ⇒ a
                    // truncation whose cause is `upstream_ended_early`
                    // — no bound tripped, so no `window_ms`.
                    let truncated =
                        if vadis_protocol::sse::is_text_event_stream(st.content_type.as_deref())
                            && !st.tap.terminal_seen()
                        {
                            Some(TruncationInfo {
                                reason: "the upstream closed the body before its wire's \
                                         terminal event"
                                    .to_string(),
                                stream_end: "upstream_ended_early",
                                window_ms: None,
                                // ADR-060 D3: a cut after a 2xx head is a
                                // served attempt — never retried, and the
                                // word says which of the two facts refused.
                                retry_refused: cut_retry_refused(st.relayed, false),
                            })
                        } else {
                            None
                        };
                    if let Some(t) = truncated.as_ref() {
                        record_classified(&ctx, &st, t);
                    }
                    record_terminal(&ctx, &st, truncated);
                    return None;
                }
                vadis_providers::stream::StreamRead::Failed { message, timed_out } => {
                    // Which bound fired (ADR-053 §3): a `timed_out`
                    // read at or past the absolute deadline is the
                    // WHOLE-request bound, not the idle gap — the
                    // declared reason must name the one that tripped.
                    let request_bound =
                        timed_out && tokio::time::Instant::now() >= ctx.request_deadline;
                    // ADR-060 D3 (spec §4.2's table row 4): a cut
                    // after a `2xx` head is a served attempt — the
                    // model was invoked — so it is TERMINAL: no
                    // same-route retry, no walk of any kind, however
                    // many bytes were relayed. The old zero-byte
                    // failover ("relayed == 0 may walk") was the
                    // proxy this replaces: "no byte reached the
                    // client" is not "nothing was billed". The relay
                    // therefore has no retry branch at all — record,
                    // close, and the record's `retry_refused` word
                    // says the table was consulted.
                    let trunc = if request_bound {
                        TruncationInfo {
                            reason: "server.request_timeout exceeded: the whole-request \
                                         bound expired before the stream completed"
                                .to_string(),
                            stream_end: "request_timeout",
                            window_ms: Some(window_ms),
                            retry_refused: cut_retry_refused(st.relayed, true),
                        }
                    } else if timed_out {
                        TruncationInfo {
                            reason: format!("idle bound exceeded: {message}"),
                            stream_end: "idle_timeout",
                            window_ms: Some(window_ms),
                            retry_refused: cut_retry_refused(st.relayed, false),
                        }
                    } else {
                        // A transport error mid-body: the upstream (or
                        // the path to it) cut the stream — no bound of
                        // ours tripped, so no window is reported.
                        TruncationInfo {
                            reason: message,
                            stream_end: "upstream_ended_early",
                            window_ms: None,
                            retry_refused: cut_retry_refused(st.relayed, false),
                        }
                    };
                    record_classified(&ctx, &st, &trunc);
                    record_terminal(&ctx, &st, Some(trunc));
                    return None;
                }
            }
        }
    })
}

/// `error.classified` for a mid-body truncation (§12.10.5 row 7, the
/// relay's arm). When — and only when — the cause is `idle_timeout`
/// (ADR-056 §2.5: a graceful early close and our own deadline order
/// nothing), the payload carries the demotion object in its ONE shape
/// and the route-scoped `provider_cooldown` projection lands from the
/// SAME `until_us` value (CONF-21's identity: the rebuild reads the
/// payload, the live row reads this write, and the two cannot differ).
fn record_classified(ctx: &RelayCtx, st: &RelayState, trunc: &TruncationInfo) {
    let mut payload = json!({
        "attempt_index": st.attempt_count.saturating_sub(1),
        "status": Value::Null,
        "reason": "stream_truncated",
        "action": "abort",
        "matched": false,
        "detail": trunc.reason,
        "stream": true,
    });
    // ADR-060 §4.3 (spec §4.20.3, R1): the per-attempt cause word rides
    // the classification row too, beside its `detail` — the sequence of
    // causes across attempts is readable from the log alone.
    payload["stream_end"] = json!(trunc.stream_end);
    let mut cooldown_write: Option<(String, String, i64)> = None;
    if trunc.stream_end == "idle_timeout" {
        let (provider, model) = split_route_label(&st.route_label);
        let until_us = now_us() + (crate::forward::STREAM_SILENCE_TTL_S as i64) * 1_000_000;
        payload["demotion"] = crate::forward::demotion_json(
            "route",
            provider,
            model,
            until_us,
            crate::forward::STREAM_SILENCE_TTL_S,
            crate::forward::REASON_STREAM_SILENCE,
        );
        cooldown_write = Some((provider.to_string(), model.to_string(), until_us));
    }
    let anchor = event(ctx, EventKind::ErrorClassified, payload);
    if let (Some(store), Some((provider, model, until_us))) = (&ctx.store, cooldown_write) {
        let _ = store.project(vadis_core::store::ProjectionWrite::ProviderCooldown {
            scope: "route",
            provider: &provider,
            model: &model,
            until_us,
            reason: crate::forward::REASON_STREAM_SILENCE,
            last_event: anchor.unwrap_or(EventId(0)),
        });
    }
}

/// The terminal `upstream.responded` (R8/R11) **and** the buffered
/// path's closing stages: usage from the tap or
/// `usage_missing: true`; then §12.10.5 note R2's order — cost, the trace line,
/// `cost.computed` + `quota.charged`, the ledger put (§12.10.5 note R3: the
/// accounting rows commit at stream end, before the last byte is
/// written through). A truncation rides in `errors[]`, never presented
/// as completeness; nothing is charged twice.
fn record_terminal(ctx: &RelayCtx, st: &RelayState, truncated: Option<TruncationInfo>) {
    // ADR-060 D1: the invariant's take-before-write — the relay's own
    // terminal row is about to exist, so the cancelled-row guard (which
    // shares this slot from inside the response body) can never add a
    // second one when the body is later dropped.
    ctx.cancel.take();
    let (usage, usage_missing) = match st.tap.finish() {
        SseUsageOutcome::Complete(u) => (Some(u), false),
        SseUsageOutcome::Missing => (None, true),
    };
    // Spec §4.20.3: measured from the last byte received from the
    // upstream, or from the answering head's arrival when no byte ever
    // arrived. Always present on a truncation entry.
    let since_last_byte_ms = Instant::now()
        .duration_since(st.last_byte_at.unwrap_or(st.head_arrived))
        .as_millis() as u64;
    // `stream_end` rides the event only when the stream was truncated —
    // absent on a completion, never a null with a key (spec §4.20.3).
    let mut responded = json!({
        "status": 200,
        "route": st.route_label,
        "attempt_index": st.attempt_count.saturating_sub(1),
        "wrote_full_request": true,
        "stream": true,
        "stream_completed": truncated.is_none(),
        "stream_truncated_reason": truncated.as_ref().map(|t| t.reason.clone()),
        "bytes_relayed": st.relayed > 0,
        "usage": usage.map(|u| json!({
            "input_total": u.input_total,
            "input_cached": u.input_cached,
            "cache_write": u.cache_write,
            "output": u.output,
            "reasoning": u.reasoning,
        })),
        "usage_missing": usage_missing,
    });
    if let Some(t) = &truncated {
        responded["stream_end"] = json!(t.stream_end);
    }
    event(ctx, EventKind::UpstreamResponded, responded);

    // The ledger (§12.10.5 note R2's order): measure continuity against
    // the PREVIOUS block set, compute cost, write the trace line, commit
    // the accounting rows, then replace the session's blocks for the
    // NEXT request.
    let mut blocks = ctx.blocks.clone();
    if let Some(u) = &usage {
        attribute_tokens(&mut blocks, u);
    }
    let mut errors: Vec<TraceError> = Vec::new();
    if let Some(t) = &truncated {
        // Spec §4.20.3: the entry names its cause and carries its three
        // observables. `window_ms` is present iff a bound tripped — the
        // window in force for the tripping read, never the configured key
        // alone.
        let mut details = json!({
            "error_class": "stream_truncated",
            "stream": true,
            "stream_end": t.stream_end,
            "relayed_bytes": st.relayed,
            "since_last_byte_ms": since_last_byte_ms,
            // ADR-060 D3 / spec §4.20.3: the class table was consulted
            // and answered no — the word says which fact refused the
            // retry.
            "retry_refused": t.retry_refused,
        });
        if let Some(w) = t.window_ms {
            details["window_ms"] = json!(w);
        }
        errors.push(TraceError {
            kind: "upstream_error".to_string(),
            message: format!(
                "stream truncated before its terminal event; not retried: {}",
                t.reason
            ),
            plugin: None,
            details: Some(details),
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
        now_us: ctx.now_us,
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
        // ADR-060 D3: the attempts this request made across all routes,
        // counted by the pre-relay walk and carried into the relay.
        st.attempt_count,
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
                    // R74-0-F7: the ANSWERING head's label, kept fresh
                    // across a zero-byte failover.
                    content_type: st.content_type.clone(),
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

#[cfg(test)]
mod tests {
    //! ADR-053's unit witnesses on the driver seam: the single-owner
    //! `server.request_timeout` on the stream path (head phase and relay
    //! tripping at the SAME absolute instant, one timer, never re-armed),
    //! and the head-arrival attempt bound's conservative arm.
    use super::*;
    use std::sync::Mutex as StdMutex;
    use tokio::io::AsyncReadExt;
    use vadis_core::config::VadisConfig;
    use vadis_core::trace::DecisionRecord;
    use vadis_core::transform::TransformMode;

    /// Captures every record the driver writes, for assertions.
    #[derive(Default)]
    struct CaptureTrace {
        records: StdMutex<Vec<String>>,
    }

    impl vadis_core::TraceWriter for CaptureTrace {
        fn write(&self, rec: &DecisionRecord) -> Result<Option<String>, String> {
            self.records
                .lock()
                .unwrap()
                .push(serde_json::to_string(rec).unwrap());
            Ok(Some("test:0".into()))
        }
        fn config_digest(&self) -> &str {
            ""
        }
    }

    /// Captures every event the walks and the relay append, for
    /// assertions on the event log's own rows (`error.classified`
    /// carries the per-attempt cause word, R1).
    #[derive(Default)]
    struct CaptureStore {
        events: StdMutex<Vec<(vadis_core::store::EventKind, Value)>>,
    }

    impl vadis_core::store::Store for CaptureStore {
        fn append(
            &self,
            ev: vadis_core::store::NewEvent<'_>,
        ) -> Result<vadis_core::store::EventId, vadis_core::store::StoreError> {
            let mut events = self.events.lock().unwrap();
            events.push((ev.kind, ev.payload.clone()));
            Ok(vadis_core::store::EventId(events.len() as i64))
        }
        fn project(
            &self,
            _w: vadis_core::store::ProjectionWrite<'_>,
        ) -> Result<(), vadis_core::store::StoreError> {
            Ok(())
        }
        fn query(
            &self,
            _q: vadis_core::store::Query<'_>,
        ) -> Result<vadis_core::store::QueryRow, vadis_core::store::StoreError> {
            Ok(vadis_core::store::QueryRow::Count(0))
        }
        fn rebuild(
            &self,
            _which: vadis_core::store::Projection,
        ) -> Result<vadis_core::store::RebuildStats, vadis_core::store::StoreError> {
            Ok(vadis_core::store::RebuildStats::default())
        }
        fn schema_version(&self) -> Result<u32, vadis_core::store::StoreError> {
            Ok(1)
        }
    }

    impl CaptureStore {
        /// Every `error.classified` payload, in append order.
        fn classified(&self) -> Vec<Value> {
            self.events
                .lock()
                .unwrap()
                .iter()
                .filter(|(k, _)| matches!(k, vadis_core::store::EventKind::ErrorClassified))
                .map(|(_, v)| v.clone())
                .collect()
        }
    }

    /// A config whose one `mock` provider serves chat from the given
    /// loopback port, under the given bounds.
    fn test_config(attempt_ms: u64, request_ms: u64, upstream_port: u16) -> VadisConfig {
        serde_json::from_value(serde_json::json!({
            "server": {
                "addr": "127.0.0.1:0",
                "upstream_attempt_timeout": format!("{attempt_ms}ms"),
                "request_timeout": format!("{request_ms}ms"),
            },
            "session": { "key_sources": ["prompt_cache_key"], "ttl": "11h" },
            "cache": { "sticky": true,
                       "breakeven": { "enabled": true, "min_remaining_turns": 2, "safety_factor": 1.1 } },
            "trace": { "dir": "./state/traces", "rollover": "hourly" },
            "providers": [{
                "name": "mock",
                "urls": { "chat": format!("http://127.0.0.1:{upstream_port}/v1/chat/completions") },
                "api_key_env": "UNIT_STREAM_KEY",
                "wire_api": "chat",
                "supports": ["chat"],
                "models": [{
                    "id": "m", "context": "128k",
                    "price": { "input_miss": 0.001, "input_hit": 0.0001,
                               "cache_write": 0.0, "output": 0.002,
                               "peak": { "multiplier": 1.0, "windows": [] } },
                    "source": "unit fixture"
                }]
            }],
            "aliases": {},
            "plugins": [],
            "fallback": []
        }))
        .unwrap()
    }

    /// The R4 rig's config: a `refuser` provider whose upstream answers
    /// an error head with a body, then a `dead` provider on a closed
    /// port as the fallback — the walk's two steps.
    fn walk_test_config(attempt_ms: u64, request_ms: u64, refuser_port: u16, dead_port: u16) -> VadisConfig {
        let provider = |name: &str, port: u16| serde_json::json!({
            "name": name,
            "urls": { "chat": format!("http://127.0.0.1:{port}/v1/chat/completions") },
            "api_key_env": "UNIT_STREAM_KEY",
            "wire_api": "chat",
            "supports": ["chat"],
            "models": [{
                "id": "m", "context": "128k",
                "price": { "input_miss": 0.001, "input_hit": 0.0001,
                           "cache_write": 0.0, "output": 0.002,
                           "peak": { "multiplier": 1.0, "windows": [] } },
                "source": "unit fixture"
            }]
        });
        serde_json::from_value(serde_json::json!({
            "server": {
                "addr": "127.0.0.1:0",
                "upstream_attempt_timeout": format!("{attempt_ms}ms"),
                "request_timeout": format!("{request_ms}ms"),
            },
            "session": { "key_sources": ["prompt_cache_key"], "ttl": "11h" },
            "cache": { "sticky": true,
                       "breakeven": { "enabled": true, "min_remaining_turns": 2, "safety_factor": 1.1 } },
            "trace": { "dir": "./state/traces", "rollover": "hourly" },
            "providers": [provider("refuser", refuser_port), provider("dead", dead_port)],
            "aliases": {},
            "plugins": [],
            "fallback": ["dead/m"]
        }))
        .unwrap()
    }

    /// A port that is (almost certainly) closed: bind, read the port,
    /// drop the listener. The walk's connect-failure step points here.
    fn dead_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    fn forwarder(cfg: VadisConfig, trace: Option<std::sync::Arc<CaptureTrace>>) -> Forwarder {
        forwarder_with(cfg, trace, None)
    }

    fn forwarder_with(
        cfg: VadisConfig,
        trace: Option<std::sync::Arc<CaptureTrace>>,
        store: Option<std::sync::Arc<CaptureStore>>,
    ) -> Forwarder {
        Forwarder {
            config: cfg,
            transports: HashMap::new(),
            api_keys: HashMap::from([
                ("mock".to_string(), vec!["sk-unit".to_string()]),
                ("refuser".to_string(), vec!["sk-unit".to_string()]),
                ("dead".to_string(), vec!["sk-unit".to_string()]),
            ]),
            store: store.map(|s| s as std::sync::Arc<dyn vadis_core::store::Store>),
            trace: trace.map(|t| t as std::sync::Arc<dyn vadis_core::TraceWriter>),
            transform_engine: None,
            response_cache: None,
            session_ttl_us: 0,
        }
    }

    const STREAM_BODY: &[u8] =
        br#"{"model":"mock/m","messages":[{"role":"user","content":"x"}],"stream":true}"#;

    /// A listener that accepts, reads the request, and never answers —
    /// the slow-head fixture. Each accepted connection is held open.
    async fn slow_head_listener() -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 8192];
                    // Hold the connection open, draining quietly.
                    loop {
                        match tokio::time::timeout(
                            std::time::Duration::from_secs(5),
                            s.read(&mut buf),
                        )
                        .await
                        {
                            Ok(Ok(0)) | Err(_) => return,
                            Ok(Ok(_)) => {}
                            Ok(Err(_)) => return,
                        }
                    }
                });
            }
        });
        port
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn request_timeout_trips_head_phase_into_unknown_outcome_refusal() {
        let port = slow_head_listener().await;
        // attempt (head) bound 10s, whole-request bound 150ms: the OUTER
        // bound must fire first — the change's own discriminating rig.
        let f = forwarder(test_config(10_000, 150, port), None);
        let t0 = std::time::Instant::now();
        let outcome = f
            .forward_stream(
                WireApi::Chat,
                STREAM_BODY,
                "req-unit",
                &[],
                TransformMode::Passthrough,
            )
            .await;
        let elapsed = t0.elapsed();
        assert!(elapsed >= std::time::Duration::from_millis(140));
        assert!(
            elapsed < std::time::Duration::from_millis(2_000),
            "trips at the bound, not far past it: {elapsed:?}"
        );
        match outcome {
            StreamOutcome::Failure(f) => {
                assert_eq!(f.status, 502);
                assert_eq!(f.code, ErrorCode::UpstreamError);
                let d = f.details.expect("details");
                assert_eq!(d["stage"], "unknown_outcome");
                assert_eq!(d["error_class"], "timeout");
                assert_eq!(d["stream"], true);
            }
            StreamOutcome::Success(_) => panic!("the head phase must trip, not serve"),
        }
    }

    /// A trickle upstream: head + one SSE chunk every 30ms (every gap
    /// well under any idle bound used here), forever — past whatever
    /// bound the caller arms. The relay keeps producing while the
    /// caller keeps polling.
    async fn trickle_listener() -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = vec![0u8; 8192];
                    let _ =
                        tokio::time::timeout(std::time::Duration::from_secs(2), s.read(&mut buf))
                            .await;
                    let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n";
                    if s.write_all(head.as_bytes()).await.is_err() {
                        return;
                    }
                    let mut n = 0u32;
                    loop {
                        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                        let data = format!("data: tick{n}\n\n");
                        let frame = format!("{:x}\r\n{}\r\n", data.len(), data);
                        if s.write_all(frame.as_bytes()).await.is_err() {
                            return; // the vadis dropped the connection — expected
                        }
                        n += 1;
                    }
                });
            }
        });
        port
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn request_timeout_trips_a_busy_relay_into_declared_truncation() {
        // A trickle upstream: head + one SSE chunk every 30ms (every gap
        // well under the 10s idle bound), forever — past the 400ms
        // whole-request bound. The relay must be killable at the bound
        // and the trip must be a DECLARED truncation.
        let port = trickle_listener().await;
        let trace = std::sync::Arc::new(CaptureTrace::default());
        let f = forwarder(test_config(10_000, 400, port), Some(trace.clone()));
        let t0 = std::time::Instant::now();
        let outcome = f
            .forward_stream(
                WireApi::Chat,
                STREAM_BODY,
                "req-unit",
                &[],
                TransformMode::Passthrough,
            )
            .await;
        assert!(
            t0.elapsed() < std::time::Duration::from_millis(3_000),
            "the driver returns at the head; the relay is killed at the bound"
        );
        match outcome {
            StreamOutcome::Success(s) => {
                // Drain the relay: it must END (the truncation), never run
                // forever, and must have relayed some bytes first.
                let mut body = Box::pin(s.body);
                let mut items = 0u32;
                while let Some(_b) = futures::StreamExt::next(&mut body).await {
                    items += 1;
                    assert!(
                        items < 10_000,
                        "the relay must be killable at request_timeout"
                    );
                }
                assert!(
                    items > 0,
                    "some bytes must have been relayed before the trip"
                );
                let records = trace.records.lock().unwrap().clone();
                assert_eq!(records.len(), 1, "one terminal record");
                let rec: serde_json::Value = serde_json::from_str(&records[0]).unwrap();
                assert_eq!(rec["result"]["status"], 200);
                let errors = rec["errors"].as_array().expect("errors[]");
                assert!(
                    errors
                        .iter()
                        .any(|e| e["details"]["error_class"] == "stream_truncated"),
                    "the whole-request trip is a DECLARED truncation: {errors:?}"
                );
                assert_eq!(rec["usage_missing"], true, "usage honesty (R8)");
            }
            StreamOutcome::Failure(f) => {
                panic!(
                    "the head arrived; the outcome must be a relay, not status {} {:?}",
                    f.status, f.details
                )
            }
        }
    }

    // -- ADR-060 D1: the cancelled row on the shipped cancellation path --

    /// Arm A, wired (the disabling edit is `CancelGuard::new` in
    /// `forward_stream`): dropping the handler future mid-head-phase —
    /// what hyper does when the client disconnects before any response
    /// head exists — writes exactly one terminal row: `499`, kind
    /// `canceled`, `stage: not_billed` (an attempt was submitted but
    /// this process never observed a full write), and NO second row
    /// ever appears.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_the_head_phase_future_writes_the_canceled_row() {
        let port = slow_head_listener().await;
        let trace = std::sync::Arc::new(CaptureTrace::default());
        let f = forwarder(test_config(10_000, 10_000, port), Some(trace.clone()));
        let fut = f.forward_stream(
            WireApi::Chat,
            STREAM_BODY,
            "req-cancel-a",
            &[],
            TransformMode::Passthrough,
        );
        // Drop the future the way hyper drops a disconnected client's
        // handler future: mid-await, before any outcome existed.
        let dropped = tokio::time::timeout(std::time::Duration::from_millis(150), fut)
            .await
            .is_err();
        assert!(dropped, "the future must still be awaiting its head");
        let records = trace.records.lock().unwrap().clone();
        assert_eq!(records.len(), 1, "exactly one terminal row");
        let rec: serde_json::Value = serde_json::from_str(&records[0]).unwrap();
        assert_eq!(rec["result"]["status"], 499);
        assert_eq!(rec["result"]["attempts"], 1);
        let e = &rec["errors"][0];
        assert_eq!(e["kind"], "canceled");
        assert_eq!(e["details"]["stage"], "not_billed");
        assert_eq!(e["details"]["canceled_by"], "client");
        assert_eq!(e["details"]["wrote_full_request"], false);
    }

    /// Arm B, wired (the disabling edit is `guard_body` in
    /// `forward_stream`'s success arm): a `2xx` head was forwarded and
    /// bytes were relayed, then the response body is dropped — what
    /// hyper does when the client disconnects mid-stream. Exactly one
    /// row: the head's status, `stream_end: client_closed`,
    /// `relayed_bytes > 0`, `retry_refused: forwarded`, and no second
    /// row from the guard after the relay's own terminal write would
    /// have run.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_the_response_body_mid_stream_writes_the_client_closed_row() {
        let port = trickle_listener().await;
        let trace = std::sync::Arc::new(CaptureTrace::default());
        let f = forwarder(test_config(10_000, 10_000, port), Some(trace.clone()));
        let outcome = f
            .forward_stream(
                WireApi::Chat,
                STREAM_BODY,
                "req-cancel-b",
                &[],
                TransformMode::Passthrough,
            )
            .await;
        let s = match outcome {
            StreamOutcome::Success(s) => s,
            StreamOutcome::Failure(f) => {
                panic!("the trickle head must answer: {} {}", f.status, f.message)
            }
        };
        // Read at least one chunk so bytes are on the wire, then drop
        // the body the way hyper drops it on a client disconnect.
        let mut body = Box::pin(s.body);
        use futures::StreamExt as _;
        let first = body.next().await;
        assert!(first.is_some(), "the trickle relayed its first chunk");
        drop(body);
        let records = trace.records.lock().unwrap().clone();
        assert_eq!(records.len(), 1, "exactly one terminal row");
        let rec: serde_json::Value = serde_json::from_str(&records[0]).unwrap();
        assert_eq!(rec["result"]["status"], 200);
        assert_eq!(rec["result"]["attempts"], 1);
        let e = &rec["errors"][0];
        assert_eq!(e["kind"], "upstream_error");
        assert_eq!(e["details"]["error_class"], "stream_truncated");
        assert_eq!(e["details"]["stream_end"], "client_closed");
        assert!(
            e["details"]["relayed_bytes"].as_u64().unwrap_or(0) > 0,
            "the dropped body had relayed bytes"
        );
        assert_eq!(e["details"]["retry_refused"], "forwarded");
    }

    /// ADR-060 D3, shape A on the wire (the disabling edit is
    /// `cut_retry_refused` / the `retry_refused` member in
    /// `record_terminal`): an upstream that serves a 2xx head and a few
    /// chunks and then cuts the connection is TERMINAL — the truncation
    /// entry carries `retry_refused: "forwarded"`, the upstream saw
    /// exactly ONE request (no same-route retry, no walk: the old
    /// zero-byte failover is gone), and `attempts` stays 1.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_cut_after_relayed_bytes_is_terminal_with_the_forwarded_refusal() {
        // A burst-then-cut upstream: head, three SSE chunks, then the
        // connection is dropped mid-body (no terminal event).
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let served = std::sync::Arc::new(std::sync::Mutex::new(0u32));
        let served_probe = served.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else {
                    return;
                };
                *served.lock().unwrap() += 1;
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = vec![0u8; 8192];
                    let _ =
                        tokio::time::timeout(std::time::Duration::from_secs(2), s.read(&mut buf))
                            .await;
                    let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n";
                    if s.write_all(head.as_bytes()).await.is_err() {
                        return;
                    }
                    for n in 0..3u32 {
                        let data = format!("data: tick{n}\n\n");
                        let frame = format!("{:x}\r\n{}\r\n", data.len(), data);
                        if s.write_all(frame.as_bytes()).await.is_err() {
                            return;
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    // CUT: drop the socket without the SSE terminal.
                    drop(s);
                });
            }
        });
        let trace = std::sync::Arc::new(CaptureTrace::default());
        let store = std::sync::Arc::new(CaptureStore::default());
        let f = forwarder_with(
            test_config(10_000, 10_000, port),
            Some(trace.clone()),
            Some(store.clone()),
        );
        let outcome = f
            .forward_stream(
                WireApi::Chat,
                STREAM_BODY,
                "req-cut",
                &[],
                TransformMode::Passthrough,
            )
            .await;
        let s = match outcome {
            StreamOutcome::Success(s) => s,
            StreamOutcome::Failure(f) => panic!("the head must answer: {}", f.message),
        };
        // Drain to the truncation.
        let mut body = Box::pin(s.body);
        use futures::StreamExt as _;
        while let Some(_b) = body.next().await {}
        assert_eq!(
            *served_probe.lock().unwrap(),
            1,
            "exactly one upstream request — no retry, no walk"
        );
        let records = trace.records.lock().unwrap().clone();
        assert_eq!(records.len(), 1);
        let rec: serde_json::Value = serde_json::from_str(&records[0]).unwrap();
        assert_eq!(rec["result"]["status"], 200);
        assert_eq!(rec["result"]["attempts"], 1);
        let e = &rec["errors"][0];
        assert_eq!(e["details"]["error_class"], "stream_truncated");
        assert_eq!(e["details"]["retry_refused"], "forwarded");
        assert!(e["details"]["relayed_bytes"].as_u64().unwrap_or(0) > 0);
        // ADR-060 §4.3 / spec §4.20.3 (R1): the classification row —
        // `error.classified`, written before the terminal record —
        // carries the same `stream_end` cause word beside its `detail`,
        // so the per-attempt sequence of causes is readable from the
        // log alone. This cut is an upstream that dropped the body
        // mid-stream: the word is `upstream_ended_early`.
        let classified = store.classified();
        assert_eq!(
            classified.len(),
            1,
            "exactly one classification for the one truncated attempt"
        );
        assert_eq!(classified[0]["reason"], "stream_truncated");
        assert_eq!(classified[0]["action"], "abort");
        assert_eq!(
            classified[0]["stream_end"], "upstream_ended_early",
            "the per-attempt cause word rides the classification row"
        );
    }

    /// ADR-060 D2 (R4): the refusal slice and the answering head are
    /// per attempt. The first attempt answers a 429 error head WITH a
    /// body (the walk's failover input); every later attempt — here a
    /// connect failure that never sent a byte — must not carry that
    /// slice as its own `errors[0].details.upstream_body`, and the
    /// terminal record names no `upstream_status` it did not answer.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_next_attempt_carries_no_previous_attempts_refusal_slice() {
        // The refuser: accepts, reads the request, answers 429 with a
        // JSON error body (a body the slice would carry if it leaked).
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let refuser_port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = vec![0u8; 8192];
                    let _ =
                        tokio::time::timeout(std::time::Duration::from_secs(2), s.read(&mut buf))
                            .await;
                    let body = br#"{"error":{"message":"rate limited for key sk-unit","type":"rate_limit_error"}}"#;
                    let head = format!(
                        "HTTP/1.1 429 Too Many Requests\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = s.write_all(head.as_bytes()).await;
                    let _ = s.write_all(body).await;
                });
            }
        });
        let dead = dead_port();
        let trace = std::sync::Arc::new(CaptureTrace::default());
        let f = forwarder(
            walk_test_config(10_000, 10_000, refuser_port, dead),
            Some(trace.clone()),
        );
        let body = br#"{"model":"refuser/m","messages":[{"role":"user","content":"x"}],"stream":true}"#;
        let outcome = f
            .forward_stream(WireApi::Chat, body, "req-r4", &[], TransformMode::Passthrough)
            .await;
        match outcome {
            StreamOutcome::Failure(f) => {
                assert_eq!(f.status, 502, "the terminal failure is the walk's end");
            }
            StreamOutcome::Success(_) => panic!("a 429 then a dead fallback cannot serve"),
        }
        let records = trace.records.lock().unwrap().clone();
        assert_eq!(records.len(), 1, "one terminal record");
        let rec: serde_json::Value = serde_json::from_str(&records[0]).unwrap();
        // The walk really moved: the 429's failover plus the dead
        // route's attempts make `attempts >= 2` — this is what proves
        // an error head with a body existed on an EARLIER attempt.
        assert!(
            rec["result"]["attempts"].as_u64().unwrap_or(0) >= 2,
            "the walk must have moved past the 429 to the dead route: {}",
            records[0]
        );
        assert_eq!(
            rec["result"]["upstream_status"],
            serde_json::Value::Null,
            "no attempt of the terminal failure answered a head"
        );
        let e = &rec["errors"][0];
        assert_eq!(e["details"]["error_class"], "connect_failure");
        assert!(
            e["details"].get("upstream_body").is_none(),
            "the terminal record must not carry the 429 attempt's refusal slice: {}",
            e["details"]
        );
    }
}
