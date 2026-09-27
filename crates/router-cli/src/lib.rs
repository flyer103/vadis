//! Library surface of the CLI crate so conformance cases can drive the
//! real `serve` assembly (CONF-25) without spawning a process.

#![forbid(unsafe_code)]

pub mod config_load;
pub mod config_path;
pub mod metrics;
pub mod reload;
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

/// The production trace writer: the hourly sink plus the identity of the
/// configuration this process loaded (ADR-037 D6; spec §6). The digest is
/// computed once by the loader and carried here as an immutable value, so
/// every record the process writes is stamped with the one config identity
/// by the constructors that read it back off this writer (`TraceWriter::
/// config_digest`) — the record and its write cannot disagree, and nothing
/// behind a request opens, reads or hashes a file.
///
/// The sink is shared (`Arc`): the reload (ADR-040 D2/D6) builds one
/// writer per revision — stamped with THAT revision's digest — over the
/// process's one sink, so a request finishing after a switch is still
/// stamped by the revision that served it.
pub(crate) struct ConfigTraceWriter {
    inner: std::sync::Arc<router_store::TraceSink>,
    config_digest: String,
}

impl ConfigTraceWriter {
    /// The served writer's one constructor — and the empty digest's
    /// tripwire (R43-F4; spec §6's empty-string bullet: "a writer wired
    /// into the serving path that cannot produce a non-empty digest is a
    /// defect, and forgetting it is loud, not silent"). The loader's
    /// digest is never empty (spec §4.14's recipe hashes the root's half
    /// always), so an empty value here is a *code defect*, and it fails
    /// AT CONSTRUCTION, naming the writer — what may never happen is a
    /// record stamped with an identity of `""`.
    fn new(inner: std::sync::Arc<router_store::TraceSink>, config_digest: String) -> Self {
        assert!(
            !config_digest.is_empty(),
            "ConfigTraceWriter: an empty config_digest on the serving path is a defect \
             (R43-F4; spec §6) — the writer fails at construction rather than stamping \
             a record with an empty identity"
        );
        Self {
            inner,
            config_digest,
        }
    }
}

impl router_core::TraceWriter for ConfigTraceWriter {
    fn write(&self, rec: &router_core::DecisionRecord) -> Result<Option<String>, String> {
        router_core::TraceWriter::write(self.inner.as_ref(), rec)
    }

    fn config_digest(&self) -> &str {
        &self.config_digest
    }
}

