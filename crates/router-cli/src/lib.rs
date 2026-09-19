//! Library surface of the CLI crate so conformance cases can drive the
//! real `serve` assembly (CONF-25) without spawning a process.

#![forbid(unsafe_code)]

pub mod config_load;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "router", version, about = "multi-protocol LLM gateway")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Start the HTTP proxy (config-driven)
    Serve {
        /// Path to the router config file
        #[arg(long)]
        config: String,
    },
}

/// `router serve`: load the config (exit code 2 + reason on any failure),
/// then serve exactly what it says. Returns the process exit code.
pub async fn serve(config_path: &str) -> i32 {
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use axum::routing::{get, post};
    use axum::Json;
    use router_core::config::WireApi;
    use router_core::error::ErrorBody;
    use router_core::store::Store as _;
    use router_proxy::{AppState, ForwardOutcome, Forwarder};
    use std::collections::HashMap;

    // Load-time contract (DESIGN §12.10.2): an illegal config exits
    // non-zero with the reason — never a silent fallback to defaults.
    let rc = match config_load::load(std::path::Path::new(config_path)) {
        Ok(rc) => rc,
        Err(reason) => {
            eprintln!("router: {reason}");
            return 2;
        }
    };

    let addr = match rc.router.listen_addr() {
        Ok(a) => a,
        Err(e) => {
            // validate() already ran in load(); unreachable in practice.
            eprintln!("router: config file {config_path}: {e}");
            return 2;
        }
    };

    // The store is a startup prerequisite (ADR-009 item 6/8): open and
    // migrate it before anything else runs. Failure exits non-zero with the
    // distinguishable reason — there is no in-memory degraded mode, and no
    // "state off" switch in v0.1. Exit code 4 (2 = config, 3 = bind).
    let store = match router_store::SqliteStore::open(&rc.state_db) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("router: state store {}: {e}", rc.state_db.display());
            return 4;
        }
    };
    // config.applied is an intent/accounting event (ADR-010 item 2): FULL,
    // committed before the process starts serving on top of it.
    let _config_applied = store.append(router_core::NewEvent {
        kind: router_core::EventKind::ConfigApplied,
        request_id: None,
        session: None,
        body_hash: None,
        trace_ref: None,
        payload: serde_json::json!({
            "config_path": config_path,
            "schema_version": store.schema_version().unwrap_or(0),
        }),
    });

    // Key presence is probed once at startup; the values themselves are
    // never read here (the provider client holds them, §12.10.1) and never
    // reported — only their presence travels to /health.
    let provider_keys: Vec<(String, String, bool)> = rc
        .router
        .providers
        .iter()
        .map(|p| {
            (
                p.name.clone(),
                p.api_key_env.clone(),
                std::env::var_os(&p.api_key_env).is_some(),
            )
        })
        .collect();

    let unavailable: Vec<&str> = provider_keys
        .iter()
        .filter(|(_, _, present)| !present)
        .map(|(name, _, _)| name.as_str())
        .collect();
    if !unavailable.is_empty() {
        // Not a startup failure (§12.5): reported per provider via /health.
        eprintln!(
            "router: no api key in env for provider(s) {}: marked unavailable (see /health)",
            unavailable.join(", ")
        );
    }

    let state = std::sync::Arc::new(AppState {
        config: rc.router.clone(),
        trace_dir: rc.trace_dir.to_string_lossy().into_owned(),
        state_db: rc.state_db.to_string_lossy().into_owned(),
        provider_keys,
    });

    // The forwarding engine (DESIGN §12.10.5): one transport per provider
    // with a key present, the api key values read once here and held only
    // by the engine (never in /health, logs or events). A keyless provider
    // gets no transport: the chain skips it and /health reports it
    // unavailable.
    let mut transports: HashMap<String, std::sync::Arc<dyn router_proxy::ProviderTransport>> =
        HashMap::new();
    let mut api_keys: HashMap<String, String> = HashMap::new();
    for p in &rc.router.providers {
        if let Some(v) = std::env::var_os(&p.api_key_env) {
            let key = v.to_string_lossy().into_owned();
            let timeout =
                std::time::Duration::from_millis(rc.router.server.upstream_attempt_timeout.0);
            match router_providers::ReqwestProviderClient::new(timeout) {
                Ok(client) => {
                    transports.insert(p.name.clone(), std::sync::Arc::new(client));
                    api_keys.insert(p.name.clone(), key);
                }
                Err(e) => {
                    eprintln!("router: provider '{}': client build failed: {e}", p.name);
                }
            }
        }
    }

    // The trace sink (R2-2f): one DecisionRecord per request under
    // `trace.dir`, rolled hourly. A startup failure is a refusal — a
    // router that cannot write its analysis truth must not serve (same
    // line as CONF-23a; per-request failures are the non-blocking §8
    // case, handled inside the forwarding path).
    let trace_sink = match router_store::TraceSink::open(&rc.trace_dir) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("router: trace dir {}: {e}", rc.trace_dir.display());
            return 4;
        }
    };
    let trace_writer: std::sync::Arc<dyn router_core::TraceWriter> =
        std::sync::Arc::new(trace_sink);

    let forwarder = std::sync::Arc::new(Forwarder {
        config: rc.router.clone(),
        transports,
        api_keys,
        store: Some(std::sync::Arc::new(store) as std::sync::Arc<dyn router_core::store::Store>),
        trace: Some(trace_writer),
        session_ttl_us: (rc.router.session.ttl.0 as i64).saturating_mul(1_000_000),
    });

    /// One POST through the forwarding engine: body bytes in, the engine's
    /// outcome mapped to an HTTP response. `stream: true` bodies take the
    /// SSE relay (R2-2e); both paths forward the body verbatim (minus
    /// router-owned keys) — never parsed and reserialized.
    async fn proxy_endpoint(
        forwarder: std::sync::Arc<Forwarder>,
        proto_in: WireApi,
        headers: Vec<(String, String)>,
        body: axum::body::Bytes,
    ) -> Response {
        let request_id = request_id();
        // The path split is decided by a shallow parse of `stream` only —
        // the same field the buffered engine refuses on, so the two paths
        // cannot both accept one request.
        let is_stream = serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("stream").and_then(|s| s.as_bool()))
            .unwrap_or(false);
        if is_stream {
            return match forwarder
                .forward_stream(proto_in, &body, &request_id, &headers)
                .await
            {
                router_proxy::StreamOutcome::Success(s) => {
                    // axum's body wants a TryStream; the relay never errs,
                    // so every item is Ok — the truncation semantics are
                    // carried by the stream simply ending (R6).
                    use futures::StreamExt as _;
                    let ok_items = s.body.map(Ok::<_, std::io::Error>);
                    let mut resp = Response::new(axum::body::Body::from_stream(ok_items));
                    *resp.status_mut() =
                        StatusCode::from_u16(s.status).expect("upstream status maps");
                    if let Some(ct) = s.content_type {
                        if let Ok(v) = ct.parse() {
                            resp.headers_mut()
                                .insert(axum::http::header::CONTENT_TYPE, v);
                        }
                    }
                    // R3: the three §8 headers go out with the head, before
                    // the first event byte.
                    if let Ok(v) = request_id.parse() {
                        resp.headers_mut().insert("x-router-request-id", v);
                    }
                    resp
                }
                router_proxy::StreamOutcome::Failure(f) => {
                    let mut body = ErrorBody::new(f.code, f.message, request_id);
                    body.error.details = f.details;
                    (
                        StatusCode::from_u16(f.status).expect("failure status maps"),
                        Json(body),
                    )
                        .into_response()
                }
            };
        }
        let outcome = forwarder
            .forward(proto_in, &body, &request_id, &headers)
            .await;
        match outcome {
            ForwardOutcome::Success(s) => {
                let mut resp = Response::new(axum::body::Body::from(s.body));
                *resp.status_mut() = StatusCode::from_u16(s.status).expect("upstream status maps");
                if let Some(ct) = s.content_type {
                    if let Ok(v) = ct.parse() {
                        resp.headers_mut()
                            .insert(axum::http::header::CONTENT_TYPE, v);
                    }
                }
                if let Some(from) = s.failover_from {
                    if let Ok(v) = from.to_string().parse() {
                        resp.headers_mut().insert("x-router-failover-from", v);
                    }
                }
                resp
            }
            ForwardOutcome::Failure(f) => {
                let mut body = ErrorBody::new(f.code, f.message, request_id);
                body.error.details = f.details;
                (
                    StatusCode::from_u16(f.status).expect("failure status maps"),
                    Json(body),
                )
                    .into_response()
            }
        }
    }

    let app = axum::Router::new()
        .route(
            "/health",
            get({
                let state = state.clone();
                move || {
                    let body = router_proxy::health_json(&state);
                    async move { (StatusCode::OK, Json(body)).into_response() }
                }
            }),
        )
        .route(
            "/v1/chat/completions",
            post({
                let forwarder = forwarder.clone();
                move |headers: axum::http::HeaderMap, body: axum::body::Bytes| {
                    let hs: Vec<(String, String)> = headers
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                        .collect();
                    proxy_endpoint(forwarder, WireApi::Chat, hs, body)
                }
            }),
        )
        .route(
            "/v1/responses",
            post({
                let forwarder = forwarder.clone();
                move |headers: axum::http::HeaderMap, body: axum::body::Bytes| {
                    let hs: Vec<(String, String)> = headers
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                        .collect();
                    proxy_endpoint(forwarder, WireApi::Responses, hs, body)
                }
            }),
        )
        .route(
            "/v1/messages",
            post({
                let forwarder = forwarder.clone();
                move |headers: axum::http::HeaderMap, body: axum::body::Bytes| {
                    let hs: Vec<(String, String)> = headers
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                        .collect();
                    proxy_endpoint(forwarder, WireApi::Anthropic, hs, body)
                }
            }),
        )
        .with_state(());

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("router: cannot listen on {addr} (from server.addr): {e}");
            return 3;
        }
    };
    eprintln!(
        "router listening on {addr} (config dir: {}, trace: {}, state: {} [store open])",
        rc.config_dir.display(),
        state.trace_dir,
        state.state_db
    );
    match axum::serve(listener, app).await {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("router: server error: {e}");
            1
        }
    }
}

fn request_id() -> String {
    // R2's data plane switches to uuid; a process-local increment is enough
    // for the stub endpoints — no new dependency.
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("req-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}
