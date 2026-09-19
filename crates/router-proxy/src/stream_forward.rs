//! The streaming forwarding engine (R2-2e): byte-faithful SSE
//! passthrough per DESIGN §12.10.3 R1–R11.
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
//! The zero-byte branch of R6: a mid-stream failure with **no byte relayed
//! yet** may fail over (the client has observed only the 200 SSE head, no
//! event), walking the same fallback chain as the buffered path with the
//! same provider-exclusion and cooldown state. Once any byte is relayed,
//! no retry of any kind happens.
//!
//! Pre-flight (parse → route → capability → outbound bytes) reuses the
//! buffered path's rules so the streaming request is the same request
//! (R11): same intents, same event vocabulary, same demotion state.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::stream::{unfold, Stream};
use router_core::config::{ProviderCfg, RouteSpec, WireApi};
use router_core::error::ErrorCode;
use router_core::error_class::{classify_upstream_error, ErrorEvidence};
use router_core::store::{EventKind, NewEvent, Query, QueryRow, Store};
use router_protocol::sse::{SseUsageExtractor, SseUsageOutcome};
use serde_json::{json, Value};

use crate::forward::{ForwardFailure, Forwarder};

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
struct RelayCtx {
    store: Option<Arc<dyn Store>>,
    idle: Duration,
    request_id: String,
    /// chat only: the client asked for `stream_options.include_usage`.
    client_requested_usage: bool,
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
    /// The first route that failed pre-relay, for `failover_from`.
    failover_from: Option<RouteSpec>,
}

impl Forwarder {
    /// Forwards one streaming request. On success the caller receives the
    /// head plus the byte relay; the relay owns the single open upstream
    /// connection and closes it when dropped (R5).
    pub async fn forward_stream(
        &self,
        proto_in: WireApi,
        body: &[u8],
        request_id: &str,
    ) -> StreamOutcome {
        // Pre-flight: the same parse → route → capability → outbound
        // bytes rules as the buffered path (R11).
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
        if model == "auto" {
            return StreamOutcome::Failure(ForwardFailure {
                status: 400,
                code: ErrorCode::AutoNotSupported,
                message: "model 'auto' is not supported in v0.1 (a plugin takes it over; the slot is reserved)"
                    .into(),
                details: None,
            });
        }
        let (primary, selection_source) = match resolve_route(&self.config, model) {
            Ok(r) => r,
            Err(f) => return StreamOutcome::Failure(f),
        };
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
                    "translation {} -> {} lands in R2-3; this round forwards native routes only",
                    proto_in, provider.wire_api
                ),
                details: None,
            });
        }

        // The outbound body: the client's bytes minus router-owned
        // top-level keys — the only permitted rewrite (AGENTS constraint 1).
        let raw = router_core::RawBody::new(body.to_vec());
        let outbound = match raw.remove_top_level_keys(router_core::ROUTER_OWNED_TOP_LEVEL_KEYS) {
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
        let outbound_hash = router_core::prefix::body_sha16(outbound.as_bytes());
        let outbound_bytes = Bytes::from(outbound.as_bytes().to_vec());

        self.stream_event(
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
                "stream": true,
            }),
        );
        self.stream_event(
            EventKind::DecisionMade,
            request_id,
            None,
            json!({
                "provider": primary.provider,
                "model": primary.model,
                "selection_source": selection_source,
                "protocol_out": provider.wire_api.as_str(),
                "stream": true,
            }),
        );

        // The primary candidate plus the fallback chain (spec §4.2),
        // filtered to native routes with a key present at startup.
        let mut candidates: Vec<Candidate> = Vec::new();
        for route in std::iter::once(primary.clone()).chain(self.config.fallback.iter().cloned()) {
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
        let Some(first) = candidates.first().cloned() else {
            return StreamOutcome::Failure(ForwardFailure {
                status: 502,
                code: ErrorCode::UpstreamError,
                message: "no available route: the primary provider is keyless or unavailable"
                    .into(),
                details: Some(json!({"stream": true})),
            });
        };
        let remaining: Vec<Candidate> = candidates.into_iter().skip(1).collect();

        let idle = Duration::from_millis(self.config.server.upstream_attempt_timeout.0);
        let route_label = format!("{}/{}", first.route.provider, first.route.model);

        // Row 5 — the intent, FULL, committed before the wire (CONF-20).
        let intent = NewEvent {
            kind: EventKind::UpstreamSubmitted,
            request_id: Some(request_id),
            session: None,
            body_hash: Some(&outbound_hash),
            trace_ref: None,
            payload: json!({
                "route": route_label,
                "attempt_index": 0,
                "protocol_out": first.wire.as_str(),
                "stream": true,
            }),
        };
        if let Some(store) = &self.store {
            if store.append(intent).is_err() {
                return StreamOutcome::Failure(ForwardFailure {
                    status: 500,
                    code: ErrorCode::Internal,
                    message: "the state store rejected the upstream intent".into(),
                    details: Some(json!({"stage": "intent"})),
                });
            }
        }

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
        let head = match open_head(&client, &first, &outbound_bytes).await {
            OpenHead::Head(h) => h,
            OpenHead::Failed(status, message, details) => {
                return StreamOutcome::Failure(ForwardFailure {
                    status,
                    code: ErrorCode::UpstreamError,
                    message,
                    details: Some(details),
                })
            }
        };
        // A failure status head before any relayed byte is the ordinary
        // error path (R6 column 1): classify, record, answer the §8 body.
        if !(200..300).contains(&head.status) {
            let status = head.status;
            let resp = head.as_upstream_response();
            let evidence = ErrorEvidence {
                status: Some(status),
                retry_after: resp.retry_after.as_deref(),
                body: b"",
                wrote_full_request: true,
            };
            let cls = classify_upstream_error(&evidence);
            self.stream_event(
                EventKind::ErrorClassified,
                request_id,
                None,
                json!({
                    "attempt_index": 0,
                    "status": status,
                    "reason": cls.class.as_str(),
                    "action": "abort",
                    "matched": cls.matched,
                    "stream": true,
                }),
            );
            self.stream_event(
                EventKind::UpstreamResponded,
                request_id,
                None,
                json!({
                    "status": status,
                    "route": route_label,
                    "attempt_index": 0,
                    "wrote_full_request": true,
                    "stream": true,
                    "usage": Value::Null,
                }),
            );
            return StreamOutcome::Failure(ForwardFailure {
                status: 502,
                code: ErrorCode::UpstreamError,
                message: format!(
                    "upstream error status {status} on the streaming path ({}); not retried",
                    cls.class.as_str()
                ),
                details: Some(json!({
                    "upstream_status": status,
                    "error_class": cls.class.as_str(),
                    "stream": true,
                })),
            });
        }

        let content_type = head.content_type.clone();
        let client_requested_usage = parsed
            .get("stream_options")
            .and_then(|o| o.get("include_usage"))
            .and_then(|b| b.as_bool())
            .unwrap_or(false);

        let ctx = RelayCtx {
            store: self.store.clone(),
            idle,
            request_id: request_id.to_string(),
            client_requested_usage,
        };
        let state = RelayState {
            tap: SseUsageExtractor::new(first.wire, client_requested_usage),
            head,
            relayed: false,
            remaining,
            attempted: vec![first.route.provider.clone()],
            route_label,
            failover_from: None,
        };
        // The head answered from the primary route (any pre-relay
        // failover happens inside the relay and is reported there).
        let failover_from: Option<RouteSpec> = None;
        let relay = relay_stream(ctx, state, outbound_bytes);
        StreamOutcome::Success(StreamSuccess {
            status: 200,
            content_type,
            route: first.route.clone(),
            failover_from,
            body: Box::pin(relay),
        })
    }

    fn stream_event(
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
    /// A pre-relay failure as a terminal answer: (status, message, details).
    Failed(u16, String, Value),
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
        Ok(router_providers::stream::StreamOpen::NotSent(code, message)) => {
            OpenHead::Failed(502, message, json!({"code": code.as_str(), "stream": true}))
        }
        Err(message) => OpenHead::Failed(
            502,
            message,
            json!({"stage": "unknown_outcome", "error_class": "timeout", "stream": true}),
        ),
    }
}

