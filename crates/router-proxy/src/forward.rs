//! The buffered forwarding engine (R2-2d): native passthrough with byte
//! fidelity, usage normalization, ADR-011 error classification, the
//! fallback chain with provider-level exclusion, and the §12.10.5 event
//! sequence (`upstream.submitted` → `upstream.responded` →
//! `error.classified` → the action's effect).
//!
//! Non-streaming only: a `stream: true` inbound body is refused with 501
//! by this engine before any attempt runs (SSE lands in R2-2e), so an
//! attempt never half-relays.

use std::borrow::Cow;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use http::Request as HttpRequest;
use router_core::config::{ProviderCfg, RouteSpec, RouterConfig, WireApi};
use router_core::error::ErrorCode;
use router_core::error_class::{
    classify_upstream_error, Classification, ErrorClass, ErrorEvidence, TransportCause,
};
use router_core::prefix::{attribute_tokens, body_sha16, extract_prefix_blocks, PrefixBlock};
use router_core::store::{EventKind, NewEvent, ProjectionWrite, Query, QueryRow, Store};
use router_core::{RawBody, RawEditError, Usage, ROUTER_OWNED_TOP_LEVEL_KEYS};
use router_providers::{AttemptOutcome, TransportKind, UpstreamPlan};
use serde_json::{json, Value};

/// The async seam the engine is monomorphized over: the real reqwest client
/// and the test double both satisfy it (DESIGN §12.10.1: no `async-trait`).
pub trait ProviderSend {
    fn send(&self, req: HttpRequest<Bytes>) -> impl Future<Output = AttemptOutcome> + Send;
}

/// Object-safe wrapper so `Forwarder` can hold one transport per provider
/// without making the whole serving state generic. The future is tied to
/// `&self` and awaited immediately at the call site.
pub type BoxedAttempt<'a> = Pin<Box<dyn Future<Output = AttemptOutcome> + Send + 'a>>;

pub trait ProviderTransport: Send + Sync {
    fn send_boxed<'a>(&'a self, req: HttpRequest<Bytes>) -> BoxedAttempt<'a>;
}

impl<T> ProviderTransport for T
where
    T: ProviderSend + Send + Sync + 'static,
{
    fn send_boxed<'a>(&'a self, req: HttpRequest<Bytes>) -> BoxedAttempt<'a> {
        Box::pin(self.send(req))
    }
}

/// The production transport: the reqwest-backed client satisfies the
/// monomorphized seam (DESIGN §12.10.1).
impl ProviderSend for router_providers::ReqwestProviderClient {
    fn send(&self, req: HttpRequest<Bytes>) -> impl Future<Output = AttemptOutcome> + Send {
        router_providers::ReqwestProviderClient::send(self, req)
    }
}

/// One forwarded request's terminal state for the caller (the axum handler
/// or a conformance case).
pub enum ForwardOutcome {
    Success(ForwardSuccess),
    Failure(ForwardFailure),
}

pub struct ForwardSuccess {
    pub status: u16,
    pub content_type: Option<String>,
    /// The upstream's response bytes, verbatim (AGENTS constraint 1).
    pub body: Bytes,
    /// The route that answered.
    pub route: RouteSpec,
    /// Set when the request switched away from a failed route (spec §6).
    pub failover_from: Option<RouteSpec>,
    /// Normalized usage from the response, when the protocol carried one.
    pub usage: Option<Usage>,
    /// Prefix blocks of the outbound body that produced this answer
    /// (tokens attributed when usage exists — an `inferred` figure).
    pub prefix_blocks: Vec<PrefixBlock>,
}

#[derive(Clone)]
pub struct ForwardFailure {
    pub status: u16,
    pub code: ErrorCode,
    pub message: String,
    pub details: Option<Value>,
}

impl ForwardFailure {
    fn new(status: u16, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            details: None,
        }
    }
}

/// The forwarding engine: one transport per provider, the api keys read at
/// startup, and (optionally) the store and trace sink. `store: None` only
/// in tests without state.
pub struct Forwarder {
    pub config: RouterConfig,
    pub transports: HashMap<String, Arc<dyn ProviderTransport>>,
    /// Provider name → api key (from `api_key_env` at startup). The values
    /// exist only in outbound requests — never in events or traces
    /// (DESIGN §12.10.1's auth rule).
    pub api_keys: HashMap<String, String>,
    pub store: Option<Arc<dyn Store>>,
    /// The trace sink (R2-2f): one DecisionRecord per request. `None`
    /// only in tests without a trace dir.
    pub trace: Option<Arc<dyn router_core::TraceWriter>>,
    /// Inbound headers per request (the session key sources live there;
    /// the body's `prompt_cache_key` is checked at receive).
    pub session_ttl_us: i64,
}

