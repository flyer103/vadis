//! Library surface of the CLI crate so conformance cases can drive the
//! real `serve` assembly (CONF-25) without spawning a process.

#![forbid(unsafe_code)]

pub mod config_load;
pub mod config_path;
pub mod metrics;
pub mod reload;
pub mod setup;
pub mod stats;
pub mod steps;
pub mod wrap;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "vadis", version, about = "multi-protocol LLM gateway")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Start the HTTP proxy (config-driven)
    Serve {
        /// Path to the vadis config file; absent, found by spec §4.12's
        /// discovery order (--config > the XDG location > ./config.yaml,
        /// else a refusal naming `vadis setup`)
        #[arg(long)]
        config: Option<String>,
    },
    /// Report the window's figures from the trace + event log (spec §9.2)
    Stats {
        /// Path to the vadis config file (resolves `trace.dir`); absent,
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
    /// One line per request of one session, from the trace files only
    /// (spec §9.4): what each step sent, what the provider's cache
    /// said, which account paid, and which block broke the prefix
    Steps {
        /// Path to the vadis config file (resolves `trace.dir` and the
        /// roster's `account:` entries). **Required** (spec §9.4.1): no
        /// discovery order — a refusal, never a silent default
        #[arg(long)]
        config: String,
        /// The session key, `identity.session` verbatim (the client's
        /// `prompt_cache_key`). Required: an unkeyed request belongs to
        /// no session and the command cannot invent one
        #[arg(long)]
        session: String,
        /// Print only the last <n> steps; the header says so
        #[arg(long)]
        last: Option<usize>,
        /// Emit the machine-readable form (the same fields, same names)
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
    /// Launch a client with the transform-mode header supplied through the
    /// client's own configuration mechanism (spec §2.1: the request must
    /// ask; wrap makes the client ask, it never makes vadis decide)
    Wrap {
        /// The client to launch: codex | claude (the message on an unknown
        /// name lists the supported set)
        #[arg(value_name = "CLIENT")]
        client: String,
        /// The client's own args, verbatim — only collected after `--`,
        /// so a leading `-c`/flag is never mistaken for vadis's
        #[arg(last = true, value_name = "ARGS")]
        args: Vec<String>,
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
    inner: std::sync::Arc<vadis_store::TraceSink>,
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
    fn new(inner: std::sync::Arc<vadis_store::TraceSink>, config_digest: String) -> Self {
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

impl vadis_core::TraceWriter for ConfigTraceWriter {
    fn write(&self, rec: &vadis_core::DecisionRecord) -> Result<Option<String>, String> {
        vadis_core::TraceWriter::write(self.inner.as_ref(), rec)
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
    store: &std::sync::Arc<dyn vadis_core::store::Store>,
    trace_sink: &std::sync::Arc<vadis_store::TraceSink>,
    auth_gate: Option<vadis_proxy::AuthGate>,
    note: &dyn Fn(String),
) -> vadis_proxy::Revision {
    use std::collections::HashMap;

    // Key presence is probed at the revision's build; the values
    // themselves are never read here (the provider client holds them,
    // §12.10.1) and never reported — only their presence travels to
    // /health, beside each entry's declared region and currency
    // (spec §4.8). ADR-049 §3 rule 2: the pool is probed per name, and
    // the provider-level fact is the pool's disjunction.
    let provider_keys: Vec<vadis_proxy::ProviderKeyFacts> = rc
        .vadis
        .providers
        .iter()
        .map(|p| {
            let pool = p.key_pool();
            let facts: Vec<bool> = pool.iter().map(|n| std::env::var_os(n).is_some()).collect();
            let present = facts.iter().any(|&b| b);
            vadis_proxy::ProviderKeyFacts {
                name: p.name.clone(),
                // The one-key spelling's own facts (ADR-049 §7(a)): the
                // first declared name, whatever spelling was written —
                // so the existing members keep their exact meaning.
                api_key_env: pool.first().cloned().unwrap_or_default(),
                present,
                region: p.region,
                currency: p.currency,
                keys: (p.api_keys.is_some()).then(|| {
                    pool.iter()
                        .zip(facts.iter())
                        .map(|(n, &b)| vadis_proxy::KeyFact {
                            env: n.clone(),
                            present: b,
                        })
                        .collect()
                }),
            }
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
    // ADR-049 §3 rule 7: the values are read once here, all of them —
    // one per **present** pool name, in declaration order (the rotation
    // order). Held only by the engine, never in /health, logs or events;
    // a provider with no present name gets no transport and no entry.
    let mut transports: HashMap<String, std::sync::Arc<dyn vadis_proxy::ProviderTransport>> =
        HashMap::new();
    let mut api_keys: HashMap<String, Vec<String>> = HashMap::new();
    for p in &rc.vadis.providers {
        let pool: Vec<String> = p
            .key_pool()
            .iter()
            .filter_map(|n| std::env::var_os(n).map(|v| v.to_string_lossy().into_owned()))
            .collect();
        if pool.is_empty() {
            continue;
        }
        let timeout = std::time::Duration::from_millis(rc.vadis.server.upstream_attempt_timeout.0);
        match vadis_providers::ReqwestProviderClient::new(timeout) {
            Ok(client) => {
                transports.insert(p.name.clone(), std::sync::Arc::new(client));
                api_keys.insert(p.name.clone(), pool);
            }
            Err(e) => {
                note(format!("provider '{}': client build failed: {e}", p.name));
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
    let assembly = vadis_plugins::assemble(&rc.vadis.plugins, &|file| {
        config_load::resolve(&rc.config_dir, file)
    });
    for line in assembly.notes() {
        note(line.clone());
    }
    let transform_engine = assembly.transform_engine();
    // The exact-match response cache (spec §4.17): `Some` only when the
    // operator mounted `builtin/response_cache` with `config.enabled:
    // true` — an absent or inert entry assembles to `None`, and the
    // serving path then never knows the kind exists (CONF-88). A config
    // reload re-runs `assemble`, so the store's lifetime is the fiber's
    // (ADR-042 §2.2): a changed entry unloads the store with the fiber.
    let response_cache = assembly.response_cache();

    // The revision's trace writer: the process's one sink (shared Arc),
    // stamped with THIS revision's digest — a request finishing after a
    // switch is still stamped by the revision that served it (D3).
    let trace_writer: std::sync::Arc<dyn vadis_core::TraceWriter> = std::sync::Arc::new(
        ConfigTraceWriter::new(trace_sink.clone(), rc.identity.config_digest.clone()),
    );

    let forwarder = vadis_proxy::Forwarder {
        config: rc.vadis.clone(),
        transports,
        api_keys,
        store: Some(store.clone()),
        trace: Some(trace_writer),
        transform_engine,
        response_cache,
        // The session TTL's one ms → µs conversion (spec §4.5, note R6):
        // `session.ttl` is a `DurationVal` in **milliseconds**, every
        // consumer of `session_ttl_us` reads **microseconds** — so the
        // factor is × 1 000, applied once here at the resolution site,
        // never in a consumer (R21-F6: the shipped v0.1 multiplied by
        // 1 000 000, honouring a configured 12h as ~12 000 h). The
        // saturation shape is the in-tree owner's
        // (`PlanPolicyCfg::cooldown_us`): `try_from` + saturating, not a
        // plain `as` cast, which would wrap u64 → i64 negative.
        session_ttl_us: i64::try_from(rc.vadis.session.ttl.0)
            .unwrap_or(i64::MAX)
            .saturating_mul(1_000),
    };

    // The tools surface (spec §4.23, ADR-066): `tools:` names one provider,
    // and the four verbs dial that entry's own tool URLs. The mount is built
    // here, once per publish, so the serving path reads one immutable value —
    // the same discipline as every other per-revision fact (DESIGN §12.20).
    // The section's absence is a statement ("this deployment serves no tool
    // verb"), which is why it is an `Unserved` with the sentence that says so
    // rather than a silently missing route.
    let tools = match &rc.vadis.tools {
        None => vadis_proxy::ToolsMount::Unserved(
            "the config writes no tools: section, so /v1/tools/* is not served here (spec §4.23)"
                .to_string(),
        ),
        Some(cfg) => match rc.vadis.providers.iter().find(|p| p.name == cfg.provider) {
            // validate() refuses an unknown tools.provider at load, so this
            // arm is unreachable from a served config; it stays a refusal
            // rather than an unwrap because the build must not panic on the
            // serving path even when a rule it relied on was bypassed.
            None => vadis_proxy::ToolsMount::Unserved(format!(
                "tools.provider '{}' names no providers entry (spec §4.23)",
                cfg.provider
            )),
            Some(provider) => {
                // ADR-049 §3 rule 7's read-once discipline, one surface over:
                // the values are read at publish, never on the request path.
                let keys: Vec<String> = provider
                    .key_pool()
                    .iter()
                    .filter_map(|n| std::env::var_os(n).map(|v| v.to_string_lossy().into_owned()))
                    .collect();
                let transport: Option<std::sync::Arc<dyn vadis_proxy::ProviderTransport>> =
                    if keys.is_empty() {
                        None
                    } else {
                        match vadis_providers::ReqwestProviderClient::new(
                            std::time::Duration::from_millis(cfg.attempt_timeout.0),
                        ) {
                            Ok(client) => Some(std::sync::Arc::new(client)),
                            Err(e) => {
                                note(format!("tools: provider client build failed: {e}"));
                                None
                            }
                        }
                    };
                vadis_proxy::ToolsMount::from_config(cfg, provider, keys, transport)
            }
        },
    };

    vadis_proxy::Revision {
        forwarder,
        // Spec §9.1's `config` member: the loader-computed identity,
        // handed to the proxy as strings (ADR-037 D4: the proxy never
        // opens, resolves or hashes a config file).
        config_identity: vadis_proxy::ConfigIdentity {
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
        tools,
    }
}

/// `vadis serve`: load the config (exit code 2 + reason on any failure),
/// then serve exactly what it says. Returns the process exit code.
pub async fn serve(config_path: &str) -> i32 {
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use axum::routing::{get, post};
    use axum::Json;
    use vadis_core::config::{ToolKind, WireApi};
    use vadis_core::error::ErrorBody;

    use vadis_proxy::{AppState, ForwardOutcome};

    // Load-time contract (DESIGN §12.10.2): an illegal config exits
    // non-zero with the reason — never a silent fallback to defaults.
    let rc = match config_load::load(std::path::Path::new(config_path)) {
        Ok(rc) => rc,
        Err(reason) => {
            eprintln!("vadis: {reason}");
            return 2;
        }
    };

    let addr = match rc.vadis.listen_addr() {
        Ok(a) => a,
        Err(e) => {
            // validate() already ran in load(); unreachable in practice.
            eprintln!("vadis: config file {config_path}: {e}");
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
    let auth_token: Option<String> = match &rc.vadis.server.auth_token_env {
        Some(name) => match std::env::var(name) {
            Ok(v) if !v.is_empty() => Some(v),
            other => {
                let state = match other {
                    Ok(_) => "empty",
                    Err(_) => "unset",
                };
                eprintln!(
                    "vadis: config file {config_path}: server.auth_token_env names {name}, \
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
    let store = match vadis_store::SqliteStore::open(&rc.state_db) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("vadis: state store {}: {e}", rc.state_db.display());
            return 4;
        }
    };
    // One shared handle: the Forwarder (writes + reads) and /health's plan
    // section (reads, spec §9.1) go through the same single-connection
    // store behind its mutex.
    let store_dyn: std::sync::Arc<dyn vadis_core::store::Store> = std::sync::Arc::new(store);
    // config.applied is an intent/accounting event (ADR-010 item 2): FULL,
    // committed before the process starts serving on top of it.
    let _config_applied = store_dyn.append(vadis_core::NewEvent {
        kind: vadis_core::EventKind::ConfigApplied,
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
    // vadis that cannot write its analysis truth must not serve (same
    // line as CONF-23a; per-request failures are the non-blocking §8
    // case, handled inside the forwarding path). The sink is
    // process-level and shared (`Arc`): the reload refuses a `trace.dir`
    // change (ADR-040 D5), so every revision's writer sits over this one
    // sink, stamped with that revision's digest.
    let trace_sink = match vadis_store::TraceSink::open(&rc.trace_dir) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("vadis: trace dir {}: {e}", rc.trace_dir.display());
            return 4;
        }
    };
    let trace_sink = std::sync::Arc::new(trace_sink);

    // The startup revision, built by exactly the assembly the reload's
    // publish uses (ONE assembly — startup and a switch cannot drift
    // apart), then published into the cell the request path captures once
    // per request (ADR-040 D2/D3; DESIGN §12.20's seam).
    let startup_gate = auth_token.clone().map(vadis_proxy::AuthGate::new);
    // §4.7's "read once" facts, handed to the publisher: a revision
    // naming the same variable reuses this read; a renamed one is read
    // at the switch (ADR-040 D5's honesty note).
    let startup_auth: Option<(String, String)> = rc
        .vadis
        .server
        .auth_token_env
        .clone()
        .zip(auth_token.clone());
    let revision = {
        let note = |line: String| eprintln!("vadis: {line}");
        build_revision(&rc, &store_dyn, &trace_sink, startup_gate, &note)
    };
    let cell = vadis_proxy::RevisionCell::new(revision);

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
    /// vadis-owned keys) — never parsed and reserialized.
    async fn proxy_endpoint(
        rev: std::sync::Arc<vadis_proxy::Revision>,
        proto_in: WireApi,
        headers: Vec<(String, String)>,
        client: &'static str,
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
        let transform_mode = match vadis_proxy::resolve_transform_mode(&headers) {
            Ok(m) => m,
            Err(f) => {
                // Write, then answer — the same order the auth guard uses
                // (the observation exists before the client is told); a
                // trace write failure is §8's non-blocking case.
                let now_us = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_micros() as i64)
                    .unwrap_or(0);
                let record = vadis_proxy::mode_refused_record(
                    &request_id,
                    proto_in,
                    &f,
                    now_us,
                    0,
                    forwarder
                        .trace
                        .as_ref()
                        .map(|t| t.config_digest())
                        .unwrap_or(""),
                    client,
                );
                if let Some(t) = forwarder.trace.as_ref() {
                    let _ = t.write(&record);
                }
                let mut eb = ErrorBody::new(f.code, f.message, &request_id);
                eb.error.details = f.details;
                let mut resp =
                    (StatusCode::from_u16(f.status).expect("400 maps"), Json(eb)).into_response();
                if let Ok(v) = request_id.parse() {
                    resp.headers_mut().insert("x-vadis-request-id", v);
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
                .forward_stream_with_client(
                    proto_in,
                    &body,
                    &request_id,
                    &headers,
                    transform_mode,
                    client,
                )
                .await
            {
                vadis_proxy::StreamOutcome::Success(s) => {
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
                        resp.headers_mut().insert("x-vadis-request-id", v);
                    }
                    resp
                }
                vadis_proxy::StreamOutcome::Failure(f) => {
                    let details = f.client_body_details();
                    let mut body = ErrorBody::new(f.code, f.message, request_id);
                    // ADR-061 D2: the client body carries the resolution
                    // failure's resolvable set; the trace record does not.
                    body.error.details = details;
                    (
                        StatusCode::from_u16(f.status).expect("failure status maps"),
                        Json(body),
                    )
                        .into_response()
                }
            };
        }
        let outcome = forwarder
            .forward_with_client(
                proto_in,
                &body,
                &request_id,
                &headers,
                transform_mode,
                client,
            )
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
                        resp.headers_mut().insert("x-vadis-failover-from", v);
                    }
                }
                resp
            }
            ForwardOutcome::Failure(f) => {
                let details = f.client_body_details();
                let mut body = ErrorBody::new(f.code, f.message, request_id);
                // ADR-061 D2: the client body carries the resolution
                // failure's resolvable set; the trace record does not.
                body.error.details = details;
                (
                    StatusCode::from_u16(f.status).expect("failure status maps"),
                    Json(body),
                )
                    .into_response()
            }
        }
    }

    // The wiring (§12.11): each protocol route is its own vadis so it
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
                let body = vadis_proxy::health_json(&state);
                async move { (StatusCode::OK, Json(body)).into_response() }
            }
        }),
    );

    fn protocol_route(
        revision: vadis_proxy::SharedRevision,
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
                        std::sync::Arc<vadis_proxy::Revision>,
                    >,
                          axum::extract::Extension(client_word): axum::extract::Extension<
                        ClientWord,
                    >,
                          axum::extract::Extension(body): axum::extract::Extension<
                        axum::body::Bytes,
                    >| {
                        let hs: Vec<(String, String)> = headers
                            .iter()
                            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                            .collect();
                        proxy_endpoint(rev, proto_in, hs, client_word.0, body)
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
        revision: vadis_proxy::SharedRevision,
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
            .get::<std::sync::Arc<vadis_proxy::Revision>>()
            .cloned()
            .unwrap_or_else(|| st.revision.capture());
        let content_length = req
            .headers()
            .get(axum::http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());
        // The guard (outermost) derived the word; the bound runs inside
        // it (ADR-061 D3's "once") — read it before the closure so the
        // request can still move on into the pipeline.
        let client = client_word(req.extensions());
        let refuse = |declared: Option<u64>| {
            let request_id = request_id();
            let now_us = std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .map(|d| d.as_micros() as i64)
                .unwrap_or(0);
            let record = vadis_proxy::too_large_record(
                &request_id,
                st.proto_in,
                st.limit,
                declared,
                now_us,
                started.elapsed().as_millis() as u32,
                rev.forwarder
                    .trace
                    .as_ref()
                    .map(|t| t.config_digest())
                    .unwrap_or(""),
                // The guard (outermost) derived the word; the bound runs
                // inside it (ADR-061 D3's "once").
                client,
            );
            // Write, then answer — the same order the auth guard uses;
            // a trace write failure is §8's non-blocking case.
            if let Some(t) = rev.forwarder.trace.as_ref() {
                let _ = t.write(&record);
            }
            let mut eb = ErrorBody::new(
                vadis_core::error::ErrorCode::RequestTooLarge,
                vadis_proxy::too_large_message(st.limit, declared),
                &request_id,
            );
            eb.error.details = Some(serde_json::json!({
                "limit_bytes": st.limit,
                "content_length": declared,
            }));
            let mut resp = (
                StatusCode::from_u16(vadis_core::error::ErrorCode::RequestTooLarge.http_status())
                    .expect("413 maps"),
                Json(eb),
            )
                .into_response();
            if let Ok(v) = request_id.parse() {
                resp.headers_mut().insert("x-vadis-request-id", v);
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
        if let Err(declared) = vadis_proxy::check_declared(st.limit, content_length) {
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
        revision: vadis_proxy::SharedRevision,
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
        revision: vadis_proxy::SharedRevision,
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

    /// `GET /v1/models` (spec §4.19, ADR-055, DESIGN §12.8): the model
    /// list surface — the configured alias names, and nothing else. The
    /// fourth guarded non-protocol route: same guard middleware as
    /// `/metrics`, its own word `"models"`, and — like `/metrics` — no
    /// body extractor, no body layer, no request byte read. The handler
    /// reads the revision the guard already captured (ADR-040 D2's
    /// capture-once rule: the extensions copy, never a second cell
    /// read), so the body follows the revision that admitted the call,
    /// reload included. The body itself is the pure builder in
    /// `vadis-proxy`'s `models_read`. The admitted arm writes nothing.
    fn guarded_models_route(revision: vadis_proxy::SharedRevision) -> axum::Router {
        axum::Router::new()
            .route(
                "/v1/models",
                get(
                    |axum::Extension(rev): axum::Extension<
                        std::sync::Arc<vadis_proxy::Revision>,
                    >| async move {
                        (
                            StatusCode::OK,
                            [(axum::http::header::CONTENT_TYPE, "application/json")],
                            vadis_proxy::models_body_from_config(&rev.forwarder.config),
                        )
                    },
                ),
            )
            .route_layer(axum::middleware::from_fn_with_state(
                GuardState {
                    revision,
                    proto_in: "models",
                },
                guard_mw,
            ))
    }

    /// `POST /v1/tools/{ocr,transcribe,synthesize}` (spec §4.22, ADR-065):
    /// the media tool surface — the paths a client calls when it wants an
    /// answer *about* a file rather than a chat completion. The fifth
    /// guarded non-protocol route family: the same guard middleware as
    /// `/metrics`, its own word `"media"`, and its own body bound
    /// (`media.max_body_bytes`) because base64 media is megabytes while the
    /// protocol routes stay text-sized. Each path answers a **verb**; the
    /// request the pipeline forwards is synthesized in `vadis-proxy`'s
    /// `media_tools`, which is also where the answer is parsed back into
    /// `{"text": ...}`. A config with no `media:` section serves `501
    /// not_implemented` on all three (§4.22: writing nothing is the operator
    /// saying the surface is off), and so do the two audio verbs even when
    /// `media:` is written — vadis has no audio route kind, and an honest 501
    /// is the contract (§8), not a silent 404.
    fn guarded_media_tools_route(revision: vadis_proxy::SharedRevision) -> axum::Router {
        axum::Router::new()
            .route(
                "/v1/tools/ocr",
                post(
                    |axum::extract::Extension(rev): axum::extract::Extension<
                        std::sync::Arc<vadis_proxy::Revision>,
                    >,
                     axum::extract::Extension(client): axum::extract::Extension<ClientWord>,
                     headers: axum::http::HeaderMap,
                     body: axum::body::Body| async move {
                        media_tool_endpoint(rev, "ocr", headers, body, client).await
                    },
                ),
            )
            .route(
                "/v1/tools/transcribe",
                post(
                    |axum::extract::Extension(rev): axum::extract::Extension<
                        std::sync::Arc<vadis_proxy::Revision>,
                    >,
                     axum::extract::Extension(client): axum::extract::Extension<ClientWord>,
                     headers: axum::http::HeaderMap,
                     body: axum::body::Body| async move {
                        media_tool_endpoint(rev, "transcribe", headers, body, client).await
                    },
                ),
            )
            .route(
                "/v1/tools/synthesize",
                post(
                    |axum::extract::Extension(rev): axum::extract::Extension<
                        std::sync::Arc<vadis_proxy::Revision>,
                    >,
                     axum::extract::Extension(client): axum::extract::Extension<ClientWord>,
                     headers: axum::http::HeaderMap,
                     body: axum::body::Body| async move {
                        media_tool_endpoint(rev, "synthesize", headers, body, client).await
                    },
                ),
            )
            .layer(axum::extract::DefaultBodyLimit::disable())
            .route_layer(axum::middleware::from_fn_with_state(
                GuardState {
                    revision,
                    proto_in: "media",
                },
                guard_mw,
            ))
    }

    /// `POST /v1/tools/{search,fetch,tts,asr}` (spec §4.23, ADR-066): the
    /// provider tool surface — the paths a caller uses when it wants *an
    /// action performed upstream* (a search, a page, speech, a transcript)
    /// rather than a model opinion about bytes it already holds. The sixth
    /// guarded non-protocol route family: the same guard middleware as
    /// `/metrics` and `/v1/tools/*`'s media verbs, its own word `"tools"`,
    /// and its own body bound (`tools.max_body_bytes`).
    ///
    /// Unlike the media verbs, nothing is synthesized or parsed here: the
    /// client's body and content type travel **byte-for-byte** to the
    /// provider's endpoint (a rewritten multipart boundary makes an `asr`
    /// upload unparsable), and the upstream's status, content type and bytes
    /// come back unchanged — an upstream `4xx` is relayed, never re-shaped
    /// into §8's envelope (spec §4.23's error rule). vadis's own refusals
    /// (unserved verb, oversized body, no answer at all) are §8's, and each
    /// one is written to the trace in the same breath as the answer.
    fn guarded_tools_route(revision: vadis_proxy::SharedRevision) -> axum::Router {
        axum::Router::new()
            .route(
                "/v1/tools/search",
                post(
                    |axum::extract::Extension(rev): axum::extract::Extension<
                        std::sync::Arc<vadis_proxy::Revision>,
                    >,
                     axum::extract::Extension(client): axum::extract::Extension<ClientWord>,
                     headers: axum::http::HeaderMap,
                     body: axum::body::Body| async move {
                        tools_endpoint(rev, ToolKind::Search, headers, body, client).await
                    },
                ),
            )
            .route(
                "/v1/tools/fetch",
                post(
                    |axum::extract::Extension(rev): axum::extract::Extension<
                        std::sync::Arc<vadis_proxy::Revision>,
                    >,
                     axum::extract::Extension(client): axum::extract::Extension<ClientWord>,
                     headers: axum::http::HeaderMap,
                     body: axum::body::Body| async move {
                        tools_endpoint(rev, ToolKind::Fetch, headers, body, client).await
                    },
                ),
            )
            .route(
                "/v1/tools/tts",
                post(
                    |axum::extract::Extension(rev): axum::extract::Extension<
                        std::sync::Arc<vadis_proxy::Revision>,
                    >,
                     axum::extract::Extension(client): axum::extract::Extension<ClientWord>,
                     headers: axum::http::HeaderMap,
                     body: axum::body::Body| async move {
                        tools_endpoint(rev, ToolKind::Tts, headers, body, client).await
                    },
                ),
            )
            .route(
                "/v1/tools/asr",
                post(
                    |axum::extract::Extension(rev): axum::extract::Extension<
                        std::sync::Arc<vadis_proxy::Revision>,
                    >,
                     axum::extract::Extension(client): axum::extract::Extension<ClientWord>,
                     headers: axum::http::HeaderMap,
                     body: axum::body::Body| async move {
                        tools_endpoint(rev, ToolKind::Asr, headers, body, client).await
                    },
                ),
            )
            .layer(axum::extract::DefaultBodyLimit::disable())
            .route_layer(axum::middleware::from_fn_with_state(
                GuardState {
                    revision,
                    proto_in: "tools",
                },
                guard_mw,
            ))
    }

    /// One tool call (§4.23): refuse (unserved verb, oversized body, no
    /// answer) or relay the upstream's own answer, and write exactly one §6
    /// row either way — a call whose bytes left is never silent, and a
    /// refusal is never a bare 404.
    async fn tools_endpoint(
        rev: std::sync::Arc<vadis_proxy::Revision>,
        verb: ToolKind,
        headers: axum::http::HeaderMap,
        body: axum::body::Body,
        client: ClientWord,
    ) -> Response {
        let request_id = request_id();
        let rid = request_id.as_str();
        let started = std::time::Instant::now();
        let client = client.0;
        let digest = rev
            .forwarder
            .trace
            .as_ref()
            .map(|t| t.config_digest())
            .unwrap_or("");
        let write = |status: u16,
                     provider: &str,
                     upstream_status: Option<u16>,
                     request_bytes: u64,
                     response_bytes: u64,
                     error: Option<(vadis_core::error::ErrorCode, String)>| {
            let now_us = std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .map(|d| d.as_micros() as i64)
                .unwrap_or(0);
            let record = vadis_proxy::tool_record(&vadis_proxy::ToolCallFacts {
                request_id: rid,
                verb: verb.as_str(),
                provider,
                status,
                upstream_status,
                request_bytes,
                response_bytes,
                now_us,
                overhead_ms: started.elapsed().as_millis().min(u32::MAX as u128) as u32,
                upstream_ms: 0,
                config_digest: digest,
                client,
                error,
            });
            if let Some(t) = rev.forwarder.trace.as_ref() {
                let _ = t.write(&record);
            }
        };
        let surface = match &rev.tools {
            vadis_proxy::ToolsMount::Served(surface) => surface.clone(),
            vadis_proxy::ToolsMount::Unserved(message) => {
                let message = message.clone();
                write(
                    vadis_core::error::ErrorCode::NotImplemented.http_status(),
                    "",
                    None,
                    0,
                    0,
                    Some((
                        vadis_core::error::ErrorCode::NotImplemented,
                        message.clone(),
                    )),
                );
                return media_error(
                    rid,
                    vadis_core::error::ErrorCode::NotImplemented,
                    message,
                    None,
                );
            }
        };
        let provider = surface.provider().to_string();
        if surface.verb_url(verb).is_none() {
            let message = format!(
                "provider '{provider}' declares no tools.{} URL: this deployment does not \
                 serve the verb (spec §4.23)",
                verb.as_str()
            );
            write(
                vadis_core::error::ErrorCode::NotImplemented.http_status(),
                &provider,
                None,
                0,
                0,
                Some((
                    vadis_core::error::ErrorCode::NotImplemented,
                    message.clone(),
                )),
            );
            return media_error(
                rid,
                vadis_core::error::ErrorCode::NotImplemented,
                message,
                None,
            );
        }
        let limit = surface.max_body_bytes();
        // §4.13's header-first rule, on this surface's own bound: a declared
        // length over it is refused before a byte is read, so an `asr` upload
        // of the wrong size costs the caller nothing but the header.
        let declared: Option<u64> = headers
            .get(axum::http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok());
        if let Some(len) = declared {
            if len > limit as u64 {
                let message = format!(
                    "request body exceeds tools.max_body_bytes: {len} bytes declared, the \
                     configured bound is {limit}"
                );
                write(
                    vadis_core::error::ErrorCode::RequestTooLarge.http_status(),
                    &provider,
                    None,
                    0,
                    0,
                    Some((
                        vadis_core::error::ErrorCode::RequestTooLarge,
                        message.clone(),
                    )),
                );
                return media_error(
                    rid,
                    vadis_core::error::ErrorCode::RequestTooLarge,
                    message,
                    Some(serde_json::json!({
                        "bound": "tools.max_body_bytes",
                        "limit_bytes": limit,
                        "content_length": declared,
                    })),
                );
            }
        }
        let raw = match axum::body::to_bytes(body, limit).await {
            Ok(bytes) => bytes,
            Err(_) => {
                let message = format!(
                    "request body exceeds tools.max_body_bytes: the configured bound is \
                     {limit}"
                );
                write(
                    vadis_core::error::ErrorCode::RequestTooLarge.http_status(),
                    &provider,
                    None,
                    0,
                    0,
                    Some((
                        vadis_core::error::ErrorCode::RequestTooLarge,
                        message.clone(),
                    )),
                );
                return media_error(
                    rid,
                    vadis_core::error::ErrorCode::RequestTooLarge,
                    message,
                    Some(serde_json::json!({
                        "bound": "tools.max_body_bytes",
                        "limit_bytes": limit,
                        "content_length": serde_json::Value::Null,
                    })),
                );
            }
        };
        let request_bytes = raw.len() as u64;
        // The caller's content type travels as received: for `asr` it is the
        // multipart boundary's only home, and a value rewritten here would be
        // a body the upstream cannot parse. Its absence is sent as absence —
        // vadis does not invent one.
        let content_type = headers
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_string());
        let request = match surface.prepare(verb, content_type.as_deref(), raw) {
            Ok(request) => request,
            Err(message) => {
                write(
                    vadis_core::error::ErrorCode::UpstreamError.http_status(),
                    &provider,
                    None,
                    request_bytes,
                    0,
                    Some((vadis_core::error::ErrorCode::UpstreamError, message.clone())),
                );
                return media_error(
                    rid,
                    vadis_core::error::ErrorCode::UpstreamError,
                    message,
                    None,
                );
            }
        };
        let upstream_started = std::time::Instant::now();
        let outcome = surface.send(request).await;
        let upstream_ms = upstream_started.elapsed().as_millis().min(u32::MAX as u128) as u32;
        let (status, upstream_status, answer, answer_type) = match outcome {
            vadis_providers::AttemptOutcome::Responded(response) => {
                if response.body.len() > surface.max_response_bytes() {
                    let message = format!(
                        "the upstream answer is {} bytes, above tools.max_response_bytes {}: \
                         a truncated relay is worse than a refusal (spec §4.23)",
                        response.body.len(),
                        surface.max_response_bytes()
                    );
                    write(
                        vadis_core::error::ErrorCode::UpstreamError.http_status(),
                        &provider,
                        Some(response.status),
                        request_bytes,
                        response.body.len() as u64,
                        Some((vadis_core::error::ErrorCode::UpstreamError, message.clone())),
                    );
                    return media_error(
                        rid,
                        vadis_core::error::ErrorCode::UpstreamError,
                        message,
                        Some(serde_json::json!({
                            "bound": "tools.max_response_bytes",
                            "limit_bytes": surface.max_response_bytes(),
                            "upstream_status": response.status,
                        })),
                    );
                }
                (
                    response.status,
                    Some(response.status),
                    response.body,
                    response.content_type,
                )
            }
            vadis_providers::AttemptOutcome::NotSent(e)
            | vadis_providers::AttemptOutcome::WrittenNoResponse(e) => {
                let message = format!(
                    "no answer from the tools endpoint: {} ({})",
                    e.message,
                    if e.wrote_full_request {
                        "the request was fully written, so the call may have been billed"
                    } else {
                        "no request byte left this process"
                    }
                );
                write(
                    vadis_core::error::ErrorCode::UpstreamError.http_status(),
                    &provider,
                    None,
                    request_bytes,
                    0,
                    Some((vadis_core::error::ErrorCode::UpstreamError, message.clone())),
                );
                return media_error(
                    rid,
                    vadis_core::error::ErrorCode::UpstreamError,
                    message,
                    None,
                );
            }
        };
        let response_bytes = answer.len() as u64;
        // The relayed upstream error is still a call that produced no answer;
        // the row carries it so a reader of `result.status` is not left to
        // guess, and the client receives the upstream's own body untouched
        // (spec §4.23: upstream errors are relayed, not re-shaped).
        let error = (status >= 400).then(|| {
            (
                vadis_core::error::ErrorCode::UpstreamError,
                format!("upstream answered HTTP {status}"),
            )
        });
        // The one row this call writes: the relayed status is the record's
        // `result.status` (it is what the caller received), and `upstream_ms`
        // is the dial's own time — the two times a tool call has, since nothing
        // was decided and nothing was transformed.
        let now_us = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map(|d| d.as_micros() as i64)
            .unwrap_or(0);
        let record = vadis_proxy::tool_record(&vadis_proxy::ToolCallFacts {
            request_id: rid,
            verb: verb.as_str(),
            provider: &provider,
            status,
            upstream_status,
            request_bytes,
            response_bytes,
            now_us,
            overhead_ms: started.elapsed().as_millis().min(u32::MAX as u128) as u32,
            upstream_ms,
            config_digest: digest,
            client,
            error,
        });
        if let Some(t) = rev.forwarder.trace.as_ref() {
            let _ = t.write(&record);
        }
        let mut relayed = axum::response::Response::new(axum::body::Body::from(answer));
        *relayed.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
        if let Some(value) = answer_type.as_deref().and_then(|v| v.parse().ok()) {
            relayed
                .headers_mut()
                .insert(axum::http::header::CONTENT_TYPE, value);
        }
        if let Ok(v) = request_id.parse() {
            relayed.headers_mut().insert("x-vadis-request-id", v);
        }
        relayed
    }
    /// §8's answer with the request id header, for the arms that never reach
    /// the pipeline.
    fn media_error(
        request_id: &str,
        code: vadis_core::error::ErrorCode,
        message: String,
        details: Option<serde_json::Value>,
    ) -> Response {
        let status = StatusCode::from_u16(code.http_status()).expect("media status maps");
        let mut body = ErrorBody::new(code, message, request_id);
        body.error.details = details;
        let mut resp = (status, Json(body)).into_response();
        if let Ok(v) = request_id.parse() {
            resp.headers_mut().insert("x-vadis-request-id", v);
        }
        // §4.13's reasoning holds on this surface too: a refused request's
        // body may not have been drained, and a keep-alive connection
        // holding unread request bytes would parse them as the next request.
        if code == vadis_core::error::ErrorCode::RequestTooLarge {
            resp.headers_mut().insert(
                axum::http::header::CONNECTION,
                axum::http::HeaderValue::from_static("close"),
            );
        }
        resp
    }

    /// One media call (§4.22): the 501/invalid/too-large arms answer before
    /// the pipeline; the admitted arm synthesizes the chat request, forwards
    /// it as an ordinary Chat call (so accounting, tracing, failover and the
    /// roster's own routing are the pipeline's, not a second code path's)
    /// and parses the answer back into `{"text": ...}`.
    async fn media_tool_endpoint(
        rev: std::sync::Arc<vadis_proxy::Revision>,
        tool: &'static str,
        headers: axum::http::HeaderMap,
        body: axum::body::Body,
        client: ClientWord,
    ) -> Response {
        let request_id = request_id();
        let rid = request_id.as_str();
        let started = std::time::Instant::now();
        let client = client.0;
        // Spec §4.22: a refusal answered BEFORE the pipeline writes the §6
        // pre-pipeline record — no model named, no route selected, no byte
        // read from an upstream — so the client's answer and the trace agree
        // about which rule refused the call. A refusal AFTER the pipeline (a
        // non-2xx upstream, an unparsable answer) writes nothing extra: the
        // forward already wrote the ordinary chat row.
        let refuse = |code: vadis_core::error::ErrorCode,
                      message: String,
                      details: Option<serde_json::Value>| {
            let now_us = std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .map(|d| d.as_micros() as i64)
                .unwrap_or(0);
            let record = vadis_proxy::tool_refused_record(
                rid,
                tool,
                code,
                message.clone(),
                now_us,
                started.elapsed().as_millis() as u32,
                rev.forwarder
                    .trace
                    .as_ref()
                    .map(|t| t.config_digest())
                    .unwrap_or(""),
                client,
            );
            // Write, then answer — the order every boundary refusal uses
            // (ADR-040 D2); a trace write failure is §8's non-blocking case.
            if let Some(t) = rev.forwarder.trace.as_ref() {
                let _ = t.write(&record);
            }
            media_error(rid, code, message, details)
        };
        let cfg = match rev.forwarder.config.media.clone() {
            Some(cfg) => cfg,
            None => {
                return refuse(
                    vadis_core::error::ErrorCode::NotImplemented,
                    format!(
                        "POST /v1/tools/{tool} is not served here: the config writes no media: \
                         section (spec §4.22 — the tools are opt-in, and writing nothing is the \
                         operator saying they are off)"
                    ),
                    None,
                )
            }
        };
        if tool != "ocr" {
            return refuse(
                vadis_core::error::ErrorCode::NotImplemented,
                format!(
                    "POST /v1/tools/{tool} is not implemented: vadis has no audio route kind \
                     (spec §4.22 reserves the verb; OCR is the one verb that serves today)"
                ),
                None,
            );
        }
        let limit = cfg.max_body_bytes as usize;
        let declared: Option<u64> = headers
            .get(axum::http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok());
        if let Some(len) = declared {
            if len > limit as u64 {
                return refuse(
                    vadis_core::error::ErrorCode::RequestTooLarge,
                    format!(
                        "request body exceeds media.max_body_bytes: {len} bytes declared, the \
                         configured bound is {limit}"
                    ),
                    Some(serde_json::json!({
                        "bound": "media.max_body_bytes",
                        "limit_bytes": limit,
                        "content_length": declared,
                    })),
                );
            }
        }
        let raw = match axum::body::to_bytes(body, limit).await {
            Ok(bytes) => bytes,
            Err(_) => {
                return refuse(
                    vadis_core::error::ErrorCode::RequestTooLarge,
                    format!(
                        "request body exceeds media.max_body_bytes: the configured bound is \
                         {limit}"
                    ),
                    Some(serde_json::json!({
                        "bound": "media.max_body_bytes",
                        "limit_bytes": limit,
                        "content_length": serde_json::Value::Null,
                    })),
                )
            }
        };
        let pairs: Vec<(String, String)> = headers
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let transform_mode = match vadis_proxy::resolve_transform_mode(&pairs) {
            Ok(m) => m,
            Err(f) => return refuse(f.code, f.message, f.details),
        };
        let chat = match vadis_proxy::ocr_chat_body(&raw, &cfg) {
            Ok(chat) => chat,
            Err(e) => return refuse(e.code, e.message, None),
        };
        match rev
            .forwarder
            .forward_with_client(
                WireApi::Chat,
                &chat,
                rid,
                &pairs,
                transform_mode,
                "vadis-media",
            )
            .await
        {
            ForwardOutcome::Success(s) => {
                if !(200..300).contains(&s.status) {
                    return media_error(
                        rid,
                        vadis_core::error::ErrorCode::UpstreamError,
                        format!(
                            "the media model's route answered HTTP {}: the upstream's own \
                             refusal is not a tool answer (spec §4.22)",
                            s.status
                        ),
                        Some(serde_json::json!({ "upstream_status": s.status })),
                    );
                }
                match vadis_proxy::ocr_text(&s.body) {
                    Ok(text) => {
                        (StatusCode::OK, Json(serde_json::json!({ "text": text }))).into_response()
                    }
                    Err(e) => media_error(rid, e.code, e.message, None),
                }
            }
            ForwardOutcome::Failure(f) => {
                let details = f.client_body_details();
                let mut eb = ErrorBody::new(f.code, f.message, rid);
                eb.error.details = details;
                (
                    StatusCode::from_u16(f.status).expect("failure status maps"),
                    Json(eb),
                )
                    .into_response()
            }
        }
    }

    /// `GET /state/events` (spec §4.18, ADR-054, DESIGN §12.24): the live
    /// event-log figure, answered by the running process from its own
    /// store. The third guarded non-protocol route: same guard middleware
    /// as `/metrics`, its own word `"state"`, and — like `/metrics` — no
    /// body extractor, no body layer, no request byte read. The window
    /// arrives as the one query parameter; an unusable one answers
    /// `400` (§8's body) **before** any store read; a store error answers
    /// `500` honestly (an assembly that cannot read its own store does
    /// not paper over it). The admitted arm writes nothing.
    fn guarded_state_route(
        revision: vadis_proxy::SharedRevision,
        state: std::sync::Arc<AppState>,
    ) -> axum::Router {
        axum::Router::new()
            .route(
                "/state/events",
                get(move |uri: axum::http::Uri| {
                    let state = state.clone();
                    async move {
                        let raw = uri.query().and_then(|q| {
                            q.split('&')
                                .find_map(|kv| kv.strip_prefix("window=").map(|w| w.to_string()))
                        });
                        let now_us = std::time::SystemTime::now()
                            .duration_since(std::time::SystemTime::UNIX_EPOCH)
                            .map(|d| d.as_micros() as i64)
                            .unwrap_or(0);
                        match vadis_proxy::state_events_handle(
                            &state,
                            raw.as_deref(),
                            now_us,
                            rfc3339_micros,
                        ) {
                            Ok(body) => (StatusCode::OK, Json(body)).into_response(),
                            Err("window") => {
                                let request_id = request_id();
                                let mut eb = ErrorBody::new(
                                    vadis_core::error::ErrorCode::InvalidRequest,
                                    format!(
                                        "window: required, the config duration grammar, bounded \
                                         {}ms..={}ms (spec 4.18)",
                                        vadis_proxy::WINDOW_MIN_MS,
                                        vadis_proxy::WINDOW_MAX_MS
                                    ),
                                    &request_id,
                                );
                                eb.error.details = Some(serde_json::json!({
                                    "param": "window",
                                }));
                                let mut resp = (
                                    StatusCode::from_u16(
                                        vadis_core::error::ErrorCode::InvalidRequest.http_status(),
                                    )
                                    .expect("400 maps"),
                                    Json(eb),
                                )
                                    .into_response();
                                if let Ok(v) = request_id.parse() {
                                    resp.headers_mut().insert("x-vadis-request-id", v);
                                }
                                resp
                            }
                            Err(_) => (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                "the state store could not be read",
                            )
                                .into_response(),
                        }
                    }
                }),
            )
            .route_layer(axum::middleware::from_fn_with_state(
                GuardState {
                    revision,
                    proto_in: "state",
                },
                guard_mw,
            ))
    }

    /// RFC3339 UTC with millisecond precision from **microseconds** —
    /// `/state/events`'s in-band window bounds (spec §4.18 uses the same
    /// format `stats` and `/health` print).
    fn rfc3339_micros(us: i64) -> String {
        let ms = us.div_euclid(1_000);
        // Milliseconds from microseconds: the remainder of the ÷1000 is
        // microseconds-in-the-millisecond — dividing it by 1_000 again
        // (the typo this fixes) always produced 0, truncating spec
        // §4.18's "RFC3339 UTC, milliseconds" to whole seconds.
        let milli = us.rem_euclid(1_000_000) / 1_000;
        crate::stats::rfc3339_pub(ms, milli.max(0) as u64)
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
        revision: vadis_proxy::SharedRevision,
        proto_in: &'static str,
    }

    /// The boundary's `User-Agent` derivation (ADR-061 D3), as a request
    /// extension: the guard derives the vocabulary word once per request
    /// — the same place that reads the headers for §4.7's guard — and
    /// every later record writer (the `413` bound, the mode refusal, the
    /// pipeline's terminal row) reads it here rather than re-deriving,
    /// so one request's rows can never disagree about who called. The
    /// raw header value is dropped at the derivation and never stored.
    #[derive(Clone, Copy)]
    struct ClientWord(&'static str);

    /// Where the request path reads the boundary's word (ADR-061 §4.2's
    /// "once"): the extension the guard inserted, never a second
    /// derivation — the engine entry point the serving path calls
    /// (`forward_with_client` / `forward_stream_with_client`) takes the
    /// same word as a parameter. The guard is the outermost layer on
    /// every route that writes a record (§12.11), so the extension is
    /// structurally always present; the `unwrap_or` is the SAME posture
    /// the revision read one layer down takes (degrade, do not abort a
    /// response over a trace-only field) — the table's own fallback
    /// word, never a re-derivation.
    fn client_word(exts: &axum::http::Extensions) -> &'static str {
        exts.get::<ClientWord>().map(|w| w.0).unwrap_or("other")
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
        // ADR-061 D3: the ONE `User-Agent` derivation — here, where the
        // boundary already reads the headers for §4.7's guard, before
        // the pipeline, so the pre-pipeline records and the ordinary
        // row carry the same word. The header value is dropped here and
        // never recorded in any form.
        let client_word = ClientWord(vadis_core::trace::client_from_user_agent(
            req.headers()
                .get(axum::http::header::USER_AGENT)
                .and_then(|v| v.to_str().ok()),
        ));
        req.extensions_mut().insert(client_word);
        let headers: Vec<(String, String)> = req
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let verdict = rev.auth_gate.as_ref().map(|gate| gate.admits(&headers));
        match verdict {
            None | Some(vadis_proxy::AuthVerdict::Admitted) => next.run(req).await,
            Some(verdict @ vadis_proxy::AuthVerdict::Refused { .. }) => {
                let request_id = request_id();
                let started = std::time::Instant::now();
                let now_us = std::time::SystemTime::now()
                    .duration_since(std::time::SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_micros() as i64)
                    .unwrap_or(0);
                let record = vadis_proxy::refused_record(
                    &request_id,
                    st.proto_in,
                    &verdict,
                    now_us,
                    started.elapsed().as_millis() as u32,
                    rev.forwarder
                        .trace
                        .as_ref()
                        .map(|t| t.config_digest())
                        .unwrap_or(""),
                    client_word.0,
                );
                // Write, then answer — and either way the client is told
                // 401 (§8's non-blocking case).
                if let Some(t) = rev.forwarder.trace.as_ref() {
                    let _ = t.write(&record);
                }
                let mut body = ErrorBody::new(
                    vadis_core::error::ErrorCode::Unauthorized,
                    vadis_proxy::refused_message(&verdict),
                    request_id,
                );
                body.error.details = Some(serde_json::json!({
                    "header": match verdict {
                        vadis_proxy::AuthVerdict::Refused { header } => header,
                        _ => None,
                    },
                }));
                let mut resp = (
                    StatusCode::from_u16(vadis_core::error::ErrorCode::Unauthorized.http_status())
                        .expect("401 maps"),
                    Json(body),
                )
                    .into_response();
                if let Ok(v) = record.identity.request_id.parse() {
                    resp.headers_mut().insert("x-vadis-request-id", v);
                }
                resp
            }
        }
    }

    // Spec §4.13's bound, resolved once from the validated config: the
    // loader already refused anything below 1024, so the `usize` cast
    // here is total.
    let max_body_bytes = rc.vadis.server.max_body_bytes as usize;

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
        .merge(guarded_models_route(cell.clone()))
        .merge(guarded_media_tools_route(cell.clone()))
        .merge(guarded_tools_route(cell.clone()))
        .merge(guarded_state_route(cell.clone(), state.clone()))
        .with_state(());

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("vadis: cannot listen on {addr} (from server.addr): {e}");
            return 3;
        }
    };
    eprintln!(
        "vadis listening on {addr} (config dir: {}, trace: {}, state: {} [store open], auth: {})",
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
            eprintln!("vadis: {e}");
            None
        }
    };
    // ADR-060 D1 (§5's owed ordering): the graceful-shutdown drain. The
    // signal handler flips the process-wide drain flag BEFORE any
    // in-flight work is dropped, so a request whose future or body the
    // drain then drops is written `canceled_by: "shutdown"` instead of
    // blaming the client; axum's graceful shutdown then waits for the
    // in-flight connections, and every cancelled-row guard holds its own
    // `Arc`s to the store and the trace sink — the sink cannot close
    // while an in-flight request exists, so the drain's rows are not
    // lost. (A `SIGKILL` still writes nothing; that boundary is ADR-060
    // §2.3.5's, stated rather than hidden.)
    let shutdown_signal = async {
        let sigterm = async {
            use tokio::signal::unix::{signal, SignalKind};
            match signal(SignalKind::terminate()) {
                Ok(mut s) => {
                    s.recv().await;
                }
                Err(_) => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm => {}
        }
        vadis_proxy::set_shutdown_draining();
    };
    match axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal)
        .await
    {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("vadis: server error: {e}");
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

    fn trace_sink(tag: &str) -> std::sync::Arc<vadis_store::TraceSink> {
        let dir = std::env::temp_dir().join(format!("vadis-cli-lib-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sink = vadis_store::TraceSink::open(&dir).expect("a temp trace dir opens");
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
            vadis_core::TraceWriter::config_digest(&w),
            "0123456789abcdef"
        );
    }

    /// R43-F4, the forgotten case failing loudly: a writer wired into the
    /// serving path that cannot produce a non-empty digest fails AT
    /// CONSTRUCTION, with a message naming the writer — what may never
    /// happen is a record stamped with an identity of `""` (spec §6's
    /// empty-string bullet). The compile-time half of the same rule is
    /// the trait's required method (`vadis-core/src/trace.rs`): a writer
    /// that never implements `config_digest` does not compile.
    #[test]
    #[should_panic(expected = "ConfigTraceWriter: an empty config_digest on the serving path")]
    fn the_forgotten_digest_fails_at_construction_naming_the_writer() {
        let _ = ConfigTraceWriter::new(trace_sink("f4-empty"), String::new());
    }
}
