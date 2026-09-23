//! Library surface of the CLI crate so conformance cases can drive the
//! real `serve` assembly (CONF-25) without spawning a process.

#![forbid(unsafe_code)]

pub mod config_load;
pub mod config_path;
pub mod setup;
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
        /// Path to the router config file; absent, found by spec §4.12's
        /// discovery order (--config > the XDG location > ./config.yaml,
        /// else a refusal naming `router setup`)
        #[arg(long)]
        config: Option<String>,
    },
    /// Report the window's figures from the trace + event log (spec §9.2)
    Stats {
        /// Path to the router config file (resolves `trace.dir`); absent,
        /// found by spec §4.12's discovery order, as for `serve`
        #[arg(long)]
        config: Option<String>,
        /// The window, in the config duration grammar (`300ms`, `90s`,
        /// `15m`, `1h30m`). Required: a report must state its window.
        #[arg(long)]
        window: String,
        /// Emit the machine-readable form (the same figures, plus the
        /// notes the human form prints on stderr)
        #[arg(long)]
        json: bool,
    },
    /// Guided first configuration (spec §4.11–§4.12): edits the config by
    /// anchored single-line edits on a verbatim template — never by
    /// re-serializing it
    Setup {
        /// One section to walk (server | auth | session | paths |
        /// providers | routing | plugins), or all of them when absent
        #[arg(value_name = "SECTION")]
        section: Option<String>,
        /// The file to write; absent, resolved by spec §4.12's discovery
        /// order (the XDG location is created when nothing is found)
        #[arg(long)]
        config: Option<String>,
        /// The template to start from; default = the config.example.yaml
        /// embedded in this binary
        #[arg(long)]
        from: Option<String>,
        /// No prompt at all: every question takes its default (CI path)
        #[arg(long)]
        non_interactive: bool,
        /// Ask only about the items --check reports unsatisfied
        #[arg(long)]
        quick: bool,
        /// Print each section's keys with the value the file carries; no
        /// prompt, no write
        #[arg(long)]
        print: bool,
        /// Load the file with the same loader `serve` runs, then check
        /// every environment variable it names; no prompt, no write
        #[arg(long)]
        check: bool,
        /// Emit the machine-readable form (--print / --check / --dry-run)
        #[arg(long)]
        json: bool,
        /// Print the edits the run would make, in application order; no
        /// write
        #[arg(long)]
        dry_run: bool,
        /// The base becomes the template instead of the file that is
        /// there (implies --backup)
        #[arg(long)]
        force: bool,
        /// Before a write, copy the target to <target>.bak
        #[arg(long)]
        backup: bool,
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
    // reported — only their presence travels to /health, beside each
    // entry's declared region and currency (spec §4.8).
    let provider_keys: Vec<router_proxy::ProviderKeyFacts> = rc
        .router
        .providers
        .iter()
        .map(|p| router_proxy::ProviderKeyFacts {
            name: p.name.clone(),
            api_key_env: p.api_key_env.clone(),
            present: std::env::var_os(&p.api_key_env).is_some(),
            region: p.region,
            currency: p.currency,
        })
        .collect();

    let unavailable: Vec<&str> = provider_keys
        .iter()
        .filter(|f| !f.present)
        .map(|f| f.name.as_str())
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
        // The session TTL's one ms → µs conversion (spec §4.5, note R6):
        // `session.ttl` is a `DurationVal` in **milliseconds**, every
        // consumer of `session_ttl_us` reads **microseconds** — so the
        // factor is × 1 000, applied once here at the resolution site,
        // never in a consumer (R21-F6: the shipped v0.1 multiplied by
        // 1 000 000, honouring a configured 12h as ~12 000 h). The
        // saturation shape is the in-tree owner's
        // (`PlanPolicyCfg::cooldown_us`): `try_from` + saturating, not a
        // plain `as` cast, which would wrap u64 → i64 negative.
        session_ttl_us: i64::try_from(rc.router.session.ttl.0)
            .unwrap_or(i64::MAX)
            .saturating_mul(1_000),
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
        limit: usize,
        trace: std::sync::Arc<dyn router_core::TraceWriter>,
    ) -> axum::Router {
        axum::Router::new()
            .route(
                path,
                post(
                    move |headers: axum::http::HeaderMap,
                          axum::extract::Extension(body): axum::extract::Extension<
                        axum::body::Bytes,
                    >| {
                        let hs: Vec<(String, String)> = headers
                            .iter()
                            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                            .collect();
                        proxy_endpoint(forwarder, proto_in, hs, body)
                    },
                ),
            )
            // §4.13 / DESIGN §12.15: the framework's own default body limit
            // (axum's implicit 2 MiB `Limited` wrapper) is **disabled** on
            // the three protocol routes so exactly one cap exists — the
            // configured one, answered in §8's shape by the boundary layer
            // below instead of the framework's plain-text 413.
            .layer(axum::extract::DefaultBodyLimit::disable())
            // The bound itself (§12.15): inside the token guard (installed
            // later, so it wraps this one and stays outermost), above the
            // transform-mode resolution and the path split — both live in
            // `proxy_endpoint`, which this layer runs before. It reads the
            // body bounded and hands the surviving bytes on unchanged, so
            // the byte path (§12.3.1) sees the client's bytes exactly as
            // before.
            .route_layer(axum::middleware::from_fn_with_state(
                BodyLimitState {
                    limit,
                    trace,
                    proto_in,
                },
                body_limit_mw,
            ))
    }

    /// What the bound runs with: the configured limit, the same
    /// `Arc<dyn TraceWriter>` the `Forwarder` holds (a refused request
    /// and a served one land in the same file, same format), and this
    /// route's own protocol — the §12.11 assembly's shape.
    #[derive(Clone)]
    struct BodyLimitState {
        limit: usize,
        trace: std::sync::Arc<dyn router_core::TraceWriter>,
        proto_in: WireApi,
    }

    /// One boundary pass of the inbound body bound (spec §4.13,
    /// DESIGN §12.15). Header-first: a declared `Content-Length` above
    /// the bound refuses **without reading a byte**. Otherwise the read
    /// itself is bounded; the moment the bound is passed the same
    /// refusal answers. One response, one trace record, both arms —
    /// and the connection is closed (the body may not have been
    /// drained, and a keep-alive connection holding unread request
    /// bytes would parse them as the next request).
    async fn body_limit_mw(
        axum::extract::State(st): axum::extract::State<BodyLimitState>,
        req: axum::http::Request<axum::body::Body>,
        next: axum::middleware::Next,
    ) -> Response {
        use futures::StreamExt as _;
        let started = std::time::Instant::now();
        let content_length = req
            .headers()
            .get(axum::http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());
        let refuse = |declared: Option<u64>| {
            let request_id = request_id();
            let now_epoch_s = std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let record = router_proxy::too_large_record(
                &request_id,
                st.proto_in,
                st.limit,
                declared,
                now_epoch_s,
                started.elapsed().as_millis() as u32,
            );
            // Write, then answer — the same order the auth guard uses;
            // a trace write failure is §8's non-blocking case.
            let _ = st.trace.write(&record);
            let mut eb = ErrorBody::new(
                router_core::error::ErrorCode::RequestTooLarge,
                router_proxy::too_large_message(st.limit, declared),
                &request_id,
            );
            eb.error.details = Some(serde_json::json!({
                "limit_bytes": st.limit,
                "content_length": declared,
            }));
            let mut resp = (
                StatusCode::from_u16(router_core::error::ErrorCode::RequestTooLarge.http_status())
                    .expect("413 maps"),
                Json(eb),
            )
                .into_response();
            if let Ok(v) = request_id.parse() {
                resp.headers_mut().insert("x-router-request-id", v);
            }
            // §4.13: the connection is closed after the refusal — the
            // refused request's body may not have been drained.
            resp.headers_mut().insert(
                axum::http::header::CONNECTION,
                axum::http::HeaderValue::from_static("close"),
            );
            resp
        };
        // Arm 1 — header-first: the declared length refuses without a
        // body read.
        if let Err(declared) = router_proxy::check_declared(st.limit, content_length) {
            return refuse(Some(declared));
        }
        // Arm 2 — the bounded read (no declaration, or one at/below the
        // bound): the moment the accumulated bytes pass the bound, the
        // same refusal answers.
        let (mut parts, body) = req.into_parts();
        let mut buf: Vec<u8> = Vec::with_capacity(content_length.unwrap_or(0) as usize);
        let mut exceeded = false;
        let mut read_failed = false;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    buf.extend_from_slice(&bytes);
                    if buf.len() > st.limit {
                        exceeded = true;
                        break;
                    }
                }
                Err(_) => {
                    // A transport-level read failure (client disconnect
                    // mid-body): the same class the framework's `Bytes`
                    // extractor rejected with a bare 400 before this
                    // layer existed — no record, no invented refusal.
                    read_failed = true;
                    break;
                }
            }
        }
        if exceeded {
            return refuse(content_length);
        }
        if read_failed {
            let mut resp =
                (StatusCode::BAD_REQUEST, "Failed to buffer the request body").into_response();
            resp.headers_mut().insert(
                axum::http::header::CONNECTION,
                axum::http::HeaderValue::from_static("close"),
            );
            return resp;
        }
        let bytes = axum::body::Bytes::from(buf);
        parts.extensions.insert(bytes);
        let req = axum::http::Request::from_parts(parts, axum::body::Body::empty());
        next.run(req).await
    }

    fn guarded_protocol_route(
        forwarder: std::sync::Arc<Forwarder>,
        gate: router_proxy::AuthGate,
        trace: std::sync::Arc<dyn router_core::TraceWriter>,
        proto_in: WireApi,
        path: &'static str,
        limit: usize,
    ) -> axum::Router {
        protocol_route(forwarder, proto_in, path, limit, trace.clone()).route_layer(
            axum::middleware::from_fn_with_state(
                GuardState {
                    gate,
                    trace,
                    proto_in,
                },
                guard_mw,
            ),
        )
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

    // Spec §4.13's bound, resolved once from the validated config: the
    // loader already refused anything below 1024, so the `usize` cast
    // here is total.
    let max_body_bytes = rc.router.server.max_body_bytes as usize;

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
                max_body_bytes,
            );
            let responses = guarded_protocol_route(
                forwarder.clone(),
                router_proxy::AuthGate::new(token.clone()),
                trace_writer.clone(),
                WireApi::Responses,
                "/v1/responses",
                max_body_bytes,
            );
            let anthropic = guarded_protocol_route(
                forwarder.clone(),
                router_proxy::AuthGate::new(token.clone()),
                trace_writer.clone(),
                WireApi::Anthropic,
                "/v1/messages",
                max_body_bytes,
            );
            health_router.merge(chat).merge(responses).merge(anthropic)
        }
        None => health_router
            .merge(protocol_route(
                forwarder.clone(),
                WireApi::Chat,
                "/v1/chat/completions",
                max_body_bytes,
                trace_writer.clone(),
            ))
            .merge(protocol_route(
                forwarder.clone(),
                WireApi::Responses,
                "/v1/responses",
                max_body_bytes,
                trace_writer.clone(),
            ))
            .merge(protocol_route(
                forwarder.clone(),
                WireApi::Anthropic,
                "/v1/messages",
                max_body_bytes,
                trace_writer.clone(),
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
