use router_core::config::RouterConfig;
use serde_json::{json, Map, Value};

/// What `serve` actually loaded from one config file (CONF-25): built by
/// `router-cli` at startup after validation, read by `/health`.
#[derive(Clone)]
pub struct AppState {
    /// The validated config itself (roster, aliases, fallback, plugins).
    pub config: RouterConfig,
    /// `<config dir>/<trace.dir>` (spec §4.1 resolution rule).
    pub trace_dir: String,
    /// `<config dir>/state/router.db` (spec §4.5; fixed in v0.1).
    pub state_db: String,
    /// For each provider: the `api_key_env` name and whether the env var was
    /// present at startup. Key values never travel here (§12.10.2).
    pub provider_keys: Vec<(String, String, bool)>,
}

/// `/health` reports what was actually loaded (the R2-2b/2c contract,
/// DESIGN §12.10.2): the plugin set with `disabled` shown as disabled, each
/// provider's key presence, the resolved trace/state paths, and the store
/// status — honestly `pending` until R2-2c lands the store (then `open` or
/// the refusal reason, CONF-23).
pub fn health_json(state: &AppState) -> Value {
    let mut plugins = Vec::new();
    for p in &state.config.plugins {
        let mut entry = Map::new();
        entry.insert("id".into(), json!(p.id));
        entry.insert("kind".into(), json!(p.kind));
        if p.disabled {
            entry.insert("disabled".into(), Value::Bool(true));
        }
        plugins.push(Value::Object(entry));
    }

    let providers: Vec<Value> = state
        .provider_keys
        .iter()
        .map(|(name, env, present)| {
            json!({
                "name": name,
                "api_key_env": env,
                "api_key_present": present,
                "available": present,
            })
        })
        .collect();

    json!({
        "status": "ok",
        "addr": state.config.server.addr,
        "plugins": plugins,
        "providers": providers,
        "trace_dir": state.trace_dir,
        "state_db": state.state_db,
        // R2-2c lands the store (trait Store, CONF-23); until then the path
        // is resolved and reported honestly as pending.
        "store": "pending",
    })
}

pub(crate) fn not_implemented_body(
    request_id: String,
    endpoint: &'static str,
) -> router_core::error::ErrorBody {
    router_core::error::ErrorBody::new(
        router_core::error::ErrorCode::NotImplemented,
        format!("endpoint {endpoint} is a stub in this build; forwarding lands in Round 2"),
        request_id,
    )
}