/// The one revision assembly (ADR-040 D2: "the candidate is built and
/// validated before anything is visible"). `serve` builds the startup
/// revision with it and the reload's [`reload::Publisher`] builds every
/// accepted candidate with it — ONE assembly, so a switch and a restart
/// cannot drift apart. Everything per-revision is (re)built here: the
/// provider transports and keys, the plugin assembly and its transform
/// engine, the trace writer stamped with THIS revision's digest, the
/// session TTL, the `/health` facts. Everything process-level is taken
/// as given: the store handle, the trace sink (`trace.dir` is refused,
/// D5), and the already-resolved auth gate.
///
/// Total by construction: a provider whose client fails to build is
/// skipped with a note (the startup behaviour, unchanged), the plugin
/// assembly reports through its own notes, and a keyless provider simply
/// gets no transport.
pub(crate) fn build_revision(
    rc: &config_load::ResolvedConfig,
    store: &std::sync::Arc<dyn router_core::store::Store>,
    trace_sink: &std::sync::Arc<router_store::TraceSink>,
    auth_gate: Option<router_proxy::AuthGate>,
    note: &dyn Fn(String),
) -> router_proxy::Revision {
    use std::collections::HashMap;

    // Key presence is probed at the revision's build; the values
    // themselves are never read here (the provider client holds them,
    // §12.10.1) and never reported — only their presence travels to
    // /health, beside each entry's declared region and currency
    // (spec §4.8).
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
        note(format!(
            "no api key in env for provider(s) {}: marked unavailable (see /health)",
            unavailable.join(", ")
        ));
    }

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
                    note(format!("provider '{}': client build failed: {e}", p.name));
                }
            }
        }
    }

    // The plugin assembly (ADR-036 D1/D8, spec §4.3): the CLI is the
    // launcher, not the assembler. It hands the declared `plugins:` list
    // to the loader-driven assembly — with the config dir's path
    // resolution as the only wiring — surfaces the assembly's notes
    // verbatim (named absences, rule-load failures, named loading waits),
    // and reads the Forwarder's engine from the assembled context's typed
    // slot. Resolution happens at the revision's build: nothing on the
    // serving path touches the loader (D8). The transform engine is the
    // rule set of spec §4.4 (`builtin/transform_rules`, ADR-019), or
    // `None` when no entry mounted one — a request that asks for
    // transform mode then runs with an empty ledger ("asked, not
    // applied" — a countable state, spec §6). The mode is a request
    // fact: the engine never edits a request that did not ask (I3).
    let assembly = router_plugins::assemble(&rc.router.plugins, &|file| {
        config_load::resolve(&rc.config_dir, file)
    });
    for line in assembly.notes() {
        note(line.clone());
    }
    let transform_engine = assembly.transform_engine();

    // The revision's trace writer: the process's one sink (shared Arc),
    // stamped with THIS revision's digest — a request finishing after a
    // switch is still stamped by the revision that served it (D3).
    let trace_writer: std::sync::Arc<dyn router_core::TraceWriter> = std::sync::Arc::new(
        ConfigTraceWriter::new(trace_sink.clone(), rc.identity.config_digest.clone()),
    );

    let forwarder = router_proxy::Forwarder {
        config: rc.router.clone(),
        transports,
        api_keys,
        store: Some(store.clone()),
        trace: Some(trace_writer),
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
    };

    router_proxy::Revision {
        forwarder,
        // Spec §9.1's `config` member: the loader-computed identity,
        // handed to the proxy as strings (ADR-037 D4: the proxy never
        // opens, resolves or hashes a config file).
        config_identity: router_proxy::ConfigIdentity {
            root_path: rc.identity.root_path.to_string_lossy().into_owned(),
            roster_path: rc
                .identity
                .roster_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            root_sha16: rc.identity.root_sha16.clone(),
            roster_sha16: rc.identity.roster_sha16.clone(),
            config_digest: rc.identity.config_digest.clone(),
        },
        provider_keys,
        auth_gate,
    }
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

    use router_proxy::{AppState, ForwardOutcome};

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
            // Row 13's designed "config digest" half (DESIGN §12.10.5;
            // ADR-037 D6): the composed digest beside both resolved paths
            // and both file digests — the same value the trace rows and
            // /health carry.
            "root_path": rc.identity.root_path.to_string_lossy(),
            "roster_path": rc.identity.roster_path.as_ref().map(|p| p.to_string_lossy()),
            "root_sha16": rc.identity.root_sha16,
            "roster_sha16": rc.identity.roster_sha16,
            "config_digest": rc.identity.config_digest,
            // One flat object on every row 13 (note R10): at startup the
            // two switch members are null — there is no predecessor to
            // name or to diff against (ADR-040 D10).
            "previous_config_digest": serde_json::Value::Null,
            "changed_keys": serde_json::Value::Null,
        }),
    });

    // The trace sink: one DecisionRecord per request under
    // `trace.dir`, rolled hourly. A startup failure is a refusal — a
    // router that cannot write its analysis truth must not serve (same
    // line as CONF-23a; per-request failures are the non-blocking §8
    // case, handled inside the forwarding path). The sink is
    // process-level and shared (`Arc`): the reload refuses a `trace.dir`
    // change (ADR-040 D5), so every revision's writer sits over this one
    // sink, stamped with that revision's digest.
    let trace_sink = match router_store::TraceSink::open(&rc.trace_dir) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("router: trace dir {}: {e}", rc.trace_dir.display());
            return 4;
        }
    };
    let trace_sink = std::sync::Arc::new(trace_sink);

    // The startup revision, built by exactly the assembly the reload's
    // publish uses (ONE assembly — startup and a switch cannot drift
    // apart), then published into the cell the request path captures once
    // per request (ADR-040 D2/D3; DESIGN §12.20's seam).
    let startup_gate = auth_token.clone().map(router_proxy::AuthGate::new);
    // §4.7's "read once" facts, handed to the publisher: a revision
    // naming the same variable reuses this read; a renamed one is read
    // at the switch (ADR-040 D5's honesty note).
    let startup_auth: Option<(String, String)> = rc
        .router
        .server
        .auth_token_env
        .clone()
        .zip(auth_token.clone());
    let revision = {
        let note = |line: String| eprintln!("router: {line}");
        build_revision(&rc, &store_dyn, &trace_sink, startup_gate, &note)
    };
    let cell = router_proxy::RevisionCell::new(revision);

    let state = std::sync::Arc::new(AppState {
        revision: cell.clone(),
        trace_dir: rc.trace_dir.to_string_lossy().into_owned(),
        state_db: rc.state_db.to_string_lossy().into_owned(),
        // Spec §9.1: the `plan` section reads the `plan_state` projection
        // (and the probe gate's two projection inputs) through the writer's
        // own connection — read-only queries, one mutex, no migration.
        store: Some(store_dyn.clone()),
    });

    /// One POST through the forwarding engine: the revision is the one
    /// the guard captured **at receive** (ADR-040 D2's capture-once rule —
    /// this request reads THAT revision for its whole lifetime, even if a
    /// reload publishes mid-flight). Body bytes in, the engine's outcome
    /// mapped to an HTTP response. `stream: true` bodies take the
    /// SSE relay; both paths forward the body verbatim (minus
    /// router-owned keys) — never parsed and reserialized.
    async fn proxy_endpoint(
        rev: std::sync::Arc<router_proxy::Revision>,
        proto_in: WireApi,
        headers: Vec<(String, String)>,
        body: axum::body::Bytes,
    ) -> Response {
        let forwarder = &rev.forwarder;
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
                let record = router_proxy::mode_refused_record(
                    &request_id,
                    proto_in,
                    &f,
                    now_epoch_s,
                    0,
                    forwarder
                        .trace
                        .as_ref()
                        .map(|t| t.config_digest())
                        .unwrap_or(""),
                );
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
    // (spec §4.7), not a path comparison inside the guard. The guard
    // layer is ALWAYS installed (R47-2): the gate lives on the revision
    // now, and a reload may introduce `server.auth_token_env` into a
    // process that started without one (ADR-040 D5's honesty note) — a
    // route with no layer could never engage it. A revision with no gate
    // admits everything, which is CONF-45 ⑤'s behaviour exactly.
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
        revision: router_proxy::SharedRevision,
        proto_in: WireApi,
        path: &'static str,
        limit: usize,
    ) -> axum::Router {
        axum::Router::new()
            .route(
                path,
                post(
                    move |headers: axum::http::HeaderMap,
                          axum::extract::Extension(rev): axum::extract::Extension<
                        std::sync::Arc<router_proxy::Revision>,
                    >,
                          axum::extract::Extension(body): axum::extract::Extension<
                        axum::body::Bytes,
                    >| {
                        let hs: Vec<(String, String)> = headers
                            .iter()
                            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                            .collect();
                        proxy_endpoint(rev, proto_in, hs, body)
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
                    revision,
                    proto_in,
                },
                body_limit_mw,
            ))
    }

    /// What the bound runs with: the configured limit (process-level —
    /// `server.max_body_bytes` is in ADR-040 D5's refused set, so no
    /// revision moves it), the revision handle whose capture supplies the
    /// trace writer (a refused request's record is stamped with the
    /// revision IN FORCE at receive), and this route's own protocol —
    /// the §12.11 assembly's shape.
    #[derive(Clone)]
    struct BodyLimitState {
        limit: usize,
        revision: router_proxy::SharedRevision,
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
        // The revision this request serves under: the guard's capture,
        // handed down through the request's extensions (the guard is the
        // outermost layer, so it is always present); the fallback is a
        // capture here — the same rule, one level down.
        let rev = req
            .extensions()
            .get::<std::sync::Arc<router_proxy::Revision>>()
            .cloned()
            .unwrap_or_else(|| st.revision.capture());
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
                rev.forwarder
                    .trace
                    .as_ref()
                    .map(|t| t.config_digest())
                    .unwrap_or(""),
            );
            // Write, then answer — the same order the auth guard uses;
            // a trace write failure is §8's non-blocking case.
            if let Some(t) = rev.forwarder.trace.as_ref() {
                let _ = t.write(&record);
            }
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
        revision: router_proxy::SharedRevision,
        proto_in: WireApi,
        path: &'static str,
        limit: usize,
    ) -> axum::Router {
        protocol_route(revision.clone(), proto_in, path, limit).route_layer(
            axum::middleware::from_fn_with_state(
                GuardState {
                    revision,
                    proto_in: proto_in.as_str(),
                },
                guard_mw,
            ),
        )
    }

    /// `GET /metrics` (spec §4.16, ADR-041 §3.7, DESIGN §12.21): the one
    /// guarded route that is not a protocol endpoint. It sits BEHIND the
    /// same guard — §4.7's exemption is `/health`'s alone ("Nothing else
    /// is exempt") — and its guard state carries the endpoint's own
    /// protocol word, `"metrics"`, so a refused scrape leaves the same
    /// boundary-class record a refused protocol request does. The handler
    /// takes the state and nothing else: NO `Bytes` extractor, no header
    /// extractor, no body layer (the §4.13 bound is not installed — the
    /// route reads nothing, so it needs no bound; `/health` has the same
    /// property today). The admitted arm writes nothing.
    fn guarded_metrics_route(
        revision: router_proxy::SharedRevision,
        state: std::sync::Arc<AppState>,
    ) -> axum::Router {
        axum::Router::new()
            .route(
                "/metrics",
                get(move || {
                    let state = state.clone();
                    async move {
                        (
                            StatusCode::OK,
                            [(
                                axum::http::header::CONTENT_TYPE,
                                "text/plain; version=0.0.4; charset=utf-8",
                            )],
                            crate::metrics::snapshot(&state),
                        )
                    }
                }),
            )
            .route_layer(axum::middleware::from_fn_with_state(
                GuardState {
                    revision,
                    proto_in: "metrics",
                },
                guard_mw,
            ))
    }

    /// What the guard runs with: the revision handle and this route's own
    /// protocol word. The gate itself lives on the **revision** (ADR-040
    /// D5's honesty note — a reload may rename, add or drop
    /// `server.auth_token_env`), and the refusal's record is written by
    /// the captured revision's own trace writer, so a refused request is
    /// stamped with the digest that refused it. The word is a `&str`
    /// (ADR-041 §3.8): the three protocol routes pass their
    /// `WireApi::as_str()` — byte-identical on the wire — and a guarded
    /// non-protocol route (`/metrics`) passes its own word.
    #[derive(Clone)]
    struct GuardState {
        revision: router_proxy::SharedRevision,
        proto_in: &'static str,
    }

    /// One boundary guard pass — and THE capture point of the whole
    /// request path (ADR-040 D2's capture-once rule): the revision in
    /// force at receive is taken here, once, inserted into the request's
    /// extensions, and every later stage (the body bound, the handler)
    /// reads THAT value. Then: read the headers, decide, and on a
    /// refusal write the record **then** answer — the same order the
    /// end-of-request path uses (the observation exists before the
    /// client is told). A trace write failure here is §8's non-blocking
    /// case and does not change the response. A revision with no gate
    /// admits everything (CONF-45 ⑤'s behaviour).
    async fn guard_mw(
        axum::extract::State(st): axum::extract::State<GuardState>,
        mut req: axum::http::Request<axum::body::Body>,
        next: axum::middleware::Next,
    ) -> Response {
        let rev = st.revision.capture();
        req.extensions_mut().insert(rev.clone());
        let headers: Vec<(String, String)> = req
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let verdict = rev.auth_gate.as_ref().map(|gate| gate.admits(&headers));
        match verdict {
            None | Some(router_proxy::AuthVerdict::Admitted) => next.run(req).await,
            Some(verdict @ router_proxy::AuthVerdict::Refused { .. }) => {
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
                    rev.forwarder
                        .trace
                        .as_ref()
                        .map(|t| t.config_digest())
                        .unwrap_or(""),
                );
                // Write, then answer — and either way the client is told
                // 401 (§8's non-blocking case).
                if let Some(t) = rev.forwarder.trace.as_ref() {
                    let _ = t.write(&record);
                }
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

    // One assembly for both auth shapes: the guard layer is always
    // installed (its gate lives on the revision — see the wiring note
    // above), so a reload can introduce, rename or drop
    // `server.auth_token_env` without a restart (ADR-040 D5).
    let app = health_router
        .merge(guarded_protocol_route(
            cell.clone(),
            WireApi::Chat,
            "/v1/chat/completions",
            max_body_bytes,
        ))
        .merge(guarded_protocol_route(
            cell.clone(),
            WireApi::Responses,
            "/v1/responses",
            max_body_bytes,
        ))
        .merge(guarded_protocol_route(
            cell.clone(),
            WireApi::Anthropic,
            "/v1/messages",
            max_body_bytes,
        ))
        .merge(guarded_metrics_route(cell.clone(), state.clone()))
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
    // The reload (ADR-039's mechanism; ADR-040 D1/D2/D5/D10/D11/D12;
    // DESIGN §12.20, landed): default behaviour — no flag, no key, no
    // signal (the owner's standing R44 ruling; D8). The watcher decides
    // when to look, the digest decides whether anything changed, the same
    // loader `serve` started with is the gate, and the Publisher runs
    // steps 3–5 of the sequence: a refused candidate (the loader's, or
    // the publish's own refused-set/auth gates) is reported in exactly
    // one line on stderr and the revision in force keeps serving
    // (D11/RV-9); an accepted candidate is committed as one
    // `config.applied` row (the value diff against its named
    // predecessor, D10), mounted delta-only, and published into the cell
    // the request path captures. The watcher builds and runs on its own
    // thread (reload::Watcher's doc: the backend's start is slow and its
    // teardown blocks, so neither may sit on serve's paths). A watcher
    // that cannot start degrades to the pre-reload behaviour — a change
    // then takes effect on restart, D12.6's named fallback — announced
    // once, never silent: the thread's own setup failure reports through
    // the same sink; a spawn failure is the Err arm here.
    let reload_sink: std::sync::Arc<dyn Fn(String) + Send + Sync> =
        std::sync::Arc::new(|line| eprintln!("{line}"));
    // The publisher borrows the assembly weak (its ownership rule: the
    // watcher's thread is detached, so the strong refs stay here, in
    // serve's own frame — the store's writer lock is released when THIS
    // frame drops, never on that thread's schedule).
    let publisher = std::sync::Arc::new(reload::Publisher::new(
        &cell,
        &store_dyn,
        &trace_sink,
        &rc.identity,
        startup_auth,
        reload_sink.clone(),
    ));
    let _reload_watcher = match reload::Watcher::start(&rc.identity, reload_sink, publisher) {
        Ok(w) => Some(w),
        Err(e) => {
            eprintln!("router: {e}");
            None
        }
    };
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

#[cfg(test)]
mod tests {
    use super::*;

    fn trace_sink(tag: &str) -> std::sync::Arc<router_store::TraceSink> {
        let dir = std::env::temp_dir().join(format!("router-cli-lib-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sink = router_store::TraceSink::open(&dir).expect("a temp trace dir opens");
        let _ = std::fs::remove_dir_all(&dir);
        std::sync::Arc::new(sink)
    }

    /// R43-F4, the served half that keeps working: the one writer wired
    /// into the serving path carries the loader-computed digest, and
    /// every constructor reads exactly that value back off it.
    #[test]
    fn the_served_writer_carries_the_loaders_digest() {
        let w = ConfigTraceWriter::new(trace_sink("f4-ok"), "0123456789abcdef".to_string());
        assert_eq!(
            router_core::TraceWriter::config_digest(&w),
            "0123456789abcdef"
        );
    }

    /// R43-F4, the forgotten case failing loudly: a writer wired into the
    /// serving path that cannot produce a non-empty digest fails AT
    /// CONSTRUCTION, with a message naming the writer — what may never
    /// happen is a record stamped with an identity of `""` (spec §6's
    /// empty-string bullet). The compile-time half of the same rule is
    /// the trait's required method (`router-core/src/trace.rs`): a writer
    /// that never implements `config_digest` does not compile.
    #[test]
    #[should_panic(expected = "ConfigTraceWriter: an empty config_digest on the serving path")]
    fn the_forgotten_digest_fails_at_construction_naming_the_writer() {
        let _ = ConfigTraceWriter::new(trace_sink("f4-empty"), String::new());
    }
}