fn now_us() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0)
}

/// Session key resolution (spec §4 key_sources), shared by both forwarding
/// paths so a streamed request is the same request: the body's
/// `prompt_cache_key` first, then the configured `header:<name>` sources,
/// in declaration order. One implementation — the buffered and streaming
/// paths cannot disagree about who is asking.
pub(crate) fn resolve_session_key(
    config: &RouterConfig,
    parsed: &Value,
    headers: &[(String, String)],
) -> Option<String> {
    parsed
        .get("prompt_cache_key")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            config.session.key_sources.iter().find_map(|src| {
                let name = src.strip_prefix("header:")?;
                headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(name))
                    .map(|(_, v)| v.clone())
            })
        })
}

/// The turn index for a session: `requests_seen` from the projection + 1,
/// 1 on the session's first request, 1 with no session (§12.10.5).
pub(crate) fn turn_index_for(store: &Option<Arc<dyn Store>>, session: Option<&str>) -> u32 {
    session
        .and_then(|s| {
            let q = store
                .as_ref()?
                .query(Query::SessionRequestsSeen { session_key: s });
            match q {
                Ok(QueryRow::Count(n)) if n > 0 => Some(n as u32 + 1),
                _ => Some(1),
            }
        })
        .unwrap_or(1)
}

/// Whether the session already has a binding row (the sticky-hit input).
pub(crate) fn session_sticky_hit(store: &Option<Arc<dyn Store>>, session: Option<&str>) -> bool {
    session.is_some()
        && matches!(
            store.as_ref().map(|s| {
                s.query(Query::SessionBinding {
                    session_key: session.unwrap_or(""),
                })
            }),
            Some(Ok(QueryRow::SessionBinding(Some(_))))
        )
}

/// The one shared outbound-body composition step (DESIGN §12.10.7): both
/// forwarding paths compose the upstream body through this function's two
/// mutations only — a hand-copied second composition is where the byte
/// boundary dies. Mutation (a) runs once per request in the callers; this
/// is mutation (b), per attempt: the value of the top-level `model`
/// member becomes `native_id`, every other byte the client's. The rewrite
/// is a pure function of (bytes, route), which is what makes an alias and
/// a direct route to the same route byte-identical upstream (CONF-27).
pub(crate) fn rewrite_outbound_model<'a>(
    cleaned: &'a RawBody,
    native_id: &str,
) -> Result<Cow<'a, [u8]>, RawEditError> {
    cleaned.set_top_level_string("model", native_id)
}

/// What the buffered path learned about a request before its terminal
/// outcome — enough to write the failure `DecisionRecord` when the
/// outcome is a failure (spec §6: one line per request, failures
/// included). `now_epoch_s` is the request's single clock read (AGENTS
/// constraint 2), reused by the terminal record instead of a second one.
/// Shared with the streaming path (R2G8): both paths' failures record
/// through the same facts shape.
pub(crate) struct RequestFacts<'a> {
    pub(crate) request_id: &'a str,
    pub(crate) received_event: Option<router_core::EventId>,
    pub(crate) proto_in: WireApi,
    pub(crate) proto_out: Option<WireApi>,
    pub(crate) session: Option<String>,
    pub(crate) turn_index: u32,
    pub(crate) requested_model: Option<String>,
    pub(crate) selection_source: &'static str,
    pub(crate) decision_ms: u32,
    pub(crate) started: Instant,
    pub(crate) now_epoch_s: u64,
    /// Prefix blocks of the cleaned body, set once mutation (a) ran.
    pub(crate) blocks: Vec<PrefixBlock>,
    /// The first route that failed over, for `result.failover_from`.
    pub(crate) failover_from: Option<RouteSpec>,
    /// The last attempt's wire latency (a single-attempt failure's
    /// `result.upstream_ms`); `None` until an attempt answered.
    pub(crate) upstream_ms: Option<u32>,
    /// The last route actually attempted (the failure record's
    /// `decision.provider`/`model` — "which provider died", R2G8).
    pub(crate) attempted_route: Option<RouteSpec>,
    /// The last upstream status that arrived, when one did (mirrored into
    /// `result.upstream_status` on the failure record, R2G8).
    pub(crate) last_upstream_status: Option<u16>,
}

