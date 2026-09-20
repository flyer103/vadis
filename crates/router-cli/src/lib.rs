//! Library surface of the CLI crate so conformance cases can drive the
//! real `serve` assembly (CONF-25) without spawning a process.

#![forbid(unsafe_code)]

pub mod config_load;
pub mod stats;

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
    /// Report the window's figures from the trace + event log (spec §9.2)
    Stats {
        /// Path to the router config file (resolves `trace.dir`)
        #[arg(long)]
        config: String,
        /// The window, in the config duration grammar (`300ms`, `90s`,
        /// `15m`, `1h30m`). Required: a report must state its window.
        #[arg(long)]
        window: String,
        /// Emit the machine-readable form (the same figures, plus the
        /// notes the human form prints on stderr)
        #[arg(long)]
        json: bool,
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

    // Spec §4.7's startup resolution: the key names an env var whose
    // value is the inbound token, read **once** here and never re-read
    // (rotating it means restarting the process). A missing or empty
    // value refuses the start — exit 4, §12.10.2's class for an
    // unsatisfiable environment prerequisite — because a token-less
    // start would silently serve unauthenticated. No key ⇒ no gate at
    // all (CONF-45 ⑤).
    let auth_token: Option<String> = match &rc.router.server.auth_token_env {
        Some(name) => match std::env::var(name) {
            Ok(v) if !v.is_empty() => Some(v),
            other => {
                let state = match other {
                    Ok(_) => "empty",
                    Err(_) => "unset",
                };
                eprintln!(
                    "router: config file {config_path}: server.auth_token_env names {name}, \
                     which is {state}: refusing to start \
                     (a token-less start would serve unauthenticated)"
                );
                return 4;
            }
        },
        None => None,
    };

    // The store is a startup prerequisite (ADR-009 item 6/8): open and
    // migrate it before anything else runs. Failure exits non-zero with
    // the distinguishable reason — there is no in-memory degraded mode, and
    // no "state off" switch in v0.1. Exit code 4 (2 = config, 3 = bind).
    let store = match router_store::SqliteStore::open(&rc.state_db) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("router: state store {}: {e}", rc.state_db.display());
            return 4;
        }
    };
    // One shared handle: the Forwarder (writes + reads) and /health's plan
    // section (reads, spec §9.1) go through the same single-connection
    // store behind its mutex.
    let store_dyn: std::sync::Arc<dyn router_core::store::Store> = std::sync::Arc::new(store);
    // config.applied is an intent/accounting event (ADR-010 item 2): FULL,
    // committed before the process starts serving on top of it.
    let _config_applied = store_dyn.append(router_core::NewEvent {
        kind: router_core::EventKind::ConfigApplied,
        request_id: None,
        session: None,
        body_hash: None,
        trace_ref: None,
        payload: serde_json::json!({
            "config_path": config_path,
            "schema_version": store_dyn.schema_version().unwrap_or(0),
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
        // Spec §9.1: the `plan` section reads the `plan_state` projection
        // (and the probe gate's two projection inputs) through the writer's
        // own connection — read-only queries, one mutex, no migration.
        store: Some(store_dyn.clone()),
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

    // The trace sink: one DecisionRecord per request under
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

    // The transform rule engine (spec §4.4 `builtin/transform_rules`,
    // ADR-019): loaded from the first enabled `kind: builtin/transform_rules`
    // plugin's `config.rules_file`. A rule that fails to load, compile or
    // pass its inline tests does not load — the failure is named on the
    // startup log (a countable, declared state), and the rest of the set
    // serves. The mode is a request fact: this engine never edits a request
    // that did not ask (I3 — the closed-mode byte equality).
    let mut transform_engine: Option<std::sync::Arc<dyn router_core::transform::TransformEngine>> =
        None;
    for plug in &rc.router.plugins {
        if plug.disabled || plug.kind != "builtin/transform_rules" {
            continue;
        }
        let rules_file = plug
            .config
            .as_ref()
            .and_then(|c| c.get("rules_file"))
            .and_then(|v| v.as_str())
            .map(|v| v.to_string());
        let Some(rules_file) = rules_file else {
            eprintln!(
                "router: plugins[{}] (kind builtin/transform_rules) has no config.rules_file: \
                 not loaded (an engine that cannot find its rules is a named absence, \
                 not a silent empty one)",
                plug.id
            );
            continue;
        };
        let path = config_load::resolve(&rc.config_dir, &rules_file);
        match router_plugins::load_path(&path) {
            Ok((engine, report)) => {
                for line in report.failure_lines() {
                    eprintln!("router: {line}");
                }
                if report.loaded.is_empty() && report.failed.is_empty() {
                    eprintln!(
                        "router: transform_rules: rule file {} has no rules; \
                         transform mode will ask-but-not-apply",
                        path.display()
                    );
                }
                transform_engine = Some(std::sync::Arc::new(engine));
                // First hit wins (the three-level override is a later
                // card's lookup ladder; v0.1 has one file).
                break;
            }
            Err(e) => {
                eprintln!(
                    "router: transform_rules: rule file {}: {e}; not loaded \
                     (a request that asks for transform mode runs with an empty ledger)",
                    path.display()
                );
            }
        }
    }

    let forwarder = std::sync::Arc::new(Forwarder {
        config: rc.router.clone(),
        transports,
        api_keys,
        store: Some(store_dyn.clone()),
        trace: Some(trace_writer.clone()),
        // The transform rule engine (spec §4.4 `builtin/transform_rules`,
        // ADR-019): the loaded rule set, or `None` when no plugin declared
        // one / the file failed to read — a request that asks for
        // transform mode then runs with an empty ledger ("asked, not
        // applied" — a countable state, spec §6). The mode is a request
        // fact: this field never decides anything on the passthrough
        // path (I3).
        transform_engine,
        session_ttl_us: (rc.router.session.ttl.0 as i64).saturating_mul(1_000_000),
    });

    /// One POST through the forwarding engine: body bytes in, the engine's
    /// outcome mapped to an HTTP response. `stream: true` bodies take the
    /// SSE relay; both paths forward the body verbatim (minus
    /// router-owned keys) — never parsed and reserialized.
    async fn proxy_endpoint(
        forwarder: std::sync::Arc<Forwarder>,
        proto_in: WireApi,
        headers: Vec<(String, String)>,
        body: axum::body::Bytes,
    ) -> Response {
        let request_id = request_id();
        // The transform-mode opt-in, resolved at the boundary (ADR-019 §2,
        // spec §2.1) — decided before the body is read. An unusable value
        // is a 400 `invalid_request`: a typo must not silently disable a
        // saving the client asked for, and it must never silently enable
        // one. The header is not a body byte, so the opt-in cannot perturb
        // the prefix (§2.1).
        let transform_mode = match router_proxy::resolve_transform_mode(&headers) {
            Ok(m) => m,
            Err(f) => {
                // Write, then answer — the same order the auth guard uses
                // (the observation exists before the client is told); a
                // trace write failure is §8's non-blocking case.
                let now_epoch_s = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let record =
                    router_proxy::mode_refused_record(&request_id, proto_in, &f, now_epoch_s, 0);
                if let Some(t) = forwarder.trace.as_ref() {
                    let _ = t.write(&record);
                }
                let mut eb = ErrorBody::new(f.code, f.message, &request_id);
                eb.error.details = f.details;
                let mut resp =
                    (StatusCode::from_u16(f.status).expect("400 maps"), Json(eb)).into_response();
                if let Ok(v) = request_id.parse() {
                    resp.headers_mut().insert("x-router-request-id", v);
                }
                return resp;
            }
        };
        // The path split is decided by a shallow parse of `stream` only —
        // the same field the buffered engine refuses on, so the two paths
        // cannot both accept one request.
        let is_stream = serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("stream").and_then(|s| s.as_bool()))
            .unwrap_or(false);
        if is_stream {
            return match forwarder
                .forward_stream(proto_in, &body, &request_id, &headers, transform_mode)
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
                    // The three §8 headers go out with the head, before
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
            .forward(proto_in, &body, &request_id, &headers, transform_mode)
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

    // The wiring (§12.11): each protocol route is its own router so it
    // can carry its own guard layer with its own `proto_in` — the gate
    // never guesses a protocol from a path string, and a fourth route
    // added later cannot silently inherit a wrong one. `/health` is
    // registered outside the guarded set: its exemption is structural
    // (spec §4.7), not a path comparison inside the guard. When no key
    // was written, no layer is installed at all — the assembly is the
    // one v0.1 had before this key existed (CONF-45 ⑤).
    let health_router = axum::Router::new().route(
        "/health",
        get({
            let state = state.clone();
            move || {
                let body = router_proxy::health_json(&state);
                async move { (StatusCode::OK, Json(body)).into_response() }
            }
        }),
    );

    fn protocol_route(
        forwarder: std::sync::Arc<Forwarder>,
        proto_in: WireApi,
        path: &'static str,
    ) -> axum::Router {
        axum::Router::new().route(
            path,
            post(
                move |headers: axum::http::HeaderMap, body: axum::body::Bytes| {
                    let hs: Vec<(String, String)> = headers
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                        .collect();
                    proxy_endpoint(forwarder, proto_in, hs, body)
                },
            ),
        )
    }

    fn guarded_protocol_route(
        forwarder: std::sync::Arc<Forwarder>,
        gate: router_proxy::AuthGate,
        trace: std::sync::Arc<dyn router_core::TraceWriter>,
        proto_in: WireApi,
        path: &'static str,
    ) -> axum::Router {
        protocol_route(forwarder, proto_in, path).route_layer(axum::middleware::from_fn_with_state(
            GuardState {
                gate,
                trace,
                proto_in,
            },
            guard_mw,
        ))
    }

    /// What the guard runs with: the startup token's gate, the same
    /// `Arc<dyn TraceWriter>` the `Forwarder` holds (a refused request
    /// and a served one land in the same file, same format), and this
    /// route's own protocol.
    #[derive(Clone)]
    struct GuardState {
        gate: router_proxy::AuthGate,
        trace: std::sync::Arc<dyn router_core::TraceWriter>,
        proto_in: WireApi,
    }

    /// One boundary guard pass: read the headers, decide, and on a
    /// refusal write the record **then** answer — the same order the
    /// end-of-request path uses (the observation exists before the
    /// client is told). A trace write failure here is §8's non-blocking
    /// case and does not change the response.
    async fn guard_mw(
        axum::extract::State(st): axum::extract::State<GuardState>,
        req: axum::http::Request<axum::body::Body>,
        next: axum::middleware::Next,
    ) -> Response {
        let headers: Vec<(String, String)> = req
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        match st.gate.admits(&headers) {
            router_proxy::AuthVerdict::Admitted => next.run(req).await,
            verdict @ router_proxy::AuthVerdict::Refused { .. } => {
                let request_id = request_id();
                let started = std::time::Instant::now();
                let now_epoch_s = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let record = router_proxy::refused_record(
                    &request_id,
                    st.proto_in,
                    &verdict,
                    now_epoch_s,
                    started.elapsed().as_millis() as u32,
                );
                // Write, then answer — and either way the client is told
                // 401 (§8's non-blocking case).
                let _ = st.trace.write(&record);
                let mut body = ErrorBody::new(
                    router_core::error::ErrorCode::Unauthorized,
                    router_proxy::refused_message(&verdict),
                    request_id,
                );
                body.error.details = Some(serde_json::json!({
                    "header": match verdict {
                        router_proxy::AuthVerdict::Refused { header } => header,
                        _ => None,
                    },
                }));
                let mut resp = (
                    StatusCode::from_u16(router_core::error::ErrorCode::Unauthorized.http_status())
                        .expect("401 maps"),
                    Json(body),
                )
                    .into_response();
                if let Ok(v) = record.identity.request_id.parse() {
                    resp.headers_mut().insert("x-router-request-id", v);
                }
                resp
            }
        }
    }

    let app = match &auth_token {
        Some(token) => {
            // One gate per route (each carries its own `proto_in`), all
            // cloned from the single startup read.
            let chat = guarded_protocol_route(
                forwarder.clone(),
                router_proxy::AuthGate::new(token.clone()),
                trace_writer.clone(),
                WireApi::Chat,
                "/v1/chat/completions",
            );
            let responses = guarded_protocol_route(
                forwarder.clone(),
                router_proxy::AuthGate::new(token.clone()),
                trace_writer.clone(),
                WireApi::Responses,
                "/v1/responses",
            );
            let anthropic = guarded_protocol_route(
                forwarder.clone(),
                router_proxy::AuthGate::new(token.clone()),
                trace_writer.clone(),
                WireApi::Anthropic,
                "/v1/messages",
            );
            health_router.merge(chat).merge(responses).merge(anthropic)
        }
        None => health_router
            .merge(protocol_route(
                forwarder.clone(),
                WireApi::Chat,
                "/v1/chat/completions",
            ))
            .merge(protocol_route(
                forwarder.clone(),
                WireApi::Responses,
                "/v1/responses",
            ))
            .merge(protocol_route(
                forwarder.clone(),
                WireApi::Anthropic,
                "/v1/messages",
            )),
    }
    .with_state(());

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("router: cannot listen on {addr} (from server.addr): {e}");
            return 3;
        }
    };
    eprintln!(
        "router listening on {addr} (config dir: {}, trace: {}, state: {} [store open], auth: {})",
        rc.config_dir.display(),
        state.trace_dir,
        state.state_db,
        // §9.1's vocabulary; the variable's name is not printed here
        // (/health carries it), the value nowhere, ever.
        if auth_token.is_some() {
            "required"
        } else {
            "none"
        }
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
    // A process-local increment is enough
    // for the stub endpoints — no new dependency.
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("req-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}
