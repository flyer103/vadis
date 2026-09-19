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
    /// Start the HTTP proxy (R2: config-driven; forwarding lands with the data plane)
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
    use router_core::error::ErrorBody;
    use router_proxy::{protocol_stub, AppState};

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
    use router_core::store::Store as _;
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

    fn error_response(status: StatusCode, body: ErrorBody) -> Response {
        (status, axum::Json(body)).into_response()
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
            post(|| async {
                let (status, body) = protocol_stub(request_id(), "/v1/chat/completions");
                error_response(StatusCode::from_u16(status).expect("valid status"), body)
            }),
        )
        .route(
            "/v1/responses",
            post(|| async {
                let (status, body) = protocol_stub(request_id(), "/v1/responses");
                error_response(StatusCode::from_u16(status).expect("valid status"), body)
            }),
        )
        .route(
            "/v1/messages",
            post(|| async {
                let (status, body) = protocol_stub(request_id(), "/v1/messages");
                error_response(StatusCode::from_u16(status).expect("valid status"), body)
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
