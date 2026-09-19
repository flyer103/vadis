//! `router serve` / `stats` / `replay` / `trace` (DESIGN §12.1). R1-2: serve
//! stub only.

#![forbid(unsafe_code)]

use std::net::SocketAddr;

use clap::{Parser, Subcommand};
use router_proxy::AppState;

#[derive(Parser)]
#[command(name = "router", version, about = "multi-protocol LLM gateway")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the HTTP proxy (R1-2: /health + protocol stubs; no forwarding yet)
    Serve {
        /// Path to the router config file
        #[arg(long)]
        config: String,
    },
}

fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        Command::Serve { config } => tokio::runtime::Runtime::new()
            .expect("tokio runtime")
            .block_on(serve(&config)),
    };
    std::process::exit(code);
}

async fn serve(_config_path: &str) -> i32 {
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use axum::routing::{get, post};
    use axum::Json;
    use router_core::error::ErrorBody;
    use router_proxy::protocol_stub;

    // Config parsing lands in R2 (DESIGN §12.5): a load failure exits with a
    // §12.5 error; with a missing/bad config, /health must still come up
    // (orchestrator ruling: config errors are reported separately).
    let addr: SocketAddr = "127.0.0.1:8790".parse().expect("static listen addr parses");
    let state = std::sync::Arc::new(AppState {
        addr: addr.to_string(),
        plugins: vec![
            "builtin/cache_guard".into(),
            "builtin/transform_rules".into(),
            "builtin/cost_ledger".into(),
            "builtin/quota_guard".into(),
            "builtin/sticky".into(),
        ],
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

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("bind listen addr");
    eprintln!("router listening on {addr}");
    axum::serve(listener, app).await.expect("axum serve");
    0
}

fn request_id() -> String {
    // R2 switches to uuid; a process-local increment is enough for the stub
    // phase — no new dependency.
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("req-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}
