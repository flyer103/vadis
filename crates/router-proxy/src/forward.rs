//! The buffered forwarding engine (R2-2d): native passthrough with byte
//! fidelity, usage normalization, ADR-011 error classification, the
//! fallback chain with provider-level exclusion, and the §12.10.5 event
//! sequence (`upstream.submitted` → `upstream.responded` →
//! `error.classified` → the action's effect).
//!
//! Non-streaming only: a `stream: true` inbound body is refused with 501
//! by this engine before any attempt runs (SSE lands in R2-2e), so an
//! attempt never half-relays.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use http::Request as HttpRequest;
use router_core::config::{ProviderCfg, RouteSpec, RouterConfig, WireApi};
use router_core::error::ErrorCode;
use router_core::error_class::{classify_upstream_error, Classification, ErrorClass, ErrorEvidence};
use router_core::prefix::{attribute_tokens, body_sha16, extract_prefix_blocks, PrefixBlock};
use router_core::store::{EventKind, NewEvent, ProjectionWrite, Query, QueryRow, Store};
use router_core::{RawBody, Usage, ROUTER_OWNED_TOP_LEVEL_KEYS};
use router_providers::{AttemptOutcome, TransportKind, UpstreamPlan};
use serde_json::{json, Value};

/// The async seam the engine is monomorphized over: the real reqwest client
/// and the test double both satisfy it (DESIGN §12.10.1: no `async-trait`).
pub trait ProviderSend {
    fn send(&self, req: HttpRequest<Bytes>) -> impl Future<Output = AttemptOutcome> + Send;
}

/// Object-safe wrapper so `Forwarder` can hold one transport per provider
/// without making the whole serving state generic.
pub type BoxedAttempt = Pin<Box<dyn Future<Output = AttemptOutcome> + Send>>;

pub trait ProviderTransport: Send + Sync {
    fn send_boxed(&self, req: HttpRequest<Bytes>) -> BoxedAttempt;
}

impl<T> ProviderTransport for T
where
    T: ProviderSend + Send + Sync + 'static,
{
    fn send_boxed(&self, req: HttpRequest<Bytes>) -> BoxedAttempt {
        Box::pin(self.send(req))
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
/// startup, and (optionally) the store. `store: None` only in tests
/// without state.
pub struct Forwarder {
    pub config: RouterConfig,
    pub transports: HashMap<String, Arc<dyn ProviderTransport>>,
    /// Provider name → api key (from `api_key_env` at startup). The values
    /// exist only in outbound requests — never in events or traces
    /// (DESIGN §12.10.1's auth rule).
    pub api_keys: HashMap<String, String>,
    pub store: Option<Arc<dyn Store>>,
}

fn now_us() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0)
}