impl Forwarder {
    /// Forwards one buffered request. `proto_in` is the inbound endpoint's
    /// protocol; a native route (proto_in == the provider's `wire_api`)
    /// forwards the client's bytes minus router-owned top-level keys,
    /// everything else byte-identical. `headers` feed the session key
    /// sources (`header:<name>`); the body's `prompt_cache_key` wins when
    /// present (spec §4 key_sources order).
    ///
    /// Every terminal failure records its `DecisionRecord` before the
    /// outcome leaves the engine (R2G4) — one shared call into
    /// `Accountant::finish_failure`, never a second inlined copy.
    pub async fn forward(
        &self,
        proto_in: WireApi,
        body: &[u8],
        request_id: &str,
        headers: &[(String, String)],
    ) -> ForwardOutcome {
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
            now_epoch_s: (now_us() / 1_000_000).max(0) as u64,
            blocks: Vec::new(),
            failover_from: None,
            upstream_ms: None,
            attempted_route: None,
            last_upstream_status: None,
        };
        let outcome = self
            .forward_inner(&mut facts, proto_in, body, request_id, headers)
            .await;
        if let ForwardOutcome::Failure(f) = &outcome {
            self.record_failure_trace(&facts, f);
        }
        outcome
    }

    /// One trace line for a terminal failure (spec §6 / §8): the same
    /// `Accountant` seam as the success path, so the failure record's
    /// shape cannot drift from it. Missing facts stay missing — an
    /// unparsable body has no `session` and no `blocks`, and the record
    /// says so by leaving them empty rather than inventing values.
    /// Shared by both forwarding paths (R2G8).
    pub(crate) fn record_failure_trace(&self, facts: &RequestFacts<'_>, f: &ForwardFailure) {
        crate::accounting::Accountant {
            store: self.store.as_deref(),
            trace: self.trace.as_deref(),
            // No usage arrived on a failure path: the price table would
            // price nothing (commit() skips everything at usage_missing),
            // so the lookup is deliberately not performed.
            accounting: None,
        }
        .finish_failure(
            &crate::accounting::AccountCtx {
                request_id: facts.request_id,
                received_event: facts.received_event,
                proto_in: facts.proto_in.as_str(),
                proto_out: facts.proto_out.map(WireApi::as_str),
                session: facts.session.as_deref(),
                turn_index: facts.turn_index,
                selection_source: facts.selection_source,
                requested_model: facts.requested_model.as_deref(),
                decision_ms: facts.decision_ms,
                started: facts.started,
                now_epoch_s: facts.now_epoch_s,
            },
            f,
            facts
                .attempted_route
                .as_ref()
                .map(|r| (r.provider.as_str(), r.model.as_str())),
            facts.last_upstream_status,
            &facts.blocks,
            facts.upstream_ms,
            facts
                .failover_from
                .as_ref()
                .map(|r| format!("{}/{}", r.provider, r.model)),
        );
    }

    async fn forward_inner(
        &self,
        facts: &mut RequestFacts<'_>,
        proto_in: WireApi,
        body: &[u8],
        request_id: &str,
        headers: &[(String, String)],
    ) -> ForwardOutcome {
        let started = facts.started;
        let now_epoch_s = facts.now_epoch_s;
        let decision_start = Instant::now();
        // Parse only to read `model` and `stream`; the forwarded bytes are
        // the raw original, never a reserialization.
        let parsed: Value = match serde_json::from_slice(body) {
            Ok(v) => v,
            Err(e) => {
                return ForwardOutcome::Failure(ForwardFailure::new(
                    400,
                    ErrorCode::InvalidRequest,
                    format!("request body is not parsable JSON: {e}"),
                ))
            }
        };
        let Some(model) = parsed.get("model").and_then(|m| m.as_str()) else {
            return ForwardOutcome::Failure(ForwardFailure::new(
                400,
                ErrorCode::InvalidRequest,
                "request body is missing the string field 'model'",
            ));
        };
        facts.requested_model = Some(model.to_string());
        if model == "auto" {
            return ForwardOutcome::Failure(ForwardFailure::new(
                400,
                ErrorCode::AutoNotSupported,
                "model 'auto' is not supported in v0.1 (a plugin takes it over; the slot is reserved)",
            ));
        }
        if parsed.get("stream").and_then(|s| s.as_bool()) == Some(true) {
            // SSE lands in R2-2e; never half-relay a stream buffered.
            return ForwardOutcome::Failure(ForwardFailure::new(
                501,
                ErrorCode::NotImplemented,
                "streaming responses land in R2-2e; resend with stream: false",
            ));
        }

        // Route resolution (spec §3).
        let (primary, selection_source) = match self.resolve_route(model) {
            Ok(r) => r,
            Err(f) => return ForwardOutcome::Failure(f),
        };
        facts.selection_source = selection_source;
        let Some(provider) = self.provider(&primary) else {
            return ForwardOutcome::Failure(ForwardFailure::new(
                404,
                ErrorCode::UnknownProvider,
                format!("unknown provider '{}'", primary.provider),
            ));
        };
        // Capability: the undeclared cell is a 400, never a best-effort
        // translation (spec §8); the declared-but-different cell is a
        // translation, which lands in R2-3.
        if !provider.supports.contains(&proto_in) {
            let supports: Vec<&str> = provider.supports.iter().map(|w| w.as_str()).collect();
            return ForwardOutcome::Failure(ForwardFailure {
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
            return ForwardOutcome::Failure(ForwardFailure::new(
                501,
                ErrorCode::NotImplemented,
                format!(
                    "translation {} -> {} lands in R2-3; this round forwards native routes only",
                    proto_in, provider.wire_api
                ),
            ));
        }

        // The outbound base (DESIGN §12.10.7): the client's bytes minus
        // router-owned top-level keys — mutation (a), once per request. The
        // `model` rewrite (mutation (b)) is per attempt, below, because the
        // fallback chain walks routes whose native ids differ (§12.10.5
        // note R4: this base is the router-visible inbound the row-1 hash
        // names, and it does not depend on the route taken).
        let cleaned =
            match RawBody::new(body.to_vec()).remove_top_level_keys(ROUTER_OWNED_TOP_LEVEL_KEYS) {
                Ok(b) => b,
                Err(e) => {
                    return ForwardOutcome::Failure(ForwardFailure::new(
                        400,
                        ErrorCode::InvalidRequest,
                        format!("request body is not a well-formed top-level JSON object: {e:?}"),
                    ))
                }
            };
        let outbound_hash = body_sha16(cleaned.as_bytes());
        facts.blocks = extract_prefix_blocks(&cleaned).unwrap_or_default();

        // Session resolution (spec §4 key_sources) — the shared helper, so
        // the buffered and streaming paths resolve the same client the
        // same way.
        let session = resolve_session_key(&self.config, &parsed, headers);
        let turn_index = turn_index_for(&self.store, session.as_deref());
        facts.session = session.clone();
        facts.turn_index = turn_index;

        // Rows 1 + 3 of the §12.10.5 wiring (FULL + NORMAL).
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
            }),
            session.as_deref(),
        );
        // Row 2 — the transform chain point is kept even when the chain is
        // empty (this round's chain is passthrough; the wiring point lands
        // with the transform chain itself, R2-3).
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

        // Row 4 — session.bound (FULL) when the binding is created or
        // moved; a sticky hit on an unchanged route writes nothing. The
        // `sessions` projection rides on the event row (requests_seen
        // drives the next turn's `turn_index` — a missed projection write
        // would freeze the counter, so it goes through the accountant's
        // bind_session, the one writer).
        let sticky_hit = session_sticky_hit(&self.store, session.as_deref());
        if session.is_some() {
            crate::accounting::Accountant {
                store: self.store.as_deref(),
                trace: self.trace.as_deref(),
                accounting: None,
            }
            .bind_session(
                &crate::accounting::AccountCtx {
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
                },
                &primary.provider,
                &primary.model,
                sticky_hit,
                false,
                self.session_ttl_us,
            );
        }

        // The candidate chain: the primary route, then the global fallback
        // list (spec §4.2), walked once. Provider-exclusion semantics: a
        // failed provider is excluded whole — its other routes are not
        // attempted either (ADR-011 item 4's in-request form).
        let mut candidates: Vec<RouteSpec> = vec![primary.clone()];
        for r in &self.config.fallback {
            if !candidates.contains(r) {
                candidates.push(r.clone());
            }
        }

        let mut attempted_providers: Vec<String> = Vec::new();
        let mut last_class: Option<ErrorClass> = None;
        let mut last_upstream_status: Option<u16> = None;
        let mut attempt_index: u32 = 0;

        for candidate in &candidates {
            if attempted_providers.iter().any(|p| p == &candidate.provider)
                || self.provider_in_cooldown(&candidate.provider)
            {
                continue;
            }
            let Some(cand_provider) = self.provider(candidate).cloned() else {
                continue;
            };
            let Some(transport) = self.transports.get(&candidate.provider) else {
                // No transport at startup (missing api key): the provider
                // is unavailable (§12.10.2), reported by /health; the
                // chain continues.
                attempted_providers.push(candidate.provider.clone());
                continue;
            };
            attempted_providers.push(candidate.provider.clone());

            let api_key = self
                .api_keys
                .get(&candidate.provider)
                .cloned()
                .unwrap_or_default();
            let plan = UpstreamPlan {
                provider: &candidate.provider,
                model: &candidate.model,
                protocol_out: cand_provider.wire_api,
                base_url: &cand_provider.base_url,
                api_key: &api_key,
                attempt: attempt_index,
            };

            // Mutation (b), per attempt (DESIGN §12.10.7): the value of the
            // top-level `model` member becomes this route's native id, every
            // other byte the client's. `set_top_level_string` cannot fail
            // here — `model` was parsed as a string above, and mutation (a)
            // never removes it — but a failure is still answered, never
            // ignored, so no unrewritten bytes can reach a provider.
            let rewritten = match rewrite_outbound_model(&cleaned, &candidate.model) {
                Ok(b) => b,
                Err(e) => {
                    return ForwardOutcome::Failure(ForwardFailure::new(
                        500,
                        ErrorCode::Internal,
                        format!("cannot rewrite the outbound model to the native id: {e:?}"),
                    ))
                }
            };
            let attempt_hash = body_sha16(rewritten.as_ref());

            // Row 5 — the intent, FULL, committed before the wire. Nothing
            // is written between the commit and the attempt (CONF-20). The
            // payload carries the session's `prefix_blocks` — the encoder
            // computed them from exactly these bytes, which is what the
            // cache_ledger rebuild reads (§12.10.6, CONF-21). `body_hash`
            // is that attempt's byte-final bytes (note R4): the rewrite is
            // done, so the hash names exactly what goes to the wire.
            // Cloned (not taken): a failed attempt continues to the next
            // candidate, which re-reads the same cleaned-body blocks.
            let blocks_now = facts.blocks.clone();
            let intent_id = if let Some(store) = &self.store {
                let intent = NewEvent {
                    kind: EventKind::UpstreamSubmitted,
                    request_id: Some(request_id),
                    session: session.as_deref(),
                    body_hash: Some(&attempt_hash),
                    trace_ref: None,
                    payload: json!({
                        "route": format!("{}/{}", candidate.provider, candidate.model),
                        "attempt_index": attempt_index,
                        "protocol_out": cand_provider.wire_api.as_str(),
                        "prefix_blocks": blocks_now.iter().map(|b| json!({
                            "index": b.index, "kind": b.kind.as_str(),
                            "tokens": b.tokens, "hash": b.hash,
                        })).collect::<Vec<_>>(),
                    }),
                };
                match store.append(intent) {
                    Ok(id) => Some(id),
                    Err(_) => {
                        // CONF-22: nothing reached the upstream; the
                        // client's retry is safe.
                        return ForwardOutcome::Failure(ForwardFailure {
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

            let req = match router_providers::build_request(&plan, rewritten.as_ref()) {
                Ok(r) => r,
                Err(e) => {
                    return ForwardOutcome::Failure(ForwardFailure::new(
                        500,
                        ErrorCode::Internal,
                        format!("cannot build the outbound request: {e}"),
                    ))
                }
            };
            let attempt_started = Instant::now();
            let outcome = transport.send_boxed(req).await;
            let latency_us = attempt_started.elapsed().as_micros() as i64;
            let upstream_ms = Some((latency_us / 1000) as u32);
            facts.upstream_ms = upstream_ms;
            facts.attempted_route = Some(candidate.clone());

            match outcome {
                AttemptOutcome::Responded(resp) => {
                    last_upstream_status = Some(resp.status);
                    facts.last_upstream_status = Some(resp.status);
                    // Row 6 — the outcome, FULL. `wrote_full_request` is
                    // true by construction on the answered path.
                    let usage = normalize_usage(cand_provider.wire_api, &resp.body);
                    self.append_event(
                        EventKind::UpstreamResponded,
                        request_id,
                        None,
                        json!({
                            "status": resp.status,
                            "route": format!("{}/{}", candidate.provider, candidate.model),
                            "attempt_index": attempt_index,
                            "latency_us": latency_us,
                            "wrote_full_request": true,
                            "usage": usage.map(|u| usage_json(&u)),
                        }),
                        session.as_deref(),
                    );
                    if (200..300).contains(&resp.status) {
                        let mut blocks = blocks_now;
                        if let Some(u) = &usage {
                            attribute_tokens(&mut blocks, u);
                        }
                        facts.blocks = blocks.clone();
                        let route_acc =
                            crate::accounting::route_accounting(&self.config, candidate);
                        let accountant = crate::accounting::Accountant {
                            store: self.store.as_deref(),
                            trace: self.trace.as_deref(),
                            accounting: route_acc.as_ref(),
                        };
                        // Note R2's order: trace line, then cost/quota rows.
                        // The continuity measurement inside `finish` reads
                        // the session's PREVIOUS block set, so the ledger
                        // put (replacing it with this request's blocks for
                        // the NEXT request) must run after it — measure,
                        // then replace (§12.10.6).
                        let ctx = crate::accounting::AccountCtx {
                            request_id,
                            received_event: facts.received_event,
                            proto_in: proto_in.as_str(),
                            proto_out: Some(cand_provider.wire_api.as_str()),
                            session: session.as_deref(),
                            turn_index,
                            selection_source,
                            requested_model: Some(model),
                            decision_ms,
                            started,
                            now_epoch_s,
                        };
                        let _accounted = accountant.finish(
                            &ctx,
                            &ForwardSuccess {
                                status: resp.status,
                                content_type: resp.content_type.clone(),
                                body: resp.body.clone(),
                                route: candidate.clone(),
                                failover_from: facts.failover_from.clone(),
                                usage,
                                prefix_blocks: blocks.clone(),
                            },
                            &blocks,
                            upstream_ms,
                        );
                        if let (Some(id), Some(s)) = (intent_id, session.as_deref()) {
                            accountant.put_ledger(Some(s), &blocks, id);
                        }
                        return ForwardOutcome::Success(ForwardSuccess {
                            status: resp.status,
                            content_type: resp.content_type,
                            body: resp.body,
                            route: candidate.clone(),
                            failover_from: facts.failover_from.clone(),
                            usage,
                            prefix_blocks: blocks,
                        });
                    }
                    // A failure answered: classify → error.classified → act.
                    let evidence = ErrorEvidence {
                        status: Some(resp.status),
                        retry_after: resp.retry_after.as_deref(),
                        body: &resp.body,
                        wrote_full_request: true,
                        transport_cause: None,
                    };
                    let cls = classify_upstream_error(&evidence);
                    last_class = Some(cls.class);
                    let classified_id = self.record_classification(
                        request_id,
                        attempt_index,
                        Some(resp.status),
                        &cls,
                    );
                    // The demotion projection (state, not a local var),
                    // riding on the classification event (ADR-011 item 4).
                    self.apply_demotion(&candidate.provider, &cls, classified_id);
                    if let Some(next) =
                        self.next_candidate(&candidates, &attempted_providers, cls.class)
                    {
                        {
                            let (reprefill, switch_cost) =
                                self.failover_cost(session.as_deref(), &next);
                            self.record_failover(
                                request_id,
                                attempt_index,
                                candidate,
                                &next,
                                &cls,
                                session.as_deref(),
                                reprefill,
                                switch_cost,
                            );
                        }
                        failover_origin(&mut facts.failover_from, candidate);
                        attempt_index += 1;
                        continue;
                    }
                    return ForwardOutcome::Failure(
                        self.exhausted_failure(last_upstream_status, cls.class),
                    );
                }
                AttemptOutcome::NotSent(err) => {
                    // No request bytes went out: nothing billed, failover
                    // allowed (ADR-011 item 6 row 1). No upstream.responded
                    // row — the attempt never reached the upstream. The
                    // transport kind the provider layer derived from the
                    // reqwest error (`is_connect()` / `is_timeout()`) is
                    // the classification evidence: a connect failure is
                    // `connect_failure` and walks the chain, a connect
                    // timeout keeps the `timeout` verdict (R2G5).
                    let evidence = ErrorEvidence {
                        status: None,
                        retry_after: None,
                        body: b"",
                        wrote_full_request: false,
                        transport_cause: Some(match err.kind {
                            TransportKind::Connect => TransportCause::Connect,
                            TransportKind::Timeout => TransportCause::Timeout,
                            TransportKind::Other => TransportCause::Other,
                        }),
                    };
                    let cls = classify_upstream_error(&evidence);
                    last_class = Some(cls.class);
                    let _classified_id =
                        self.record_classification(request_id, attempt_index, None, &cls);
                    if let Some(next) =
                        self.next_candidate(&candidates, &attempted_providers, cls.class)
                    {
                        {
                            let (reprefill, switch_cost) =
                                self.failover_cost(session.as_deref(), &next);
                            self.record_failover(
                                request_id,
                                attempt_index,
                                candidate,
                                &next,
                                &cls,
                                session.as_deref(),
                                reprefill,
                                switch_cost,
                            );
                        }
                        failover_origin(&mut facts.failover_from, candidate);
                        attempt_index += 1;
                        continue;
                    }
                    return ForwardOutcome::Failure(ForwardFailure {
                        status: 502,
                        code: ErrorCode::UpstreamError,
                        message: format!(
                            "upstream connect failed and the fallback chain is exhausted: {}",
                            err.message
                        ),
                        details: Some(json!({
                            "upstream_status": Value::Null,
                            "error_class": cls.class.as_str(),
                        })),
                    });
                }
                AttemptOutcome::WrittenNoResponse(err) => {
                    // ADR-011 item 6 row 3 / ADR-010 item 4: the attempt
                    // may already have been billed. No retry, no failover,
                    // and deliberately **no closure event** — the unpaired
                    // intent is the `unknown_outcome` reconciliation finds.
                    let timed_out = err.kind == TransportKind::Timeout;
                    return ForwardOutcome::Failure(ForwardFailure {
                        status: if timed_out { 504 } else { 502 },
                        code: if timed_out {
                            ErrorCode::UpstreamTimeout
                        } else {
                            ErrorCode::UpstreamError
                        },
                        message: format!(
                            "upstream attempt written without a response (unknown_outcome): {}",
                            err.message
                        ),
                        details: Some(json!({
                            "stage": "unknown_outcome",
                            "error_class": "timeout",
                        })),
                    });
                }
            }
        }

        // Every candidate was skipped (demoted providers, missing keys).
        ForwardOutcome::Failure(ForwardFailure {
            status: 502,
            code: ErrorCode::UpstreamError,
            message:
                "no available route: every candidate provider is demoted, keyless or unavailable"
                    .into(),
            details: Some(json!({
                "upstream_status": last_upstream_status,
                "error_class": last_class.map(|c| c.as_str()),
            })),
        })
    }

    // -- helpers -----------------------------------------------------------

    fn provider(&self, route: &RouteSpec) -> Option<&ProviderCfg> {
        self.config
            .providers
            .iter()
            .find(|p| p.name == route.provider)
    }

    fn resolve_route(&self, model: &str) -> Result<(RouteSpec, &'static str), ForwardFailure> {
        if let Some(route) = self.config.aliases.get(model) {
            return Ok((route.clone(), "alias"));
        }
        if let Some((provider, m)) = model.split_once('/') {
            match self.config.providers.iter().find(|p| p.name == provider) {
                None => Err(ForwardFailure::new(
                    404,
                    ErrorCode::UnknownProvider,
                    format!("unknown provider '{provider}'"),
                )),
                Some(p) => {
                    if p.models.iter().any(|mm| mm.id == m) {
                        Ok((
                            RouteSpec {
                                provider: provider.to_string(),
                                model: m.to_string(),
                            },
                            "explicit",
                        ))
                    } else {
                        Err(ForwardFailure::new(
                            404,
                            ErrorCode::UnknownModel,
                            format!("unknown model '{m}' on provider '{provider}'"),
                        ))
                    }
                }
            }
        } else {
            Err(ForwardFailure::new(
                404,
                ErrorCode::UnknownModel,
                format!("unknown model or alias '{model}'"),
            ))
        }
    }

    pub(crate) fn provider_in_cooldown(&self, provider: &str) -> bool {
        let Some(store) = &self.store else {
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

    /// `error.classified` (NORMAL) — §12.10.5 row 7, ADR-011 item 8. Every
    /// classification writes one event, even when the action is abort.
    /// Returns the event id the demotion projection rides on. Shared by
    /// both forwarding paths (R2G8): the same failure gets the same row
    /// whichever medium carried it.
    pub(crate) fn record_classification(
        &self,
        request_id: &str,
        attempt_index: u32,
        status: Option<u16>,
        cls: &Classification,
    ) -> Option<router_core::EventId> {
        let store = self.store.as_ref()?;
        let action = if cls.class.fails_over() {
            // ADR-011 item 7: v0.1 has one key per provider, so rotation is
            // a no-op slot that falls through to the fallback provider.
            "fallback_provider"
        } else {
            "abort"
        };
        let demotion = cls.demotion.map(|d| {
            json!({
                "scope": if d.provider_wide { "provider" } else { "route" },
                "ttl_s": d.ttl.as_secs(),
            })
        });
        store
            .append(NewEvent {
                kind: EventKind::ErrorClassified,
                request_id: Some(request_id),
                session: None,
                body_hash: None,
                trace_ref: None,
                payload: json!({
                    "attempt_index": attempt_index,
                    "status": status,
                    "reason": cls.class.as_str(),
                    "action": action,
                    "matched": cls.matched,
                    "retry_after_s": cls.retry_after.map(|d| d.as_secs()),
                    "demotion": demotion,
                }),
            })
            .ok()
    }

    /// The next unattempted candidate after a failure, when the class may
    /// fail over. Provider-exclusion: a provider already attempted in this
    /// request is never re-attempted (spec §4.2 + ADR-011 item 4).
    fn next_candidate(
        &self,
        candidates: &[RouteSpec],
        attempted_providers: &[String],
        class: ErrorClass,
    ) -> Option<RouteSpec> {
        if !class.fails_over() {
            return None;
        }
        candidates
            .iter()
            .find(|c| {
                !attempted_providers.contains(&c.provider)
                    && !self.provider_in_cooldown(&c.provider)
                    && self.transports.contains_key(&c.provider)
            })
            .cloned()
    }

    pub(crate) fn append_event(
        &self,
        kind: EventKind,
        request_id: &str,
        body_hash: Option<&str>,
        payload: Value,
        session: Option<&str>,
    ) -> Option<router_core::EventId> {
        let store = self.store.as_ref()?;
        store
            .append(NewEvent {
                kind,
                request_id: Some(request_id),
                session,
                body_hash,
                trace_ref: None,
                payload,
            })
            .ok()
    }

    /// A demotion is state, not a local variable (ADR-011 item 4): the
    /// `provider_cooldown` projection, provider-wide, TTL from the
    /// provider's own clock or the declared default, anchored on the
    /// classification event.
    pub(crate) fn apply_demotion(
        &self,
        provider: &str,
        cls: &Classification,
        anchor: Option<router_core::EventId>,
    ) {
        let Some(store) = &self.store else {
            return;
        };
        let Some(d) = cls.demotion else {
            return;
        };
        let _ = store.project(ProjectionWrite::ProviderCooldown {
            scope: "provider",
            provider,
            model: "",
            until_us: now_us() + d.ttl.as_micros() as i64,
            reason: cls.class.as_str(),
            last_event: anchor.unwrap_or(router_core::EventId(0)),
        });
    }

    /// `failover.triggered` (FULL), before the next intent (§12.10.5 row 8).
    /// Shared by both forwarding paths (R2G8).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_failover(
        &self,
        request_id: &str,
        attempt_index: u32,
        from: &RouteSpec,
        to: &RouteSpec,
        cls: &Classification,
        session: Option<&str>,
        reprefill: Option<u64>,
        switch_cost_nano: Option<u64>,
    ) {
        self.append_event(
            EventKind::FailoverTriggered,
            request_id,
            None,
            json!({
                "attempt_index": attempt_index,
                "reason": cls.class.as_str(),
                "from": format!("{}/{}", from.provider, from.model),
                "to": format!("{}/{}", to.provider, to.model),
                // ADR-011 item 9: reprefill_tokens is the session's prefix
                // token total from the ledger (an `inferred` figure,
                // GAP-Q14) and switch_cost_nano prices it at the new
                // route's input_miss — both null when no ledger exists.
                "reprefill_tokens": reprefill,
                "switch_cost_nano": switch_cost_nano,
            }),
            session,
        );
    }

    /// The session's prefix token total from the `cache_ledger` projection
    /// (ADR-011 item 9's `reprefill_tokens` source — `inferred`, GAP-Q14),
    /// priced at `to`'s input_miss for `switch_cost_nano`.
    pub(crate) fn failover_cost(
        &self,
        session: Option<&str>,
        to: &RouteSpec,
    ) -> (Option<u64>, Option<u64>) {
        let Some(session) = session else {
            return (None, None);
        };
        let Some(store) = &self.store else {
            return (None, None);
        };
        let Ok(QueryRow::CacheLedger(blocks)) = store.query(Query::CacheLedgerBlocks {
            session_key: session,
        }) else {
            return (None, None);
        };
        if blocks.is_empty() {
            return (None, None);
        }
        let tokens: u64 = blocks.iter().map(|b| b.tokens).sum();
        let cost_nano = crate::accounting::route_accounting(&self.config, to).map(|acc| {
            // tokens × price(USD/1K) → NanoUsd, floored (§12.4 discipline).
            let v = tokens as u128 * acc.price.input_miss.0 as u128 / 1000;
            if v > u64::MAX as u128 {
                u64::MAX
            } else {
                v as u64
            }
        });
        (Some(tokens), cost_nano)
    }

    fn exhausted_failure(&self, upstream_status: Option<u16>, class: ErrorClass) -> ForwardFailure {
        let deterministic = matches!(
            class,
            ErrorClass::FormatError | ErrorClass::ContentPolicyBlocked
        );
        ForwardFailure {
            status: 502,
            code: ErrorCode::UpstreamError,
            message: if deterministic {
                format!(
                    "upstream rejected the request deterministically ({}); it is not retried",
                    class.as_str()
                )
            } else {
                format!(
                    "upstream error ({}) and the fallback chain is exhausted",
                    class.as_str()
                )
            },
            details: Some(json!({
                "upstream_status": upstream_status,
                "error_class": class.as_str(),
            })),
        }
    }
}

/// The first route the request failed over from (spec §6
/// `result.failover_from`): set once, never overwritten by later hops.
fn failover_origin(slot: &mut Option<RouteSpec>, failed: &RouteSpec) {
    if slot.is_none() {
        *slot = Some(failed.clone());
    }
}

fn usage_json(u: &Usage) -> Value {
    json!({
        "input_total": u.input_total,
        "input_cached": u.input_cached,
        "cache_write": u.cache_write,
        "output": u.output,
        "reasoning": u.reasoning,
    })
}

fn normalize_usage(wire: WireApi, body: &[u8]) -> Option<Usage> {
    let v: Value = serde_json::from_slice(body).ok()?;
    match wire {
        WireApi::Chat => router_protocol::usage_from_chat(&v),
        WireApi::Responses => router_protocol::usage_from_responses(&v),
        WireApi::Anthropic => router_protocol::usage_from_anthropic(&v),
    }
}