/// The byte relay (R1/R2) with the accounting tap and R6's truncation
/// wired in. One item per upstream chunk, verbatim; the stream ends when
/// the upstream ends, and ends without any fabricated terminal marker
/// when it fails after the first relayed byte.
fn relay_stream(
    ctx: RelayCtx,
    init: RelayState,
    outbound: Bytes,
) -> impl Stream<Item = Bytes> + Send + 'static {
    unfold(
        (ctx, init, outbound),
        |(ctx, mut st, outbound)| async move {
            loop {
                match router_providers::stream::read_chunk(&mut st.head, ctx.idle).await {
                    router_providers::stream::StreamRead::Chunk(b) => {
                        if !b.is_empty() {
                            // R2 write-through: this chunk is the item; the
                            // tap reads a copy (R7).
                            st.tap.feed(&b);
                            st.relayed = true;
                            return Some((b, (ctx, st, outbound)));
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
                        if !st.relayed
                            && failover_zero_byte(&ctx, &mut st, &outbound, &message).await
                        {
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
        },
    )
}

/// R6's zero-byte branch: with no byte relayed the client's output is not
/// yet observable, so the fallback chain may be walked (provider
/// exclusion §4.2 + cooldown state). Mutates `st` in place on success;
/// returns false when the chain is exhausted (the caller truncates).
async fn failover_zero_byte(
    ctx: &RelayCtx,
    st: &mut RelayState,
    outbound: &[u8],
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
        event(
            ctx,
            EventKind::UpstreamSubmitted,
            json!({
                "route": route_label,
                "attempt_index": st.attempted.len() as u32,
                "protocol_out": cand.wire.as_str(),
                "stream": true,
            }),
        );
        st.attempted.push(cand.route.provider.clone());
        let Ok(client) = router_providers::stream::ReqwestStreamClient::new(ctx.idle) else {
            return false;
        };
        match open_head(&client, &cand, outbound).await {
            OpenHead::Head(h) if (200..300).contains(&h.status) => {
                if st.failover_from.is_none() {
                    st.failover_from = Some(st_route_spec(&st.route_label));
                }
                st.route_label = route_label;
                st.head = h;
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

/// The terminal `upstream.responded` (R8/R11): usage from the tap, or
/// `usage_missing: true`; truncation recorded, never presented as
/// complete.
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
            "usage": usage.as_ref().map(|u| json!({
                "input_total": u.input_total,
                "input_cached": u.input_cached,
                "cache_write": u.cache_write,
                "output": u.output,
                "reasoning": u.reasoning,
            })),
            "usage_missing": usage_missing,
        }),
    );
}

fn event(ctx: &RelayCtx, kind: EventKind, payload: Value) {
    if let Some(store) = &ctx.store {
        let _ = store.append(NewEvent {
            kind,
            request_id: Some(&ctx.request_id),
            session: None,
            body_hash: None,
            trace_ref: None,
            payload,
        });
    }
}

fn now_us() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0)
}