impl Forwarder {
    /// Forwards one buffered request. `proto_in` is the inbound endpoint's
    /// protocol; a native route (proto_in == the provider's `wire_api`)
    /// forwards the client's bytes minus router-owned top-level keys,
    /// everything else byte-identical.
    pub async fn forward(&self, proto_in: WireApi, body: &[u8], request_id: &str) -> ForwardOutcome {
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

        // The outbound body: the client's bytes minus router-owned
        // top-level keys — the only permitted rewrite (AGENTS constraint 1).
        let raw = RawBody::new(body.to_vec());
        let outbound = match raw.remove_top_level_keys(ROUTER_OWNED_TOP_LEVEL_KEYS) {
            Ok(b) => b,
            Err(e) => {
                return ForwardOutcome::Failure(ForwardFailure::new(
                    400,
                    ErrorCode::InvalidRequest,
                    format!("request body is not a well-formed top-level JSON object: {e:?}"),
                ))
            }
        };
        let outbound_hash = body_sha16(outbound.as_bytes());

        // Rows 1 + 3 of the §12.10.5 wiring (FULL + NORMAL).
        self.append_event(
            EventKind::RequestReceived,
            request_id,
            Some(&outbound_hash),
            json!({
                "protocol_in": proto_in.as_str(),
                "protocol_out": null,
                "client": null,
                "session": null,
                "turn_index": 1,
                "body_hash": outbound_hash,
            }),
        );
        self.append_event(
            EventKind::DecisionMade,
            request_id,
            None,
            json!({
                "provider": primary.provider,
                "model": primary.model,
                "selection_source": selection_source,
                "protocol_out": provider.wire_api.as_str(),
            }),
        );

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
        let mut failover_from: Option<RouteSpec> = None;
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

            // Row 5 — the intent, FULL, committed before the wire. Nothing
            // is written between the commit and the attempt (CONF-20).
            if let Some(store) = &self.store {
                let intent = NewEvent {
                    kind: EventKind::UpstreamSubmitted,
                    request_id: Some(request_id),
                    session: None,
                    body_hash: Some(&outbound_hash),
                    trace_ref: None,
                    payload: json!({
                        "route": format!("{}/{}", candidate.provider, candidate.model),
                        "attempt_index": attempt_index,
                        "protocol_out": cand_provider.wire_api.as_str(),
                    }),
                };
                if store.append(intent).is_err() {
                    // CONF-22: nothing reached the upstream; the client's
                    // retry is safe.
                    return ForwardOutcome::Failure(ForwardFailure {
                        status: 500,
                        code: ErrorCode::Internal,
                        message: "the state store rejected the upstream intent".into(),
                        details: Some(json!({"stage": "intent"})),
                    });
                }
            }

            let req = match router_providers::build_request(&plan, outbound.as_bytes()) {
                Ok(r) => r,
                Err(e) => {
                    return ForwardOutcome::Failure(ForwardFailure::new(
                        500,
                        ErrorCode::Internal,
                        format!("cannot build the outbound request: {e}"),
                    ))
                }
            };
            let started = Instant::now();
            let outcome = transport.send_boxed(req).await;
            let latency_us = started.elapsed().as_micros() as i64;

            match outcome {
                AttemptOutcome::Responded(resp) => {
                    last_upstream_status = Some(resp.status);
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
                    );
                    if (200..300).contains(&resp.status) {
                        let mut blocks = extract_prefix_blocks(&outbound).unwrap_or_default();
                        if let Some(u) = &usage {
                            attribute_tokens(&mut blocks, u);
                        }
                        return ForwardOutcome::Success(ForwardSuccess {
                            status: resp.status,
                            content_type: resp.content_type,
                            body: resp.body,
                            route: candidate.clone(),
                            failover_from,
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
                        self.record_failover(request_id, attempt_index, candidate, &next, &cls);
                        failover_from.get_or_insert_with(|| candidate.clone());
                        attempt_index += 1;
                        continue;
                    }
                    return ForwardOutcome::Failure(self.exhausted_failure(
                        last_upstream_status,
                        cls.class,
                    ));
                }
                AttemptOutcome::NotSent(err) => {
                    // No request bytes went out: nothing billed, failover
                    // allowed (ADR-011 item 6 row 1). No upstream.responded
                    // row — the attempt never reached the upstream.
                    let evidence = ErrorEvidence {
                        status: None,
                        retry_after: None,
                        body: b"",
                        wrote_full_request: false,
                    };
                    let cls = classify_upstream_error(&evidence);
                    last_class = Some(cls.class);
                    let _classified_id =
                        self.record_classification(request_id, attempt_index, None, &cls);
                    if let Some(next) =
                        self.next_candidate(&candidates, &attempted_providers, cls.class)
                    {
                        self.record_failover(request_id, attempt_index, candidate, &next, &cls);
                        failover_from.get_or_insert_with(|| candidate.clone());
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

    fn provider_in_cooldown(&self, provider: &str) -> bool {
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

    fn append_event(
        &self,
        kind: EventKind,
        request_id: &str,
        body_hash: Option<&str>,
        payload: Value,
    ) {
        if let Some(store) = &self.store {
            let _ = store.append(NewEvent {
                kind,
                request_id: Some(request_id),
                session: None,
                body_hash,
                trace_ref: None,
                payload,
            });
        }
    }

    /// `error.classified` (NORMAL) — §12.10.5 row 7, ADR-011 item 8. Every
    /// classification writes one event, even when the action is abort.
    /// Returns the event id the demotion projection rides on.
    fn record_classification(
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

    /// A demotion is state, not a local variable (ADR-011 item 4): the
    /// `provider_cooldown` projection, provider-wide, TTL from the
    /// provider's own clock or the declared default, anchored on the
    /// classification event.
    fn apply_demotion(
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
    fn record_failover(
        &self,
        request_id: &str,
        attempt_index: u32,
        from: &RouteSpec,
        to: &RouteSpec,
        cls: &Classification,
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
                // ADR-011 item 9: reprefill/switch-cost come from the
                // session's cache ledger; no ledger exists on this path
                // yet, so the fields are honestly absent rather than 0.
                "reprefill_tokens": Value::Null,
                "switch_cost_nano": Value::Null,
            }),
        );
    }

    fn exhausted_failure(&self, upstream_status: Option<u16>, class: ErrorClass) -> ForwardFailure {
        let deterministic =
            matches!(class, ErrorClass::FormatError | ErrorClass::ContentPolicyBlocked);
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
