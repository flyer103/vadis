//! The buffered forwarding engine: native passthrough with byte
//! fidelity, usage normalization, ADR-011 error classification, the
//! fallback chain with provider-level exclusion, and the §12.10.5 event
//! sequence (`upstream.submitted` → `upstream.responded` →
//! `error.classified` → the action's effect).
//!
//! Non-streaming only: a `stream: true` inbound body is refused with 501
//! by this engine before any attempt runs (a `stream: true` request is
//! served by the SSE relay, never buffered here), so an attempt never
//! half-relays.

use std::borrow::Cow;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use http::Request as HttpRequest;
use serde_json::{json, Value};
use vadis_core::config::{PlanPolicyCfg, ProviderCfg, RouteSpec, VadisConfig, WireApi};
use vadis_core::error::ErrorCode;
use vadis_core::error_class::{
    classify_upstream_error, Classification, ErrorClass, ErrorEvidence, TransportCause,
};
use vadis_core::plan::{
    account_of_route, displacement_reason, metered_walk, plan_tier, route_in_family,
    PlanFirstRule, PlanMove,
    PlanRequest, PlanStateRow, REASON_PRIMARY_RECOVERED,
};
use vadis_core::prefix::{attribute_tokens, body_sha16, extract_prefix_blocks, PrefixBlock};
use vadis_core::store::{EventKind, NewEvent, ProjectionWrite, Query, QueryRow, Store};
use vadis_core::trace::PlanSwitchRec;
use vadis_core::transform::{estimate_tokens, PayloadCtx, TransformEngine, TransformMode};
use vadis_core::{RawBody, RawEditError, Usage, VADIS_OWNED_TOP_LEVEL_KEYS};
use vadis_providers::{AttemptOutcome, TransportKind, UpstreamPlan};

/// The async seam the engine is monomorphized over: the real reqwest client
/// and the test double both satisfy it (DESIGN §12.10.1: no `async-trait`).
pub trait ProviderSend {
    fn send(&self, req: HttpRequest<Bytes>) -> impl Future<Output = AttemptOutcome> + Send;
}

/// Object-safe wrapper so `Forwarder` can hold one transport per provider
/// without making the whole serving state generic. The future is tied to
/// `&self` and awaited immediately at the call site.
pub type BoxedAttempt<'a> = Pin<Box<dyn Future<Output = AttemptOutcome> + Send + 'a>>;

pub trait ProviderTransport: Send + Sync {
    fn send_boxed<'a>(&'a self, req: HttpRequest<Bytes>) -> BoxedAttempt<'a>;
}

impl<T> ProviderTransport for T
where
    T: ProviderSend + Send + Sync + 'static,
{
    fn send_boxed<'a>(&'a self, req: HttpRequest<Bytes>) -> BoxedAttempt<'a> {
        Box::pin(self.send(req))
    }
}

/// The production transport: the reqwest-backed client satisfies the
/// monomorphized seam (DESIGN §12.10.1).
impl ProviderSend for vadis_providers::ReqwestProviderClient {
    fn send(&self, req: HttpRequest<Bytes>) -> impl Future<Output = AttemptOutcome> + Send {
        vadis_providers::ReqwestProviderClient::send(self, req)
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

#[derive(Clone)]
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

/// The unusable-`X-Vadis-Transform` refusal's record (DESIGN §12.12's
/// failure table: "the pre-pipeline record class of spec §6"): the same
/// field-by-field class as the auth guard's `refused_record` — decided
/// before the body is read, so nothing was parsed, planned or attempted,
/// `transform_mode` is `passthrough` (the chain never ran) and the error
/// is `transform_error` (the §8 vocabulary's word for `invalid_request`).
pub fn mode_refused_record(
    request_id: &str,
    proto_in: WireApi,
    failure: &ForwardFailure,
    now_epoch_s: u64,
    overhead_ms: u32,
    config_digest: &str,
) -> vadis_core::trace::DecisionRecord {
    use vadis_core::trace::{
        CostRec, DecisionRec, DecisionRecord, IdentityRec, PrefixRec, ProtocolRec, ResultRec,
        StateRec, TraceError, TRACE_SCHEMA_VERSION,
    };
    use vadis_core::{Currency, Nano};

    DecisionRecord {
        schema_version: TRACE_SCHEMA_VERSION,
        ts: crate::accounting::rfc3339_millis(now_epoch_s),
        config_digest: config_digest.to_string(),
        identity: IdentityRec {
            request_id: request_id.to_string(),
            // The mode is decided before §12.10.5's row 1: the same `0`
            // sentinel every pre-pipeline refusal writes.
            event_id: 0,
            client: "other",
            session: None,
            thread_id: None,
            turn_index: 0,
        },
        protocol: ProtocolRec {
            protocol_in: proto_in.as_str().to_string(),
            protocol_out: None,
            translated: false,
            lossy: Vec::new(),
        },
        decision: DecisionRec {
            provider: String::new(),
            model: String::new(),
            requested_model: None,
            selection_source: "explicit".to_string(),
            plugin_chain: Vec::new(),
            decision_ms: 0,
            // Refused before any walk began: no credential served
            // (ADR-049 §3; spec §6's null class).
            key_index: None,
        },
        state: StateRec {
            stateful_inbound: false,
            sticky_hit: false,
            cache_control_breaks: 0,
        },
        prefix: PrefixRec {
            blocks: Vec::new(),
            continuity: None,
        },
        // The chain never ran: the mode word is `passthrough` and nothing
        // is claimed (spec §6's pre-pipeline row).
        transform_mode: TransformMode::Passthrough,
        transforms: Vec::new(),
        // A pre-pipeline refusal can never be a hit: no cache group.
        cache: None,
        usage: vadis_core::Usage::default(),
        usage_missing: true,
        cost: CostRec {
            input_miss: Nano(0),
            input_hit: Nano(0),
            cache_write: Nano(0),
            output: Nano(0),
            peak_applied_pct: 100,
            total: Nano(0),
            // The pre-pipeline refusal default: no table priced this
            // record, so the unit is the USD default — the same stance
            // as the failure path in accounting.rs (spec §4.8), and
            // every amount is 0, which is why it cannot mislead.
            currency: Currency::Usd,
            quota_after: None,
        },
        result: ResultRec {
            status: failure.status,
            upstream_status: None,
            failover_from: None,
            plan_switch: None,
            overhead_ms,
            upstream_ms: None,
        },
        errors: vec![TraceError {
            kind: vadis_core::trace::TraceError::kind_for_code(failure.code).to_string(),
            message: failure.message.clone(),
            plugin: None,
            details: failure.details.clone(),
        }],
    }
}

/// What the forwarding engine holds: one transport per provider, the api keys read at
/// startup, and (optionally) the store and trace sink. `store: None` only
/// in tests without state.
pub struct Forwarder {
    pub config: VadisConfig,
    pub transports: HashMap<String, Arc<dyn ProviderTransport>>,
    /// Provider name → the provider's credential pool (ADR-049 §3): the
    /// **present** names' values, read once at the revision's build, in
    /// declaration order. The values exist only in outbound requests —
    /// never in events or traces (DESIGN §12.10.1's auth rule). Rotation
    /// (rule 3) advances an index into this list on credential-class
    /// failures only; `403 quota_exhausted` never rotates (rule 4).
    pub api_keys: HashMap<String, Vec<String>>,
    pub store: Option<Arc<dyn Store>>,
    /// The trace sink: one DecisionRecord per request. `None`
    /// only in tests without a trace dir.
    pub trace: Option<Arc<dyn vadis_core::TraceWriter>>,
    /// The transform rule engine (ADR-019 / spec §4.4 `builtin/
    /// transform_rules`), loaded at startup from the configured rule file.
    /// `None` when no rule set is configured: a request that asks for
    /// transform mode then runs with an empty ledger ("asked, not
    /// applied" — a countable state, spec §6). The mode is a request
    /// fact, never a route property, so this field never decides
    /// anything on the passthrough path (I3).
    pub transform_engine: Option<Arc<dyn vadis_core::transform::TransformEngine>>,
    /// The exact-match response cache (spec §4.17, ADR-042): the
    /// plugin's assembled handle on the **one** store, `None` unless the
    /// operator mounted `builtin/response_cache` with
    /// `config.enabled: true`. An absent or inert mount is this `None`,
    /// and the request path is then byte-identical to a build that does
    /// not know the kind (CONF-88). Reached at exactly **one** call site
    /// per forwarding medium — the last step before the upstream attempt
    /// (ADR-042 §4.4), never earlier.
    pub response_cache: Option<Arc<dyn vadis_core::response_cache::ResponseCache>>,
    /// Inbound headers per request (the session key sources live there;
    /// the body's `prompt_cache_key` is checked at receive).
    pub session_ttl_us: i64,
}

fn now_us() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0)
}

/// The `plan_switch.candidates[]` field (spec §6 / ADR-049 §5.6): the
/// resolved metered ranking, one `"<provider>/<model>"` string per
/// candidate in rank order — `None` under `declared` (nothing was
/// ranked; the key serializes null so the key set never changes shape).
/// One owner both media call, so the two paths cannot disagree.
pub(crate) fn metered_candidates_field(
    config: &vadis_core::config::VadisConfig,
    policy: &vadis_core::config::PlanPolicyCfg,
) -> Option<Vec<String>> {
    match policy.overflow_selection {
        vadis_core::config::OverflowSelection::Cheapest => Some(
            vadis_core::plan::ranked_metered_candidates(config, policy)
                .iter()
                .map(|c| format!("{}/{}", c.route.provider, c.route.model))
                .collect(),
        ),
        _ => None,
    }
}

/// The UTC calendar-month boundary (µs) containing `now_us` — the
/// overflow cap's metering month (spec §4.6: a UTC calendar month).
pub(crate) fn month_start_us(now_us: i64) -> i64 {
    let (y, m, _d, _, _) = vadis_core::peak::timestamp_parts(
        (now_us / 1_000_000).max(0) as u64,
        vadis_core::peak::Tz::Utc,
    );
    vadis_core::peak::utc_midnight_epoch(y, m, 1) as i64 * 1_000_000
}

/// The plan guard's answer for one request (spec §4.6 / DESIGN
/// §12.10.8): the route the family's state sends this request to, plus
/// what the trace needs. `probe` is true when this pass was admitted as
/// a recovery probe on the primary (ADR-014 item 3). The pre-request
/// account state is deliberately NOT here: the displacement record's
/// reason is decided by direction (the destination), never by the state
/// the guard read (spec §6's producer table).
#[derive(Debug, Clone)]
pub(crate) struct PlanGuardOutcome {
    pub route: RouteSpec,
    pub probe: bool,
}

/// Session key resolution (spec §4 key_sources), shared by both forwarding
/// paths so a streamed request is the same request: the body's
/// `prompt_cache_key` first, then the configured `header:<name>` sources,
/// in declaration order. One implementation — the buffered and streaming
/// paths cannot disagree about who is asking.
pub(crate) fn resolve_session_key(
    config: &VadisConfig,
    parsed: &Value,
    headers: &[(String, String)],
) -> Option<String> {
    parsed
        .get("prompt_cache_key")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            config.session.key_sources.iter().find_map(|src| {
                let name = src.strip_prefix("header:")?;
                headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(name))
                    .map(|(_, v)| v.clone())
            })
        })
}

/// The turn index for a session: `requests_seen` from the projection + 1,
/// 1 on the session's first request, 1 with no session (§12.10.5).
pub(crate) fn turn_index_for(store: &Option<Arc<dyn Store>>, session: Option<&str>) -> u32 {
    session
        .and_then(|s| {
            let q = store
                .as_ref()?
                .query(Query::SessionRequestsSeen { session_key: s });
            match q {
                Ok(QueryRow::Count(n)) if n > 0 => Some(n as u32 + 1),
                _ => Some(1),
            }
        })
        .unwrap_or(1)
}

/// The binding that existed when this request arrived (spec §6): the
/// ONE session-resolution read, taken before any binding write this
/// request may make. `Some(row)` ⇔ a live binding existed — that is
/// `state.sticky_hit` — and the row's `(provider, model)` is the prior
/// `route_changed` compares against (note R6: one read, both inputs,
/// never a second read after the write it gates — R11-F2's class). A
/// read error is absent, the same way the bool form treated it.
pub(crate) fn prior_session_binding(
    store: &Option<Arc<dyn Store>>,
    session: Option<&str>,
) -> Option<vadis_core::store::SessionBindingRow> {
    let session = session?;
    match store.as_ref()?.query(Query::SessionBinding {
        session_key: session,
    }) {
        Ok(QueryRow::SessionBinding(row)) => row,
        _ => None,
    }
}

/// The one shared outbound-body composition step (DESIGN §12.10.7): both
/// forwarding paths compose the upstream body through this function's two
/// mutations only — a hand-copied second composition is where the byte
/// boundary dies. Mutation (a) runs once per request in the callers; this
/// is mutation (b), per attempt: the value of the top-level `model`
/// member becomes `native_id`, every other byte the client's. The rewrite
/// is a pure function of (bytes, route), which is what makes an alias and
/// a direct route to the same route byte-identical upstream (CONF-27).
pub(crate) fn rewrite_outbound_model<'a>(
    cleaned: &'a RawBody,
    native_id: &str,
) -> Result<Cow<'a, [u8]>, RawEditError> {
    cleaned.set_top_level_string("model", native_id)
}

/// `X-Vadis-Transform` (ADR-019 §2, spec §2.1), done by
/// `vadis-proxy` at the boundary — vadis-core never reads a header.
/// Absent or `passthrough` ⇒ the byte path; `transform` ⇒ the declared-edit
/// path; **any other value is a 400 `invalid_request` decided before the
/// body is read** — a typo must not silently disable a saving the client
/// asked for, and it must never silently enable one.
pub fn resolve_transform_mode(
    headers: &[(String, String)],
) -> Result<TransformMode, ForwardFailure> {
    let Some((_, v)) = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("x-vadis-transform"))
    else {
        return Ok(TransformMode::Passthrough);
    };
    match v.trim() {
        "passthrough" => Ok(TransformMode::Passthrough),
        "transform" => Ok(TransformMode::Transform),
        other => Err(ForwardFailure::new(
            400,
            ErrorCode::InvalidRequest,
            format!(
                "unusable X-Vadis-Transform value '{other}': expected 'passthrough' or 'transform' \
                 (spec §2.1 — a client asking for a mode that does not exist is told, never \
                 silently served the other way)"
            ),
        )),
    }
}

/// The shared transform-chain composition step (DESIGN §12.12: "the same
/// step serves both forwarding paths" — a second copy is the L2 leak
/// pattern). One call per request: computes the plan from (the client's
/// bytes after mutation (a), the mode, the configured rule set) and
/// nothing else, applies its edits as value spans, and returns the
/// outbound base plus the ledger and the applier's fail-safe entry.
///
/// - `passthrough` mode plans nothing, whatever is configured (I3);
/// - no configured engine ⇒ "asked, not applied": an empty ledger, the
///   base is the cleaned bytes unchanged;
/// - the applier's failure is fail-safe (spec §8): the body is forwarded
///   unedited and the degradation is declared as
///   `errors[].kind = transform_error` with an **empty** ledger — never
///   a partial edit and never a fake ledger entry (DESIGN §12.12's
///   failure table).
pub(crate) fn compose_transform_stage(
    engine: Option<&dyn TransformEngine>,
    mode: TransformMode,
    cleaned: &RawBody,
    proto: WireApi,
) -> (
    RawBody,
    Vec<vadis_core::trace::TransformRecord>,
    Option<vadis_core::trace::TraceError>,
) {
    let mut records = Vec::new();
    if mode != TransformMode::Transform {
        return (cleaned.clone(), records, None); // I3: the closed mode plans nothing.
    }
    let Some(engine) = engine else {
        // No rule set loaded: "asked, not applied" is a countable state
        // (spec §6's separate mode field), not an error.
        return (cleaned.clone(), records, None);
    };
    let mut edits = Vec::new();
    // Aggregate per (rule id, cache_impact): one ledger entry per step
    // (rule) that changed at least one payload, carrying every node it
    // edited.
    for node in vadis_core::transform::payload_nodes(cleaned, proto) {
        let tool = node.tool.as_deref();
        let ctx = PayloadCtx {
            tool,
            kinds: vadis_core::transform::kinds_for_tool(tool.unwrap_or("")),
        };
        let bytes_in = node.text.len();
        let Some(o) = engine.apply_node(&ctx, &node.text) else {
            continue;
        };
        if o.new_text == node.text {
            continue;
        }
        // One ledger entry per **rule** (step), aggregated across the
        // payload nodes that rule edited: keyed by the outcome's own rule
        // id — spec §6's `plugin` is "the rule id", so one engine carrying
        // many rules still writes one attributable entry per rule.
        let idx = match records
            .iter()
            .position(|r| r.plugin == o.rule && r.cache_impact == o.cache_impact)
        {
            Some(i) => i,
            None => {
                records.push(vadis_core::trace::TransformRecord {
                    plugin: o.rule.clone(),
                    edited_paths: Vec::new(),
                    // net = saved − added (spec §7): the added side counts
                    // the tee marker, the re-encoding and the replaced text
                    // — all of it is inside `bytes_out`, so the two
                    // estimates below are the full sides of that
                    // arithmetic, never a raw delta.
                    added_input_tokens: 0,
                    saved_input_tokens: 0,
                    saved_output_tokens: 0,
                    cache_impact: o.cache_impact,
                    // A decision-time figure is `inferred` and says so
                    // (spec §7: one observation measures the request,
                    // never the saving; a verified figure needs the
                    // on/off pair).
                    verdict: "inferred",
                    tee_id: None,
                    error: None,
                });
                records.len() - 1
            }
        };
        let rec = &mut records[idx];
        rec.edited_paths.push(vadis_core::trace::EditedPath {
            path: node.path.display(),
            bytes_in,
            bytes_out: o.new_text.len(),
        });
        rec.saved_input_tokens += estimate_tokens(bytes_in);
        rec.added_input_tokens += estimate_tokens(o.new_text.len());
        if o.tee_id.is_some() {
            rec.tee_id = o.tee_id.clone();
        }
        edits.push(vadis_core::transform::PayloadEdit {
            path: node.path,
            rule: o.rule.clone(),
            bytes_out: o.new_text.len(),
            new_text: o.new_text,
            bytes_in,
        });
    }
    match cleaned.apply_edits(&edits) {
        Ok(b) => (RawBody::new(b.into_owned()), records, None),
        Err(e) => (
            cleaned.clone(),
            Vec::new(),
            Some(vadis_core::trace::TraceError {
                kind: "transform_error".to_string(),
                message: format!(
                    "payload edit could not be spliced ({e:?}); the body was forwarded unedited \
                     (fail-safe, spec §8)"
                ),
                plugin: Some(engine.id().to_string()),
                details: None,
            }),
        ),
    }
}

/// What the buffered path learned about a request before its terminal
/// outcome — enough to write the failure `DecisionRecord` when the
/// outcome is a failure (spec §6: one line per request, failures
/// included). `now_epoch_s` is the request's clock read in whole
/// seconds (reused by the terminal record) and `now_us` the same read
/// in microseconds — ONE read, two projections of it (AGENTS
/// constraint 2): the guard must judge the same instant `/health`
/// reports (spec §9.1), so the µs truncation of the seconds word may
/// not reach `PlanRequest::now_us` (the up-to-1s clock disagreement
/// was the flake factory). Shared with the streaming path: both
/// paths' failures record through the same facts shape.
pub(crate) struct RequestFacts<'a> {
    pub(crate) request_id: &'a str,
    pub(crate) received_event: Option<vadis_core::EventId>,
    pub(crate) proto_in: WireApi,
    pub(crate) proto_out: Option<WireApi>,
    pub(crate) session: Option<String>,
    pub(crate) turn_index: u32,
    pub(crate) requested_model: Option<String>,
    pub(crate) selection_source: &'static str,
    pub(crate) decision_ms: u32,
    pub(crate) started: Instant,
    pub(crate) now_epoch_s: u64,
    /// The same clock read as `now_epoch_s`, untruncated (µs).
    pub(crate) now_us: i64,
    /// Prefix blocks of the cleaned body, set once mutation (a) ran.
    pub(crate) blocks: Vec<PrefixBlock>,
    /// The first route that failed over, for `result.failover_from`.
    pub(crate) failover_from: Option<RouteSpec>,
    /// The last attempt's wire latency (a single-attempt failure's
    /// `result.upstream_ms`); `None` until an attempt answered.
    pub(crate) upstream_ms: Option<u32>,
    /// The last route actually attempted (the failure record's
    /// `decision.provider`/`model` — "which provider died").
    pub(crate) attempted_route: Option<RouteSpec>,
    /// The last upstream status that arrived, when one did (mirrored into
    /// `result.upstream_status` on the failure record).
    pub(crate) last_upstream_status: Option<u16>,
    /// spec §6 `result.plan_switch`: set when the plan policy displaced
    /// this request's account (ADR-014) — at the guard (state-driven
    /// move) or when a 403 `quota_exhausted` on the family's primary
    /// spilled it mid-request.
    pub(crate) plan_switch: Option<PlanSwitchRec>,
    /// spec §6 `state.sticky_hit`: **one per-request predicate, read
    /// once before any binding write** (the R21-1 freeze). The value is
    /// fixed at session resolution — "did this session already have a
    /// live binding row when the request arrived?" — and every later
    /// consumer (the `bind_session` early-return, the trace record, the
    /// shared failure record) reuses this value; recomputing it after
    /// the request's own row-4 write would report the write it just
    /// made (the R11-F2 defect class).
    pub(crate) sticky_hit: bool,
    /// spec §6 `transform_mode`: resolved from `X-Vadis-Transform` at the
    /// boundary (ADR-019 §2). `Passthrough` is the default; a request
    /// refused before the transform chain ran keeps it (nothing claimed).
    pub(crate) transform_mode: TransformMode,
    /// The transform chain's ledger entries (spec §6 `transforms[]`),
    /// written with the plan at the chain stage; kept on a failure path —
    /// an entry describes the plan, never a claim the bytes left the
    /// process (DESIGN §12.12's failure table).
    pub(crate) transform_records: Vec<vadis_core::trace::TransformRecord>,
    /// The applier's fail-safe entry, when a plan's edits could not be
    /// spliced (spec §8 / DESIGN §12.12: forwarded unedited, declared in
    /// `errors[]` as `transform_error`, empty `transforms[]`).
    pub(crate) transform_error: Option<vadis_core::trace::TraceError>,
}

impl Forwarder {
    /// Forwards one buffered request. `proto_in` is the inbound endpoint's
    /// protocol; a native route (proto_in == the provider's `wire_api`)
    /// forwards the client's bytes minus vadis-owned top-level keys,
    /// everything else byte-identical. `headers` feed the session key
    /// sources (`header:<name>`); the body's `prompt_cache_key` wins when
    /// present (spec §4 key_sources order).
    ///
    /// Every terminal failure records its `DecisionRecord` before the
    /// outcome leaves the engine — one shared call into
    /// `Accountant::finish_failure`, never a second inlined copy.
    ///
    /// `transform_mode` is the boundary-resolved mode (ADR-019 §2):
    /// `vadis-proxy` resolved `X-Vadis-Transform` before calling in —
    /// vadis-core never reads a header. `Passthrough` keeps the byte
    /// path exactly as it was.
    pub async fn forward(
        &self,
        proto_in: WireApi,
        body: &[u8],
        request_id: &str,
        headers: &[(String, String)],
        transform_mode: TransformMode,
    ) -> ForwardOutcome {
        // One clock read, two projections (AGENTS constraint 2): the µs
        // word feeds the plan guard (which must judge the same instant
        // /health reports), the seconds word everything else.
        let now_us = now_us();
        let mut facts = RequestFacts {
            request_id,
            received_event: None,
            proto_in,
            proto_out: None,
            session: None,
            turn_index: 1,
            requested_model: None,
            selection_source: "explicit",
            decision_ms: 0,
            started: Instant::now(),
            now_epoch_s: (now_us / 1_000_000).max(0) as u64,
            now_us,
            blocks: Vec::new(),
            failover_from: None,
            upstream_ms: None,
            attempted_route: None,
            last_upstream_status: None,
            plan_switch: None,
            // The one sticky read (spec §6): a request that has not yet
            // resolved its session cannot have a binding row — false by
            // definition, never recomputed later.
            sticky_hit: false,
            transform_mode,
            transform_records: Vec::new(),
            transform_error: None,
        };
        let outcome = self
            .forward_inner(&mut facts, proto_in, body, request_id, headers)
            .await;
        if let ForwardOutcome::Failure(f) = &outcome {
            self.record_failure_trace(&facts, f);
        }
        outcome
    }

    /// One trace line for a terminal failure (spec §6 / §8): the same
    /// `Accountant` seam as the success path, so the failure record's
    /// shape cannot drift from it. Missing facts stay missing — an
    /// unparsable body has no `session` and no `blocks`, and the record
    /// says so by leaving them empty rather than inventing values.
    /// Shared by both forwarding paths.
    pub(crate) fn record_failure_trace(&self, facts: &RequestFacts<'_>, f: &ForwardFailure) {
        crate::accounting::Accountant {
            store: self.store.as_deref(),
            trace: self.trace.as_deref(),
            // No usage arrived on a failure path: the price table would
            // price nothing (commit() skips everything at usage_missing),
            // so the lookup is deliberately not performed.
            accounting: None,
        }
        .finish_failure(
            &crate::accounting::AccountCtx {
                request_id: facts.request_id,
                received_event: facts.received_event,
                proto_in: facts.proto_in.as_str(),
                proto_out: facts.proto_out.map(WireApi::as_str),
                session: facts.session.as_deref(),
                turn_index: facts.turn_index,
                selection_source: facts.selection_source,
                // The walk ended unserved: no credential to name.
                key_index: None,
                requested_model: facts.requested_model.as_deref(),
                decision_ms: facts.decision_ms,
                started: facts.started,
                now_epoch_s: facts.now_epoch_s,
                plan_switch: facts.plan_switch.clone(),
                // The one value (spec §6): read at session resolution,
                // before this request's binding write — not recomputed
                // here at record time, where the same request's write
                // would answer (R21-1 freeze).
                sticky_hit: facts.sticky_hit,
                transform_mode: facts.transform_mode,
                transforms: facts.transform_records.clone(),
                transform_error: facts.transform_error.clone(),
            },
            f,
            facts
                .attempted_route
                .as_ref()
                .map(|r| (r.provider.as_str(), r.model.as_str())),
            facts.last_upstream_status,
            &facts.blocks,
            facts.upstream_ms,
            facts
                .failover_from
                .as_ref()
                .map(|r| format!("{}/{}", r.provider, r.model)),
        );
    }

    async fn forward_inner(
        &self,
        facts: &mut RequestFacts<'_>,
        proto_in: WireApi,
        body: &[u8],
        request_id: &str,
        headers: &[(String, String)],
    ) -> ForwardOutcome {
        let started = facts.started;
        let now_epoch_s = facts.now_epoch_s;
        let decision_start = Instant::now();
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
        facts.requested_model = Some(model.to_string());
        if model == "auto" {
            return ForwardOutcome::Failure(ForwardFailure::new(
                400,
                ErrorCode::AutoNotSupported,
                "model 'auto' is not supported in v0.1 (a plugin takes it over; the slot is reserved)",
            ));
        }
        if parsed.get("stream").and_then(|s| s.as_bool()) == Some(true) {
            // A streaming request is not served on this path; never half-relay a
            // stream buffered.
            return ForwardOutcome::Failure(ForwardFailure::new(
                501,
                ErrorCode::NotImplemented,
                "a streaming request is not served on this path; resend with stream: false",
            ));
        }

        // Route resolution (spec §3).
        let (mut primary, selection_source) = match self.resolve_route(model) {
            Ok(r) => r,
            Err(f) => return ForwardOutcome::Failure(f),
        };
        facts.selection_source = selection_source;
        // Session resolution (spec §4 key_sources) — moved ahead of the
        // plan guard, which reads `turn_index` from the sticky
        // projection (ADR-014 item 3's probe predicate). The shared
        // helper, so the buffered and streaming paths resolve the same
        // client the same way.
        let session = resolve_session_key(&self.config, &parsed, headers);
        let turn_index = turn_index_for(&self.store, session.as_deref());
        // The one sticky read (spec §6): fixed here, before any binding
        // write this request may make — the same value feeds
        // `bind_session`'s early return and the trace record below. The
        // row is retained whole: its `(provider, model)` is the prior
        // `route_changed` compares against (note R6 — one read, both
        // inputs, no second read after the write it gates).
        let prior_binding = prior_session_binding(&self.store, session.as_deref());
        let sticky_hit = prior_binding.is_some();
        facts.session = session.clone();
        facts.turn_index = turn_index;
        facts.sticky_hit = sticky_hit;
        // The plan policy's Guard stage (spec §4.6, before the allowance
        // rule; ADR-014 item 9): a request inside a family goes where the
        // family's account state says — primary while it lasts, overflow
        // after a spill, primary again when an admitted probe won. The
        // guard may also refuse (block mode, overflow cap). Outside a
        // family (or with no policy) the request is untouched. The
        // family key is the resolution's own tag (§4.8) — the policy
        // that names it is this request's family policy (ADR-049 §4).
        let mut plan_guard_out: Option<PlanGuardOutcome> = None;
        let plan_policy: Option<PlanPolicyCfg> =
            self.config.family_policy_for_route(&primary).cloned();
        match self.plan_guard(
            plan_policy.as_ref(),
            &primary,
            session.as_deref(),
            turn_index,
            now_epoch_s,
            facts.now_us,
        ) {
            Ok(None) => {}
            Ok(Some(g)) => {
                if g.route != primary {
                    // Reason by DIRECTION — the destination account
                    // (spec §6's producer table, extended by ADR-049
                    // §5.2's reason-by-direction rule): a move to the
                    // metered tier is `primary_exhausted`, a move within
                    // the plan tier is `plan_exhausted`, a move back
                    // into the tier is `primary_recovered`. The account
                    // state the guard read before the request is not an
                    // input: on the spill round itself both arms read
                    // `primary`, and every post-spill state displacement
                    // reads `overflow` whichever way it goes.
                    if let Some(policy) = plan_policy.as_ref() {
                        let tier = plan_tier(&self.config, policy);
                        let metered = metered_walk(&self.config, policy);
                        facts.plan_switch = Some(PlanSwitchRec {
                            from: primary.to_string(),
                            to: g.route.to_string(),
                            reason: displacement_reason(policy, &tier, &metered, &primary, &g.route),
                            probe: g.probe,
                            reprefill_tokens: None,
                            switch_cost_nano: None,
                            // §4.8: the destination route's unit (the
                            // figures above are null here, but the unit
                            // is a fact about the destination and is
                            // stated regardless).
                            cost_currency: self.route_currency(&g.route),
                            candidates: metered_candidates_field(&self.config, policy),
                            chosen: Some(g.route.to_string()),
                        });
                    }
                }
                primary = g.route.clone();
                plan_guard_out = Some(g);
            }
            Err(f) => return ForwardOutcome::Failure(f),
        }
        let Some(provider) = self.provider(&primary) else {
            return ForwardOutcome::Failure(ForwardFailure::new(
                404,
                ErrorCode::UnknownProvider,
                format!("unknown provider '{}'", primary.provider),
            ));
        };
        // Capability: an inbound protocol outside the provider's declared
        // `supports` is a 400, never a best-effort translation (spec §8).
        // Translation between wire shapes is not implemented in v0.1, so a
        // declared-but-different cell is refused below with a 501.
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
                    "translation {} -> {} is not implemented in v0.1; only native routes are served",
                    proto_in, provider.wire_api
                ),
            ));
        }

        // The outbound base (DESIGN §12.10.7): the client's bytes minus
        // vadis-owned top-level keys — mutation (a), once per request. The
        // `model` rewrite (mutation (b)) is per attempt, below, because the
        // fallback chain walks routes whose native ids differ (§12.10.5
        // §12.10.5 note R4: this base is the vadis-visible inbound the row-1 hash
        // names, and it does not depend on the route taken).
        let cleaned =
            match RawBody::new(body.to_vec()).remove_top_level_keys(VADIS_OWNED_TOP_LEVEL_KEYS) {
                Ok(b) => b,
                Err(e) => {
                    return ForwardOutcome::Failure(ForwardFailure::new(
                        400,
                        ErrorCode::InvalidRequest,
                        format!("request body is not a well-formed top-level JSON object: {e:?}"),
                    ))
                }
            };
        // The transform chain stage (§3's order: parse → session → transform
        // chain → selector/guard → encode → forward; DESIGN §12.12): the
        // one shared composition step — plan, splice, ledger and fail-safe
        // entry in a single call. In passthrough mode nothing is planned
        // even when rules would match (I3).
        let (base, ledger, applier_error) = compose_transform_stage(
            self.transform_engine.as_deref(),
            facts.transform_mode,
            &cleaned,
            proto_in,
        );
        facts.transform_records = ledger;
        facts.transform_error = applier_error;
        let outbound_hash = body_sha16(base.as_bytes());
        facts.blocks = extract_prefix_blocks(&base).unwrap_or_default();

        // Rows 1 + 3 of the §12.10.5 wiring (FULL + NORMAL).
        facts.received_event = self.append_event(
            EventKind::RequestReceived,
            request_id,
            Some(&outbound_hash),
            json!({
                "protocol_in": proto_in.as_str(),
                "protocol_out": null,
                "client": null,
                "session": session,
                "turn_index": turn_index,
                "body_hash": outbound_hash,
            }),
            session.as_deref(),
        );
        let decision_ms = decision_start.elapsed().as_millis() as u32;
        facts.decision_ms = decision_ms;
        facts.proto_out = Some(provider.wire_api);
        self.append_event(
            EventKind::DecisionMade,
            request_id,
            None,
            json!({
                "provider": primary.provider,
                "model": primary.model,
                "requested_model": model,
                "selection_source": selection_source,
                "protocol_out": provider.wire_api.as_str(),
                "decision_ms": decision_ms,
            }),
            session.as_deref(),
        );
        // Row 2 — the transform chain point, kept even when the chain is
        // empty. The payload stays §12.10.5 row 2's own shape (plugin,
        // added/saved tokens, cache_impact, verdict) — DESIGN §12.12 adds
        // no event kind and no event-payload field; the mode and the
        // edited paths live in the trace record, whose line is the
        // analysis truth.
        self.append_event(
            EventKind::TransformApplied,
            request_id,
            None,
            json!({
                "plugin": facts.transform_records.first().map(|r| r.plugin.clone()),
                "chain": facts.transform_records.iter().map(|r| json!({
                    "plugin": r.plugin,
                    "added_input_tokens": r.added_input_tokens,
                    "saved_input_tokens": r.saved_input_tokens,
                    "cache_impact": r.cache_impact,
                    "verdict": r.verdict,
                })).collect::<Vec<_>>(),
                "changed": !facts.transform_records.is_empty(),
            }),
            session.as_deref(),
        );

        // Row 4 — session.bound (FULL) when the binding is created or
        // moved; a sticky hit on an unchanged route writes nothing. The
        // `sessions` projection rides on the event row (requests_seen
        // drives the next turn's `turn_index` — a missed projection write
        // would freeze the counter, so it goes through the accountant's
        // bind_session, the one writer).
        // The one sticky value (spec §6): the read taken at session
        // resolution, above — reused here, never recomputed after the
        // write it gates. `route_changed` (note R6) is measured from that
        // same read's row against the route resolved AFTER the guard
        // chain (`primary` here) and before the attempt: true when the
        // prior binding named a different provider or model, false with
        // no prior binding (the create arm) or an unchanged route.
        let sticky_hit = facts.sticky_hit;
        let route_changed = match &prior_binding {
            Some(prior) => prior.provider != primary.provider || prior.model != primary.model,
            None => false,
        };
        if session.is_some() {
            crate::accounting::Accountant {
                store: self.store.as_deref(),
                trace: self.trace.as_deref(),
                accounting: None,
            }
            .bind_session(
                &crate::accounting::AccountCtx {
                    request_id,
                    received_event: facts.received_event,
                    proto_in: proto_in.as_str(),
                    proto_out: Some(provider.wire_api.as_str()),
                    session: session.as_deref(),
                    turn_index,
                    selection_source,
                    requested_model: Some(model),
                    // No upstream attempt served this record yet — the
                    // pool's credential is a walk fact (ADR-049 §3).
                    key_index: None,
                    decision_ms,
                    started,
                    now_epoch_s,
                    plan_switch: facts.plan_switch.clone(),
                    sticky_hit,
                    transform_mode: facts.transform_mode,
                    transforms: facts.transform_records.clone(),
                    transform_error: facts.transform_error.clone(),
                },
                &primary.provider,
                &primary.model,
                sticky_hit,
                route_changed,
                self.session_ttl_us,
            );
        }

        // The exact-match response cache (spec §4.17; ADR-042 §4.4): the
        // lookup is the **last step before the upstream attempt** —
        // after admission (§4.7), after the body is read and bounded
        // (§4.13), after route resolution (§3), after the guard chain
        // (§4.6/§4.2) and after the transform compose step (§2.1) — and
        // **nowhere earlier**: an early lookup would strip the record of
        // the route it did not call. A hit replaces exactly one thing:
        // the call. A request with no session has no key — it is neither
        // looked up nor stored (fail-closed, §3.3), which is why the key
        // is derived only inside the session arm. The digest is taken
        // over the client's inbound body bytes **as received** — before,
        // and independently of, the two permitted mutations (§3.1).
        let cache_key = match (&self.response_cache, session.as_deref()) {
            (Some(_), Some(session_key)) => {
                Some(vadis_core::response_cache::ResponseKey::for_request(
                    &vadis_core::response_cache::RequestFacts {
                        protocol_in: proto_in.as_str(),
                        // The revision this request's own record is
                        // stamped with — one source (the trace writer),
                        // so the key and the record can never disagree
                        // about which revision served the request.
                        config_digest: self.trace.as_ref().map(|t| t.config_digest()).unwrap_or(""),
                        session: session_key,
                        transform_mode: facts.transform_mode,
                        body,
                    },
                ))
            }
            _ => None,
        };
        if let (Some(cache), Some(key)) = (&self.response_cache, &cache_key) {
            if let Some(recorded) = cache.lookup(key) {
                // A hit is a **replay, not a prediction** (ADR-042 §3.2):
                // these exact bytes were sent before, and the response
                // bytes returned then are returned now, verbatim — no
                // claim about what the upstream would answer now. The
                // record is the ordinary record of this request with one
                // group added: same identity, same decision (the route
                // it did NOT call), same timings — plus `cache`.
                let hit = vadis_core::trace::CacheRec {
                    // `inferred`, always (constraint 4): the counterfactual
                    // was never measured, and no gate may count it (§5).
                    verdict: "inferred",
                    key_digest: key.key_digest_hex(),
                    replayed: vadis_core::trace::ReplayedRef {
                        request_id: recorded.source.request_id.clone(),
                        session: recorded.source.session.clone(),
                        turn_index: recorded.source.turn_index,
                    },
                    replayed_bytes: recorded.body.len() as u64,
                };
                let ctx = crate::accounting::AccountCtx {
                    request_id,
                    received_event: facts.received_event,
                    proto_in: proto_in.as_str(),
                    // No bytes left the process — spec §6's third
                    // `protocol_out` class.
                    proto_out: None,
                    session: session.as_deref(),
                    turn_index,
                    selection_source,
                    requested_model: Some(model),
                    // The replay made no upstream attempt: no credential
                    // served this record (spec §6; ADR-042 §4.3).
                    key_index: None,
                    decision_ms,
                    started,
                    now_epoch_s,
                    plan_switch: facts.plan_switch.clone(),
                    sticky_hit,
                    transform_mode: facts.transform_mode,
                    transforms: facts.transform_records.clone(),
                    transform_error: facts.transform_error.clone(),
                };
                crate::accounting::Accountant {
                    store: self.store.as_deref(),
                    trace: self.trace.as_deref(),
                    // No usage exists to price: the record is
                    // `usage_missing`-class, charged nowhere and summed
                    // nowhere (spec §6, ADR-042 §4.3).
                    accounting: None,
                }
                .finish_replay(
                    &ctx,
                    hit,
                    recorded.status,
                    &primary.provider,
                    &primary.model,
                    &facts.blocks,
                );
                return ForwardOutcome::Success(ForwardSuccess {
                    status: recorded.status,
                    content_type: recorded.content_type.clone(),
                    body: Bytes::from(recorded.body),
                    route: primary.clone(),
                    failover_from: None,
                    usage: None,
                    prefix_blocks: facts.blocks.clone(),
                });
            }
        }

        // The candidate chain: the family's ACTIVE route, then the plan
        // tier's remaining members in declaration order, then the
        // family's `overflow` route, then the global fallback list
        // (spec §4.2 refined by §4.6 / ADR-049 §5.1/§5.7), walked once.
        // The guard has already moved `primary` to the active route, and
        // the tier members BEFORE the active route are the drained ones
        // — a plan that answered 403 is retired for the family until
        // the whole tier is exhausted and a probe re-admits the head
        // (§5.2's coherence rule), so they are deliberately absent from
        // this walk. Provider-exclusion semantics: a failed provider is
        // excluded whole — its other routes are not attempted either
        // (ADR-011 item 4's in-request form).
        let mut candidates: Vec<RouteSpec> = vec![primary.clone()];
        if let Some(policy) = &plan_policy {
            let tier = plan_tier(&self.config, policy);
            let metered = metered_walk(&self.config, policy);
            if route_in_family(&tier, &metered, &primary) {
                // §5.1/§5.7: the tier's members after the active route,
                // in declaration order, come before anything metered —
                // the tier is drained before a cent is spent.
                if let Some(active_pos) = tier.iter().position(|r| r == &primary) {
                    for member in tier.iter().skip(active_pos + 1) {
                        if !candidates.contains(member) {
                            candidates.push(member.clone());
                        }
                    }
                }
                // §5.7: the metered tier in walk order — the policy's
                // `overflow` alone under `declared` (today's chain),
                // the §5.6 ranking under `cheapest`.
                for member in &metered {
                    if !candidates.contains(member) {
                        candidates.push(member.clone());
                    }
                }
            }
        }
        for r in &self.config.fallback {
            if !candidates.contains(r) {
                candidates.push(r.clone());
            }
        }

        let mut attempted_providers: Vec<String> = Vec::new();
        // The family's primary refused by ADR-011's cooldown projection
        // before any attempt (spec §6's producer table, CONF-42): set by
        // the skip below, consumed by the first candidate that is actually
        // attempted.
        let mut cooling_abandoned: Option<RouteSpec> = None;
        let mut last_class: Option<ErrorClass> = None;
        let mut last_upstream_status: Option<u16> = None;
        let mut attempt_index: u32 = 0;
        // The walk's own record of which candidates it refused without
        // attempting, and why (ADR-022 / spec §8): the input of the frozen
        // `no_available_route` refusal, should the walk end with nothing
        // served. One entry per eligibility skip, in the chain's order.
        let mut skipped: Vec<(RouteSpec, &'static str)> = Vec::new();

        for candidate in &candidates {
            if attempted_providers.iter().any(|p| p == &candidate.provider) {
                continue;
            }
            if self.provider_in_cooldown(&candidate.provider) {
                // The pre-attempt cooldown skip (ADR-011 item 4; spec §6's
                // `failover_from` table, row 2): the projection refused
                // this candidate before any attempt, so the walk moves on
                // — the skip *abandons* the route and `failover_from`
                // names it, exactly like a failed attempt would.
                failover_origin(&mut facts.failover_from, candidate);
                skipped.push((candidate.clone(), SKIP_DEMOTED));
                if let Some(policy) = &plan_policy {
                    if candidate == &policy.primary {
                        cooling_abandoned = Some(candidate.clone());
                    }
                }
                continue;
            }
            let Some(cand_provider) = self.provider(candidate).cloned() else {
                // ADR-022's keyless class: the walk's own skip, not an
                // answer — the chain continues.
                skipped.push((candidate.clone(), SKIP_UNKNOWN_PROVIDER));
                continue;
            };
            // The wire gate (ADR-022 / DESIGN §12.10.9): a candidate may
            // only be served on its own wire. `wire_api ∈ supports` holds
            // by config validation, so this one condition subsumes the
            // inbound protocol being a declared cell. The skip is in the
            // keyless class — no attempt, no intent row, no
            // classification, no `failover_from` (nothing failed and
            // nothing moved), narrated only at the walk's end.
            if cand_provider.wire_api != proto_in {
                skipped.push((candidate.clone(), SKIP_WIRE_MISMATCH));
                continue;
            }
            let Some(transport) = self.transports.get(&candidate.provider) else {
                // No transport at startup (missing api key): the provider
                // is unavailable (§12.10.2), reported by /health; the
                // chain continues. ADR-023 Decision 3: the provider-level
                // exclusion must not swallow the provider's other models —
                // a keyless provider is narrated per candidate, so its
                // second model below gets its own `keyless` entry instead
                // of vanishing at the loop's first test. Only an
                // **attempted** provider is excluded whole (ADR-011 item
                // 4's in-request form), and an attempt always leaves this
                // loop by return.
                skipped.push((candidate.clone(), SKIP_KEYLESS));
                continue;
            };
            attempted_providers.push(candidate.provider.clone());

            // The cooling displacement's record (spec §6's producer table,
            // CONF-42): the family's primary was refused by the cooldown
            // projection before any attempt and THIS candidate is the one
            // actually serving the request — `to` is the route that
            // answered, not a config guess. Trace row only: no
            // `plan.switched` event and no `plan_state` move, because a
            // cooldown is route availability, not a verdict on the plan
            // (§4.6 rule 3). The figures follow the same convention a
            // failover prices (ADR-011 item 9): the session's ledger
            // tokens at the destination route's miss price, both null
            // when no ledger exists.
            if let Some(abandoned) = cooling_abandoned.take() {
                if facts.plan_switch.is_none() {
                    let (reprefill, cost_nano) = self.failover_cost(session.as_deref(), candidate);
                    facts.plan_switch = Some(PlanSwitchRec {
                        from: abandoned.to_string(),
                        to: candidate.to_string(),
                        reason: REASON_PRIMARY_COOLING_DOWN,
                        probe: false,
                        reprefill_tokens: reprefill,
                        switch_cost_nano: cost_nano,
                        cost_currency: self.route_currency(candidate),
                        candidates: plan_policy
                            .as_ref()
                            .and_then(|p| metered_candidates_field(&self.config, p)),
                        chosen: Some(candidate.to_string()),
                    });
                }
            }

            // ADR-049 §3 rules 1/6: the provider's **present** pool, in
            // declaration order (rotation order); an attempt is a
            // (route, key_index) pair and a request never re-attempts a
            // key it has already tried. A one-credential provider (either
            // spelling) has a one-entry pool — its walk is exactly what
            // it was before the pool existed.
            let pool: Vec<String> = self
                .api_keys
                .get(&candidate.provider)
                .cloned()
                .unwrap_or_default();
            let multi_key = pool.len() > 1;
            let mut key_cursor: usize = 0;
            // spec §4.9 / ADR-020: the entry states the URL for the wire this
            // attempt uses, and it is used verbatim. `validate` refused any
            // config whose declared cell has no URL, so a miss here is an
            // internal error to answer — not a case to paper over by sending
            // the request to a guessed endpoint.
            let Some(url) = cand_provider.url_for(cand_provider.wire_api) else {
                return ForwardOutcome::Failure(ForwardFailure::new(
                    500,
                    ErrorCode::Internal,
                    format!(
                        "provider '{}' declares '{}' but carries no URL for it",
                        cand_provider.name,
                        cand_provider.wire_api.as_str()
                    ),
                ));
            };
            // The rotation arm (ADR-049 §3 rule 3): a credential-class
            // failure retries the SAME route on the pool's next key
            // before any fallover; only an exhausted pool leaves the
            // provider. One loop per provider: the outer candidate loop
            // owns provider eligibility, this inner one owns the pool.
            'keys: loop {
                let api_key = pool.get(key_cursor).cloned().unwrap_or_default();
                let plan = UpstreamPlan {
                    provider: &candidate.provider,
                    model: &candidate.model,
                    protocol_out: cand_provider.wire_api,
                    url,
                    api_key: &api_key,
                    attempt: attempt_index,
                };

                // Mutation (b), per attempt (DESIGN §12.10.7): the value of the
                // top-level `model` member becomes this route's native id, every
                // other byte the client's. `set_top_level_string` cannot fail
                // here — `model` was parsed as a string above, and mutation (a)
                // never removes it — but a failure is still answered, never
                // ignored, so no unrewritten bytes can reach a provider.
                // Mutation (b), per attempt (DESIGN §12.10.7): over `base` —
                // the transform-mode edits are already spliced in as value
                // spans, so the model rewrite and the payload edits compose
                // without either disturbing the other.
                let rewritten = match rewrite_outbound_model(&base, &candidate.model) {
                    Ok(b) => b,
                    Err(e) => {
                        return ForwardOutcome::Failure(ForwardFailure::new(
                            500,
                            ErrorCode::Internal,
                            format!("cannot rewrite the outbound model to the native id: {e:?}"),
                        ))
                    }
                };
                let attempt_hash = body_sha16(rewritten.as_ref());

                // Row 5 — the intent, FULL, committed before the wire. Nothing
                // is written between the commit and the attempt (CONF-20). The
                // payload carries the session's `prefix_blocks` — the encoder
                // computed them from exactly these bytes, which is what the
                // cache_ledger rebuild reads (§12.10.6, CONF-21). `body_hash`
                // is that attempt's byte-final bytes (§12.10.5 note R4): the rewrite is
                // done, so the hash names exactly what goes to the wire.
                // Cloned (not taken): a failed attempt continues to the next
                // candidate, which re-reads the same cleaned-body blocks.
                let blocks_now = facts.blocks.clone();
                let intent_id = if let Some(store) = &self.store {
                    let intent = NewEvent {
                        kind: EventKind::UpstreamSubmitted,
                        request_id: Some(request_id),
                        session: session.as_deref(),
                        body_hash: Some(&attempt_hash),
                        trace_ref: None,
                        payload: json!({
                            "route": format!("{}/{}", candidate.provider, candidate.model),
                            "attempt_index": attempt_index,
                            "protocol_out": cand_provider.wire_api.as_str(),
                            "prefix_blocks": blocks_now.iter().map(|b| json!({
                                "index": b.index, "kind": b.kind.as_str(),
                                "tokens": b.tokens, "hash": b.hash,
                            })).collect::<Vec<_>>(),
                        }),
                    };
                    match store.append(intent) {
                        Ok(id) => Some(id),
                        Err(_) => {
                            // CONF-22: nothing reached the upstream; the
                            // client's retry is safe.
                            return ForwardOutcome::Failure(ForwardFailure {
                                status: 500,
                                code: ErrorCode::Internal,
                                message: "the state store rejected the upstream intent".into(),
                                details: Some(json!({"stage": "intent"})),
                            });
                        }
                    }
                } else {
                    None
                };

                let req = match vadis_providers::build_request(&plan, rewritten.as_ref()) {
                    Ok(r) => r,
                    Err(e) => {
                        return ForwardOutcome::Failure(ForwardFailure::new(
                            500,
                            ErrorCode::Internal,
                            format!("cannot build the outbound request: {e}"),
                        ))
                    }
                };
                let attempt_started = Instant::now();
                let outcome = transport.send_boxed(req).await;
                let latency_us = attempt_started.elapsed().as_micros() as i64;
                let upstream_ms = Some((latency_us / 1000) as u32);
                facts.upstream_ms = upstream_ms;
                facts.attempted_route = Some(candidate.clone());

                match outcome {
                    AttemptOutcome::Responded(resp) => {
                        last_upstream_status = Some(resp.status);
                        facts.last_upstream_status = Some(resp.status);
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
                            session.as_deref(),
                        );
                        if (200..300).contains(&resp.status) {
                            // A probe succeeded (ADR-014 item 3; ADR-049
                            // §5.2: the probe attempts the tier's head): a
                            // 2xx on a plan route while the family's
                            // active route was in the metered tier flips
                            // the family back and records it — the
                            // probe's success *is* the transition.
                            if plan_guard_out.as_ref().is_some_and(|g| g.probe) {
                                if let Some(policy) = plan_policy.as_ref() {
                                    // The return trip's trace row (spec §6:
                                    // "the family's account state returning
                                    // to `primary` | `failover_from`: null |
                                    // `plan_switch`: set, `reason`:
                                    // `primary_recovered`"): the probing
                                    // request's own displacement record, set
                                    // HERE — only a 2xx is a recovery. The
                                    // displacement is from the STATE's route
                                    // (the metered tier), not from the
                                    // resolution's; the way back costs 0
                                    // (in-plan destination).
                                    if facts.plan_switch.is_none() {
                                        facts.plan_switch = Some(PlanSwitchRec {
                                            from: policy.overflow.to_string(),
                                            to: candidate.to_string(),
                                            // §5.2: the destination is the
                                            // route the probe attempted —
                                            // the tier's head (== the
                                            // policy's primary).
                                            reason: REASON_PRIMARY_RECOVERED,
                                            probe: true,
                                            reprefill_tokens: None,
                                            switch_cost_nano: Some(0),
                                            cost_currency: self.route_currency(&policy.primary),
                                            candidates: metered_candidates_field(&self.config, policy),
                                            chosen: Some(candidate.to_string()),
                                        });
                                    }
                                    self.plan_probe_succeeded(
                                        request_id,
                                        policy,
                                        session.as_deref(),
                                    );
                                }
                            }
                            let mut blocks = blocks_now;
                            if let Some(u) = &usage {
                                attribute_tokens(&mut blocks, u);
                            }
                            facts.blocks = blocks.clone();
                            let route_acc =
                                crate::accounting::route_accounting(&self.config, candidate);
                            let accountant = crate::accounting::Accountant {
                                store: self.store.as_deref(),
                                trace: self.trace.as_deref(),
                                accounting: route_acc.as_ref(),
                            };
                            // §12.10.5 note R2's order: trace line, then cost/quota rows.
                            // The continuity measurement inside `finish` reads
                            // the session's PREVIOUS block set, so the ledger
                            // put (replacing it with this request's blocks for
                            // the NEXT request) must run after it — measure,
                            // then replace (§12.10.6).
                            let ctx = crate::accounting::AccountCtx {
                                request_id,
                                received_event: facts.received_event,
                                proto_in: proto_in.as_str(),
                                proto_out: Some(cand_provider.wire_api.as_str()),
                                session: session.as_deref(),
                                turn_index,
                                selection_source,
                                requested_model: Some(model),
                                // The pool credential that served (ADR-049 §3):
                                // an index only, and only when the pool is real.
                                key_index: multi_key.then_some(key_cursor as u32),
                                decision_ms,
                                started,
                                now_epoch_s,
                                plan_switch: facts.plan_switch.clone(),
                                sticky_hit,
                                transform_mode: facts.transform_mode,
                                transforms: facts.transform_records.clone(),
                                transform_error: facts.transform_error.clone(),
                            };
                            let _accounted = accountant.finish(
                                &ctx,
                                &ForwardSuccess {
                                    status: resp.status,
                                    content_type: resp.content_type.clone(),
                                    body: resp.body.clone(),
                                    route: candidate.clone(),
                                    failover_from: facts.failover_from.clone(),
                                    usage,
                                    prefix_blocks: blocks.clone(),
                                },
                                &blocks,
                                upstream_ms,
                            );
                            if let (Some(id), Some(s)) = (intent_id, session.as_deref()) {
                                accountant.put_ledger(Some(s), &blocks, id);
                            }
                            // The store's write side (spec §4.17, ADR-042
                            // §3.3/§3.4): this miss answered with a COMPLETE
                            // `2xx` body — complete by construction (the
                            // buffered read), the status checked above — so
                            // it is a recording candidate. The source
                            // reference is THIS request's record: the bytes
                            // a later hit replays are attributed to the one
                            // measurement, and the session component comes
                            // from the key, so the reference can never point
                            // across sessions (§4.3). The byte bound and the
                            // status rule are the store's own to enforce
                            // (fail-closed).
                            if let (Some(cache), Some(key)) = (&self.response_cache, &cache_key) {
                                cache.record(
                                    key.clone(),
                                    vadis_core::response_cache::RecordedResponse {
                                        status: resp.status,
                                        content_type: resp.content_type.clone(),
                                        body: resp.body.to_vec(),
                                        source: vadis_core::response_cache::SourceRef {
                                            request_id: request_id.to_string(),
                                            session: key.session().to_string(),
                                            turn_index,
                                        },
                                    },
                                );
                            }
                            return ForwardOutcome::Success(ForwardSuccess {
                                status: resp.status,
                                content_type: resp.content_type,
                                body: resp.body,
                                route: candidate.clone(),
                                failover_from: facts.failover_from.clone(),
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
                            transport_cause: None,
                        };
                        let cls = classify_upstream_error(&evidence);
                        last_class = Some(cls.class);
                        // The rotation arm (ADR-049 §3 rule 3, CONF-91(a)):
                        // a credential-class failure whose pool still holds an
                        // untried key rotates WITHIN the provider — the
                        // classification row says `rotate_credential` with the
                        // index rotated to, no demotion (the credential is not
                        // the provider), `failover_from` untouched (the walk
                        // never left), and the SAME route is re-attempted on
                        // the next key. `quota_exhausted` never takes this
                        // arm (rule 4): it is a fact about the account, and
                        // the negative below pins it.
                        if cls.class.is_credential_class() && key_cursor + 1 < pool.len() {
                            let (reprefill, cost_nano) =
                                self.failover_cost(session.as_deref(), candidate);
                            self.record_rotation(
                                request_id,
                                attempt_index,
                                Some(resp.status),
                                &cls,
                                (key_cursor + 1) as u32,
                                reprefill,
                                cost_nano,
                            );
                            key_cursor += 1;
                            attempt_index += 1;
                            continue 'keys;
                        }
                        let classified_id = self.record_classification(
                            request_id,
                            attempt_index,
                            Some(resp.status),
                            &cls,
                        );
                        // The demotion projection (state, not a local var),
                        // riding on the classification event (ADR-011 item 4).
                        self.apply_demotion(&candidate.provider, &cls, classified_id);
                        // The plan policy's only account-moving signal
                        // (ADR-014 item 2; ADR-049 §5.2 across the tier):
                        // an upstream 403 classified `quota_exhausted` on
                        // one of the family's PLAN routes retires that
                        // plan for the family and moves the family to the
                        // walk's next step — the next plan member if the
                        // tier still holds one (`plan_exhausted`, the
                        // family stays in-plan), the metered tier
                        // otherwise (`primary_exhausted`) — recorded as
                        // `plan.switched` (FULL) before the next intent.
                        // The request then continues per
                        // `on_primary_exhausted`: spill walks on; block
                        // refuses — now while the WHOLE tier is
                        // exhausted.
                        if cls.class.demotes_provider() {
                            if let Some(policy) = plan_policy.as_ref() {
                                let tier = plan_tier(&self.config, policy);
                                let metered = metered_walk(&self.config, policy);
                                if tier.contains(candidate) {
                                    let from = candidate.clone();
                                    let next_plan = tier
                                        .iter()
                                        .position(|r| r == candidate)
                                        .and_then(|pos| tier.get(pos + 1))
                                        .cloned();
                                    let to = next_plan
                                        .clone()
                                        .unwrap_or_else(|| metered.first().cloned().unwrap_or_else(|| policy.overflow.clone()));
                                    let to_account =
                                        account_of_route(&tier, &to).as_str().to_string();
                                    self.record_plan_switch(
                                        request_id,
                                        policy,
                                        &from,
                                        &to,
                                        &to_account,
                                        false,
                                        session.as_deref(),
                                    );
                                    if policy.on_primary_exhausted
                                        == vadis_core::config::OnPrimaryExhausted::Block
                                    {
                                        // Block refuses only once every
                                        // plan is drained — i.e. when the
                                        // move above left the tier.
                                        if to_account == "overflow" {
                                            return ForwardOutcome::Failure(ForwardFailure {
                                                status: 429,
                                                code: ErrorCode::QuotaExceeded,
                                                message: format!(
                                                    "plan family '{}' is exhausted and \
                                                 on_primary_exhausted is 'block': the request is \
                                                 refused rather than served from the metered \
                                                 account (spec 4.6)",
                                                    policy.family
                                                ),
                                                details: Some(json!({
                                                    "family": policy.family,
                                                    "account_state": "overflow",
                                                    "reason": "quota_exhausted",
                                                })),
                                            });
                                        }
                                    }
                                    // Spill: this request's displacement
                                    // record (the same direction-derived
                                    // reason the event carries).
                                    if facts.plan_switch.is_none() {
                                        let (reprefill, cost_nano) =
                                            self.failover_cost(session.as_deref(), &to);
                                        facts.plan_switch = Some(PlanSwitchRec {
                                            from: from.to_string(),
                                            to: to.to_string(),
                                            reason: displacement_reason(
                                                policy, &tier, &metered, &from, &to,
                                            ),
                                            probe: false,
                                            reprefill_tokens: reprefill,
                                            switch_cost_nano: cost_nano,
                                            cost_currency: self.route_currency(&to),
                                            candidates: metered_candidates_field(&self.config, policy),
                                            chosen: Some(to.to_string()),
                                        });
                                    }
                                }
                            }
                        }
                        if let Some(next) = self.next_candidate(
                            &candidates,
                            &attempted_providers,
                            cls.class,
                            proto_in,
                        ) {
                            {
                                let (reprefill, switch_cost) =
                                    self.failover_cost(session.as_deref(), &next);
                                self.record_failover(
                                    request_id,
                                    attempt_index,
                                    candidate,
                                    &next,
                                    &cls,
                                    session.as_deref(),
                                    reprefill,
                                    switch_cost,
                                );
                            }
                            failover_origin(&mut facts.failover_from, candidate);
                            attempt_index += 1;
                            break 'keys;
                        }
                        return ForwardOutcome::Failure(
                            self.exhausted_failure(last_upstream_status, cls.class),
                        );
                    }
                    AttemptOutcome::NotSent(err) => {
                        // No request bytes went out: nothing billed, failover
                        // allowed (ADR-011 item 6 row 1). No upstream.responded
                        // row — the attempt never reached the upstream. The
                        // transport kind the provider layer derived from the
                        // reqwest error (`is_connect()` / `is_timeout()`) is
                        // the classification evidence: a connect failure is
                        // `connect_failure` and walks the chain, a connect
                        // timeout keeps the `timeout` verdict.
                        let evidence = ErrorEvidence {
                            status: None,
                            retry_after: None,
                            body: b"",
                            wrote_full_request: false,
                            transport_cause: Some(match err.kind {
                                TransportKind::Connect => TransportCause::Connect,
                                TransportKind::Timeout => TransportCause::Timeout,
                                TransportKind::Other => TransportCause::Other,
                            }),
                        };
                        let cls = classify_upstream_error(&evidence);
                        last_class = Some(cls.class);
                        // The rotation arm, transport twin (ADR-049 §3 rule
                        // 3): the same predicate the answered-failure arm
                        // applies — no bytes were billed on this attempt, so
                        // the rotation is exactly as safe as the fallover it
                        // precedes.
                        if cls.class.is_credential_class() && key_cursor + 1 < pool.len() {
                            let (reprefill, cost_nano) =
                                self.failover_cost(session.as_deref(), candidate);
                            self.record_rotation(
                                request_id,
                                attempt_index,
                                None,
                                &cls,
                                (key_cursor + 1) as u32,
                                reprefill,
                                cost_nano,
                            );
                            key_cursor += 1;
                            attempt_index += 1;
                            continue 'keys;
                        }
                        let _classified_id =
                            self.record_classification(request_id, attempt_index, None, &cls);
                        if let Some(next) = self.next_candidate(
                            &candidates,
                            &attempted_providers,
                            cls.class,
                            proto_in,
                        ) {
                            {
                                let (reprefill, switch_cost) =
                                    self.failover_cost(session.as_deref(), &next);
                                self.record_failover(
                                    request_id,
                                    attempt_index,
                                    candidate,
                                    &next,
                                    &cls,
                                    session.as_deref(),
                                    reprefill,
                                    switch_cost,
                                );
                            }
                            failover_origin(&mut facts.failover_from, candidate);
                            attempt_index += 1;
                            break 'keys;
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
            } // 'keys: every outcome returns or fails over in-arm; a pool
              // exhausted by rotations reaches the match's own fallover
              // paths with the cursor at the pool's end.
        }

        // Every candidate was skipped (demoted providers, missing keys,
        // wire mismatches): the frozen `no_available_route` refusal
        // (ADR-022 / spec §8; ADR-023 condition N) — the same body both
        // media return, so a client cannot tell which path produced it.
        // `skipped[]` is the machine-readable truth; the sentence is
        // frozen verbatim. By ADR-023 Decision 2 the narration predicate
        // is the eligibility predicate, so an attempt-bearing ending
        // cannot reach this point: every attempt outcome returns in-loop
        // (it serves, or its `next_candidate` is `None` and the
        // attempt-exhausted body is emitted there). The evidence members
        // are therefore `null` by construction — nothing was contacted.
        ForwardOutcome::Failure(ForwardFailure {
            status: 502,
            code: ErrorCode::UpstreamError,
            message:
                "no available route: every candidate provider is demoted, keyless or unavailable"
                    .into(),
            details: Some(json!({
                "stage": "no_available_route",
                "skipped": skipped_json(&skipped),
                "upstream_status": last_upstream_status,
                "error_class": last_class.map(|c| c.as_str()),
            })),
        })
    }

    // -- helpers -----------------------------------------------------------

    /// The plan policy's Guard stage (spec §4.6, evaluated before the
    /// allowance rule): for a request inside the family, where does it
    /// go? `Ok(None)` when there is no family policy for this
    /// request's resolution or the route is outside the family — the
    /// caller proceeds untouched. Shared by both forwarding paths (the
    /// same request, the same guard, whichever medium carries it).
    /// The policy is the caller's own resolution
    /// ([`VadisConfig::family_policy_for_route`], the single owner) —
    /// the guard itself is family-agnostic.
    pub(crate) fn plan_guard(
        &self,
        policy: Option<&PlanPolicyCfg>,
        resolved: &RouteSpec,
        session: Option<&str>,
        turn_index: u32,
        now_epoch_s: u64,
        now_us: i64,
    ) -> Result<Option<PlanGuardOutcome>, ForwardFailure> {
        let Some(policy) = policy else {
            return Ok(None);
        };
        // ADR-049 §5.1: the family's plan tier — every roster route of an
        // `account: coding_plan` provider carrying the family tag,
        // declaration order, `primary` pinned as the head. A
        // single-member tier (every config before this ADR) is exactly
        // the pre-tier shape.
        let tier = plan_tier(&self.config, policy);
        let metered = metered_walk(&self.config, policy);
        if !route_in_family(&tier, &metered, resolved) {
            return Ok(None);
        }
        // The state read (projection; absent ⇒ the family never switched,
        // and its active route is the tier's head). ADR-049 §5.2: the row
        // carries the active route; the two-valued word is derived from
        // it against the tier. A legacy row with no route falls back to
        // the word it stored (a pre-tier database migrating forward).
        let state = match self.store.as_ref().map(|s| {
            s.query(Query::PlanState {
                family: &policy.family,
            })
        }) {
            Some(Ok(QueryRow::PlanState(Some(row)))) => {
                let route = if row.route.is_empty() {
                    match row.account.as_str() {
                        "overflow" => policy.overflow.clone(),
                        _ => policy.primary.clone(),
                    }
                } else {
                    match row.route.split_once('/') {
                        Some((provider, model)) => RouteSpec {
                            provider: provider.to_string(),
                            model: model.to_string(),
                        },
                        None => policy.primary.clone(),
                    }
                };
                PlanStateRow {
                    route,
                    since_us: row.since_us,
                }
            }
            _ => PlanStateRow {
                route: policy.primary.clone(),
                since_us: 0,
            },
        };
        // ADR-049 §5.7: the metered tier in walk order (the policy's
        // `overflow` under `declared`, the §5.6 ranking under `cheapest`)
        // — the guard's `Downgrade` names its head, so the guard and the
        // walk cannot disagree about where a spill starts.
        let rule = PlanFirstRule::with_metered(policy.clone(), tier.clone(), metered.clone());
        let req = PlanRequest {
            session,
            turn_index,
            state: state.clone(),
            // The µs word of the same read `/health` clocks itself with
            // Deriving it back from the truncated seconds
            // word made the guard refuse for up to ~1s after the
            // surface already said `admitted: true`.
            now_us,
            // ADR-011's route-availability answer for the tier's head
            // (the two constraints are read together, merged nowhere).
            // The read lives in `availability` — the single owner both
            // this guard and `/health` call (ADR-016 §13.3 L1c) —
            // against this request's own clock word below.
            primary_allowed: !crate::availability::provider_in_cooldown(
                self.store.as_ref(),
                &policy.primary.provider,
                now_us,
            ),
            // The local counter's only influence (spec §4.6 rule 3): it
            // may defer a probe until the plan's declared window
            // boundary. Computed only when a probe is otherwise on the
            // table (the active route off-tier + recover: probe); never
            // a Reject and never a forced spill. The adjudication lives
            // in `availability` — the single owner (ADR-016 §13.3 L1d).
            deferred_by_window: crate::availability::probe_deferred_by_window(
                self.store.as_ref(),
                &self.config,
                policy,
                now_epoch_s,
            ),
            overflow_spend: self.overflow_spend(policy),
        };
        match rule.decide(&req) {
            PlanMove::Pass { route, probe } => Ok(Some(PlanGuardOutcome { route, probe })),
            PlanMove::Downgrade { route } => Ok(Some(PlanGuardOutcome {
                route,
                probe: false,
            })),
            PlanMove::Reject { code, message } => Err(ForwardFailure {
                status: code.http_status(),
                code,
                message,
                details: Some(json!({
                    "family": policy.family,
                    "account_state": account_of_route(&tier, &state.route).as_str(),
                    "reason": code.as_str(),
                })),
            }),
        }
    }

    /// The family's measured metered spend this UTC month (DESIGN
    /// §12.10.8: a SUM over the overflow route's `cost.computed` rows —
    /// no counter exists to disagree with the log). ADR-049 §5.8: under
    /// `cheapest` the cap's subject is the family's WHOLE metered spend
    /// — every candidate's route summed; under `declared` the single
    /// `overflow` route, today's subject.
    fn overflow_spend(&self, policy: &PlanPolicyCfg) -> vadis_core::Nano {
        let Some(store) = &self.store else {
            return vadis_core::Nano(0);
        };
        let month_start_us = month_start_us(now_us());
        let routes: Vec<String> = metered_walk(&self.config, policy)
            .iter()
            .map(|r| format!("{}/{}", r.provider, r.model))
            .collect();
        let mut total: i64 = 0;
        for route in &routes {
            match store.query(Query::OverflowSpend {
                family_route: route,
                month_start_us,
            }) {
                Ok(QueryRow::Count(n)) => total += n,
                _ => {}
            }
        }
        vadis_core::Nano(total.max(0) as u64)
    }

    fn provider(&self, route: &RouteSpec) -> Option<&ProviderCfg> {
        self.config
            .providers
            .iter()
            .find(|p| p.name == route.provider)
    }

    /// A route's serving entry currency (spec §4.8) — the unit a
    /// displacement's figures are denominated in. The USD default when
    /// the route is somehow off-roster can never mislead: the record's
    /// own `decision` names a roster route or the request failed before
    /// one resolved.
    pub(crate) fn route_currency(&self, route: &RouteSpec) -> vadis_core::Currency {
        self.provider(route)
            .map(|p| p.currency)
            .unwrap_or(vadis_core::Currency::Usd)
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

    /// ADR-011's route-availability answer for the provider right now
    /// (the walk-skip reads, `forward.rs:953`/`:1683` and the streaming
    /// twin). Delegates to `availability::provider_in_cooldown` — the
    /// single owner (ADR-016 §13.3 L1c) — on this method's own clock
    /// read; `plan_guard` passes its request-clock word directly instead
    /// of coming through here, so the guard's inputs share one instant.
    pub(crate) fn provider_in_cooldown(&self, provider: &str) -> bool {
        crate::availability::provider_in_cooldown(self.store.as_ref(), provider, now_us())
    }

    /// `error.classified` (NORMAL) — §12.10.5 row 7, ADR-011 item 8. Every
    /// classification writes one event, even when the action is abort.
    /// Returns the event id the demotion projection rides on. Shared by
    /// both forwarding paths: the same failure gets the same row
    /// whichever medium carried it.
    pub(crate) fn record_classification(
        &self,
        request_id: &str,
        attempt_index: u32,
        status: Option<u16>,
        cls: &Classification,
    ) -> Option<vadis_core::EventId> {
        self.record_classification_inner(request_id, attempt_index, status, cls, None)
    }

    /// The rotation twin (ADR-049 §3 rules 3/6): the classification row
    /// of a credential-class failure whose remedy is the **next key of
    /// the same provider's pool** — `action: "rotate_credential"`, the
    /// payload naming the index rotated **to** (`key_index`, an integer;
    /// the key's value is never a field — spec §4.7, CONF-92's
    /// boundary), plus the re-prefill figures under ADR-011 item 9's
    /// convention (inferred at decision time from the session ledger;
    /// verified only when usage lands). `failover_from` stays `null`:
    /// the walk never left the provider (CONF-91(a)).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_rotation(
        &self,
        request_id: &str,
        attempt_index: u32,
        status: Option<u16>,
        cls: &Classification,
        to_key_index: u32,
        reprefill_tokens: Option<u64>,
        switch_cost_nano: Option<u64>,
    ) -> Option<vadis_core::EventId> {
        self.record_classification_inner(
            request_id,
            attempt_index,
            status,
            cls,
            Some((to_key_index, reprefill_tokens, switch_cost_nano)),
        )
    }

    fn record_classification_inner(
        &self,
        request_id: &str,
        attempt_index: u32,
        status: Option<u16>,
        cls: &Classification,
        rotation: Option<(u32, Option<u64>, Option<u64>)>,
    ) -> Option<vadis_core::EventId> {
        let store = self.store.as_ref()?;
        let action = if rotation.is_some() {
            "rotate_credential"
        } else if cls.class.fails_over() {
            // ADR-011 item 7 + ADR-049 §3: the pool's rotation arm runs
            // before this action — a credential-class failure that
            // reaches here has exhausted the pool, so the provider is
            // left and the fallback answers.
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
        let (key_index, reprefill, switch_cost) = match rotation {
            Some((k, r, c)) => (Some(json!(k)), r.map(|t| json!(t)), c.map(|n| json!(n))),
            None => (None, None, None),
        };
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
                    "key_index": key_index,
                    "reprefill_tokens": reprefill,
                    "switch_cost_nano": switch_cost,
                }),
            })
            .ok()
    }

    /// The next unattempted candidate after a failure, when the class may
    /// fail over. Provider-exclusion: a provider already attempted in this
    /// request is never re-attempted (spec §4.2 + ADR-011 item 4).
    fn next_candidate(
        &self,
        candidates: &[RouteSpec],
        attempted_providers: &[String],
        class: ErrorClass,
        proto_in: WireApi,
    ) -> Option<RouteSpec> {
        if !class.fails_over() {
            return None;
        }
        // ADR-023 Decision 2: the narration predicate *is* the eligibility
        // predicate — the same conditions the walk's own loop applies, so a
        // destination is named only if the walk will actually attempt it.
        // The loop's order is kept (provider entry, wire, key/transport,
        // not-attempted, not-in-cooldown) so both statements of one rule
        // cannot drift again.
        candidates
            .iter()
            .find(|c| {
                !attempted_providers.contains(&c.provider)
                && !self.provider_in_cooldown(&c.provider)
                // The provider check doubles as existence: no entry, no
                // wire to compare, no candidate.
                && self.provider(c).is_some_and(|p| p.wire_api == proto_in)
                && self.transports.contains_key(&c.provider)
            })
            .cloned()
    }

    pub(crate) fn append_event(
        &self,
        kind: EventKind,
        request_id: &str,
        body_hash: Option<&str>,
        payload: Value,
        session: Option<&str>,
    ) -> Option<vadis_core::EventId> {
        let store = self.store.as_ref()?;
        store
            .append(NewEvent {
                kind,
                request_id: Some(request_id),
                session,
                body_hash,
                trace_ref: None,
                payload,
            })
            .ok()
    }

    /// `plan.switched` (FULL, §12.10.5 row 15 / ADR-014 item 8) plus the
    /// `plan_state` projection riding on it: the family's active route
    /// moved. Payload: `family`, `from_account`/`to_account` (derived
    /// from the routes against the tier, §5.2), `from_route`/`to_route`,
    /// `reason` (`primary_exhausted` | `plan_exhausted` |
    /// `primary_recovered` — decided by direction, the same single owner
    /// the walk uses), `probe`, `reprefill_tokens` + `switch_cost_nano`
    /// (both inferred — the same figures a failover prices, ADR-011
    /// item 9's convention; a move that stays in-plan costs 0 because an
    /// in-plan destination's marginal price is 0, §4.6 rule 4), the
    /// `session` that carried the evidence, and `probe_eligible_us`
    /// (`since_us +` the then-current cooldown — what the projection's
    /// `until_us` is rebuilt from).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_plan_switch(
        &self,
        request_id: &str,
        policy: &PlanPolicyCfg,
        from: &RouteSpec,
        to: &RouteSpec,
        to_account: &str,
        probe: bool,
        session: Option<&str>,
    ) -> Option<vadis_core::EventId> {
        let store = self.store.as_ref()?;
        let tier = plan_tier(&self.config, policy);
        let metered = metered_walk(&self.config, policy);
        let reason = displacement_reason(policy, &tier, &metered, from, to);
        let from_account = account_of_route(&tier, from).as_str();
        let from_route = format!("{}/{}", from.provider, from.model);
        let to_route = format!("{}/{}", to.provider, to.model);
        // The switch's cache price (ADR-014 item 5): reprefill_tokens =
        // the session's prefix tokens from the ledger (inferred,
        // GAP-Q14), priced at the destination route's miss price — 0
        // inside the plan tier (in-plan), the metered table leaving it.
        let dest_in_plan = tier.contains(to);
        let (reprefill, cost_nano) = match (session, dest_in_plan) {
            (Some(s), false) => self.failover_cost(Some(s), to),
            _ => {
                // A move that stays in-plan costs 0 by definition, but
                // the token work is still recorded when a ledger exists.
                let (tok, _) = self.failover_cost(session, to);
                (tok, Some(0))
            }
        };
        let cooldown_us = policy.cooldown_us();
        // §4.8: the destination route's unit denominates switch_cost_nano
        // (the same destination the figures were priced at).
        let cost_currency = self.route_currency(to);
        let ev = store
            .append(NewEvent {
                kind: EventKind::PlanSwitched,
                request_id: Some(request_id),
                session,
                body_hash: None,
                trace_ref: None,
                payload: json!({
                    "family": policy.family,
                    "from_account": from_account,
                    "to_account": to_account,
                    "from_route": from_route,
                    "to_route": to_route,
                    "reason": reason,
                    "probe": probe,
                    "reprefill_tokens": reprefill,
                    "switch_cost_nano": cost_nano,
                    // spec §4.8 (ADR-018): the unit switch_cost_nano is
                    // denominated in — the destination route's currency.
                    "currency": cost_currency.as_code(),
                    "session": session,
                    // What the projection's informational until_us is
                    // rebuilt from (DESIGN §12.10.8's rebuild rule).
                    "cooldown_ms": policy.cooldown.0 as i64,
                }),
            })
            .ok()?;
        let _ = store.project(ProjectionWrite::PlanSwitched {
            family: &policy.family,
            account: to_account,
            to_route: &to_route,
            cooldown_us,
            last_event: ev,
        });
        // Re-point every live session bound to the abandoned route
        // (ADR-014 items 1/3: the account move is a property of the
        // binding; new sessions follow the state on their own).
        if let Ok(QueryRow::SessionBindings(bound)) = store.query(Query::SessionBindingsFor {
            provider: &from.provider,
            model: &from.model,
        }) {
            for b in bound {
                // Row 4's own move arm (DESIGN §12.10.5 note R7, the
                // R28-1 freeze): the re-point IS a binding write — one
                // `session.bound` row per re-pointed session, the same
                // payload shape `Accountant::bind_session` writes, and
                // the `sessions` projection rides on THAT row
                // (`last_event` = the move row's id, never the
                // `plan.switched` id), so an event-derived rebuild
                // reproduces the incremental projection element for
                // element (ADR-010). A failed append writes no
                // projection either (the strict pairing): the session
                // stays on the abandoned route until its next request
                // self-heals via `route_changed` — the account state
                // itself is already durable in the `plan.switched` row
                // above, and the request is unaffected (ADR-009 item 8).
                let Ok(move_ev) = store.append(NewEvent {
                    kind: EventKind::SessionBound,
                    request_id: Some(request_id),
                    session: Some(&b.session_key),
                    body_hash: None,
                    trace_ref: None,
                    payload: json!({
                        "session_key": b.session_key.as_str(),
                        "provider": to.provider.as_str(),
                        "model": to.model.as_str(),
                        "ttl_us": self.session_ttl_us,
                    }),
                }) else {
                    continue;
                };
                let _ = store.project(ProjectionWrite::SessionBound {
                    session_key: &b.session_key,
                    provider: &to.provider,
                    model: &to.model,
                    ttl_us: self.session_ttl_us,
                    last_event: move_ev,
                });
            }
        }
        Some(ev)
    }

    /// A probe succeeded (a 2xx answered on a plan route while the
    /// family's active route was in the metered tier): flip the family
    /// back and record it (ADR-014 item 3 — the probe's success *is* the
    /// transition).
    pub(crate) fn plan_probe_succeeded(
        &self,
        request_id: &str,
        policy: &PlanPolicyCfg,
        session: Option<&str>,
    ) {
        let tier = plan_tier(&self.config, policy);
        let from = policy.overflow.clone();
        let to = tier
            .first()
            .cloned()
            .unwrap_or_else(|| policy.primary.clone());
        let to_account = account_of_route(&tier, &to).as_str();
        self.record_plan_switch(request_id, policy, &from, &to, to_account, true, session);
    }

    /// A demotion is state, not a local variable (ADR-011 item 4): the
    /// `provider_cooldown` projection, provider-wide, TTL from the
    /// provider's own clock or the declared default, anchored on the
    /// classification event.
    pub(crate) fn apply_demotion(
        &self,
        provider: &str,
        cls: &Classification,
        anchor: Option<vadis_core::EventId>,
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
            last_event: anchor.unwrap_or(vadis_core::EventId(0)),
        });
    }

    /// `failover.triggered` (FULL), before the next intent (§12.10.5 row 8).
    /// Shared by both forwarding paths.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_failover(
        &self,
        request_id: &str,
        attempt_index: u32,
        from: &RouteSpec,
        to: &RouteSpec,
        cls: &Classification,
        session: Option<&str>,
        reprefill: Option<u64>,
        switch_cost_nano: Option<u64>,
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
                // ADR-011 item 9: reprefill_tokens is the session's prefix
                // token total from the ledger (an `inferred` figure,
                // GAP-Q14) and switch_cost_nano prices it at the new
                // route's input_miss — both null when no ledger exists.
                "reprefill_tokens": reprefill,
                "switch_cost_nano": switch_cost_nano,
            }),
            session,
        );
    }

    /// The session's prefix token total from the `cache_ledger` projection
    /// (ADR-011 item 9's `reprefill_tokens` source — `inferred`, GAP-Q14),
    /// priced at `to`'s input_miss for `switch_cost_nano`.
    pub(crate) fn failover_cost(
        &self,
        session: Option<&str>,
        to: &RouteSpec,
    ) -> (Option<u64>, Option<u64>) {
        let Some(session) = session else {
            return (None, None);
        };
        let Some(store) = &self.store else {
            return (None, None);
        };
        let Ok(QueryRow::CacheLedger(blocks)) = store.query(Query::CacheLedgerBlocks {
            session_key: session,
        }) else {
            return (None, None);
        };
        if blocks.is_empty() {
            return (None, None);
        }
        let tokens: u64 = blocks.iter().map(|b| b.tokens).sum();
        let cost_nano = crate::accounting::route_accounting(&self.config, to).map(|acc| {
            // Pre-response (spec §4.10 rule 8): no measured `n` exists
            // yet, so the FIRST band's prices price the estimate — a
            // choice that invents no estimate — and the figure keeps its
            // `inferred` label. tokens × price(USD/1K) → Nano, floored
            // (§12.4 discipline).
            let first = &acc.prices[0].table;
            let v = tokens as u128 * first.input_miss.0 as u128 / 1000;
            if v > u64::MAX as u128 {
                u64::MAX
            } else {
                v as u64
            }
        });
        (Some(tokens), cost_nano)
    }

    fn exhausted_failure(&self, upstream_status: Option<u16>, class: ErrorClass) -> ForwardFailure {
        let deterministic = matches!(
            class,
            ErrorClass::FormatError | ErrorClass::ContentPolicyBlocked
        );
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

/// The third `plan_switch` reason word (spec §6's producer table /
/// `PlanSwitchRec.reason`'s vocabulary): ADR-011's cooldown projection
/// refused the family's primary before any attempt. Local to the proxy
/// because only the two forwarding walks can produce it; the sibling
/// words live in `vadis_core::plan` beside `plan.switched`'s writer.
pub(crate) const REASON_PRIMARY_COOLING_DOWN: &str = "primary_cooling_down";

/// The first route the request failed over from (spec §6
/// `result.failover_from`): set once, never overwritten by later hops.
fn failover_origin(slot: &mut Option<RouteSpec>, failed: &RouteSpec) {
    if slot.is_none() {
        *slot = Some(failed.clone());
    }
}

/// The candidate walk's skip reasons (ADR-022 / spec §8's frozen
/// `no_available_route` shape): one word per eligibility condition that
/// can refuse a candidate **before** any attempt. Frozen vocabulary —
/// a client parses these.
pub(crate) const SKIP_UNKNOWN_PROVIDER: &str = "unknown_provider";
pub(crate) const SKIP_KEYLESS: &str = "keyless";
pub(crate) const SKIP_WIRE_MISMATCH: &str = "wire_mismatch";
pub(crate) const SKIP_DEMOTED: &str = "demoted";

/// One `skipped[]` entry: the route in its `<provider>/<model>` wire form
/// and the one-word reason. Shared by both forwarding paths so the two
/// media cannot disagree about an entry's shape (ADR-024 ruling 2).
pub(crate) fn skipped_entry_json((route, reason): &(RouteSpec, &'static str)) -> Value {
    json!({
        "route": route.to_string(),
        "reason": reason,
    })
}

/// The `skipped[]` member of the frozen `no_available_route` refusal
/// (spec §8): one entry per candidate the walk refused **without
/// attempting**, in the chain's own order. Shared by both forwarding
/// paths so the two media cannot disagree about the walk's reasons.
pub(crate) fn skipped_json(skipped: &[(RouteSpec, &'static str)]) -> Vec<Value> {
    skipped.iter().map(skipped_entry_json).collect()
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
        WireApi::Chat => vadis_protocol::usage_from_chat(&v),
        WireApi::Responses => vadis_protocol::usage_from_responses(&v),
        WireApi::Anthropic => vadis_protocol::usage_from_anthropic(&v),
    }
}

#[cfg(test)]
mod walk_tests {
    //! The buffered candidate walk's wire gate (ADR-022 / DESIGN §12.10.9):
    //! a candidate whose provider's `wire_api` differs from the inbound
    //! protocol is skipped in the keyless class — never attempted, never a
    //! displacement — and the walk-end refusal reports it as `wire_mismatch`.
    //! The mock-upstream proof of the same relations is CONF-57.

    use super::*;
    use std::collections::BTreeMap;

    /// A fake transport that records every request it is handed and answers
    /// a fixed 200 chat completion.
    struct RecordingTransport {
        seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl ProviderSend for RecordingTransport {
        fn send(
            &self,
            _req: HttpRequest<Bytes>,
        ) -> impl Future<Output = vadis_providers::AttemptOutcome> + Send {
            self.seen.lock().unwrap().push("attempt".to_string());
            async {
                vadis_providers::AttemptOutcome::Responded(
                    vadis_providers::UpstreamResponse {
                        status: 200,
                        retry_after: None,
                        content_type: Some("application/json".into()),
                        body: Bytes::from_static(
                            br#"{"id":"ok","choices":[{"index":0,"message":{"role":"assistant","content":"served"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}"#,
                        ),
                    },
                )
            }
        }
    }

    fn provider(name: &str, wire: WireApi) -> ProviderCfg {
        // A minimal legal entry: validate() is not run here, but the shape
        // matches what load() produces for a keyed provider.
        serde_json::from_value(serde_json::json!({
            "name": name,
            "urls": { wire.as_str(): format!("http://{name}.example/v1/{wire}") },
            "api_key_env": format!("UNIT_{name}_KEY"),
            "wire_api": wire.as_str(),
            "supports": [wire.as_str()],
            "models": [{
                "id": "m",
                "context": "128k",
                "price": {
                    "input_miss": 0.001, "input_hit": 0.0001,
                    "cache_write": 0.0, "output": 0.002,
                    "peak": { "multiplier": 1.0, "windows": [] }
                },
                "source": "unit fixture"
            }],
        }))
        .unwrap()
    }

    fn forwarder(
        config: VadisConfig,
        transports: HashMap<String, Arc<dyn ProviderTransport>>,
    ) -> Forwarder {
        let mut api_keys = HashMap::new();
        for name in transports.keys() {
            api_keys.insert(name.clone(), vec!["sk-unit".to_string()]);
        }
        Forwarder {
            config,
            transports,
            api_keys,
            store: None,
            trace: None,
            transform_engine: None,
            response_cache: None,
            session_ttl_us: 0,
        }
    }

    async fn forward(forwarder: &Forwarder, model: &str) -> (u16, Option<Value>) {
        let body = format!(
            r#"{{"model":"{model}","messages":[{{"role":"user","content":"x"}}],"stream":false}}"#
        );
        match forwarder
            .forward(
                WireApi::Chat,
                body.as_bytes(),
                "req-unit",
                &[],
                TransformMode::Passthrough,
            )
            .await
        {
            ForwardOutcome::Success(s) => (s.status, None),
            ForwardOutcome::Failure(f) => (f.status, f.details),
        }
    }

    /// The gate's positive: a candidate whose wire matches the inbound
    /// protocol is still served after wire-incompatible candidates were
    /// skipped — the foreign provider's transport is never handed a
    /// request.
    #[tokio::test]
    async fn wire_matching_candidate_still_serves_after_the_gate_skips_foreign_ones() {
        let mut transports: HashMap<String, Arc<dyn ProviderTransport>> = HashMap::new();
        let foreign_seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let native_seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        transports.insert(
            "foreign".into(),
            Arc::new(RecordingTransport {
                seen: foreign_seen.clone(),
            }),
        );
        transports.insert(
            "native".into(),
            Arc::new(RecordingTransport {
                seen: native_seen.clone(),
            }),
        );
        let config = VadisConfig {
            server: server_cfg(),
            session: session_cfg(),
            cache: cache_cfg(),
            trace: trace_cfg(),
            providers: vec![
                provider("keyless", WireApi::Chat),
                provider("foreign", WireApi::Responses),
                provider("native", WireApi::Chat),
            ],
            aliases: BTreeMap::new(),
            plugins: Vec::new(),
            fallback: vec![
                RouteSpec {
                    provider: "foreign".into(),
                    model: "m".into(),
                },
                RouteSpec {
                    provider: "native".into(),
                    model: "m".into(),
                },
            ],
            plan_policy: None,
            plan_policies: None,
            state: None,
        };
        let f = forwarder(config, transports);

        let (status, _details) = forward(&f, "keyless/m").await;

        assert_eq!(
            status, 200,
            "the wire-matching candidate serves the request"
        );
        assert_eq!(
            foreign_seen.lock().unwrap().len(),
            0,
            "the responses-wire transport was never handed the chat request"
        );
        assert_eq!(native_seen.lock().unwrap().len(), 1);
    }

    /// The gate's negative: with no wire-matching candidate in the chain,
    /// the walk ends in the frozen refusal and the wire-incompatible
    /// entry is reported as `wire_mismatch`, never attempted.
    #[tokio::test]
    async fn wire_mismatch_candidate_never_serves_and_reports_the_reason() {
        let mut transports: HashMap<String, Arc<dyn ProviderTransport>> = HashMap::new();
        let foreign_seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        transports.insert(
            "foreign".into(),
            Arc::new(RecordingTransport {
                seen: foreign_seen.clone(),
            }),
        );
        let config = VadisConfig {
            server: server_cfg(),
            session: session_cfg(),
            cache: cache_cfg(),
            trace: trace_cfg(),
            providers: vec![
                provider("keyless", WireApi::Chat),
                provider("foreign", WireApi::Responses),
            ],
            fallback: vec![RouteSpec {
                provider: "foreign".into(),
                model: "m".into(),
            }],
            aliases: BTreeMap::new(),
            plugins: Vec::new(),
            plan_policy: None,
            plan_policies: None,
            state: None,
        };
        let f = forwarder(config, transports);

        let (status, details) = forward(&f, "keyless/m").await;

        assert_eq!(status, 502, "the walk exhausted: nothing may serve");
        let details = details.expect("the refusal carries details");
        assert_eq!(details["stage"], "no_available_route");
        let skipped = details["skipped"].as_array().expect("skipped[]");
        assert_eq!(skipped.len(), 2, "the resolved route and the one fallback");
        assert_eq!(skipped[0]["route"], "keyless/m");
        assert_eq!(skipped[0]["reason"], "keyless");
        assert_eq!(skipped[1]["route"], "foreign/m");
        assert_eq!(skipped[1]["reason"], "wire_mismatch");
        assert_eq!(
            foreign_seen.lock().unwrap().len(),
            0,
            "the foreign transport was never handed a request"
        );
    }

    // -- minimal legal config sections for the unit fixtures ------------

    fn server_cfg() -> vadis_core::config::ServerCfg {
        serde_json::from_value(serde_json::json!({
            "addr": "127.0.0.1:0",
            "upstream_attempt_timeout": "10s",
            "request_timeout": "30s",
        }))
        .unwrap()
    }

    fn session_cfg() -> vadis_core::config::SessionCfg {
        serde_json::from_value(serde_json::json!({
            "key_sources": ["prompt_cache_key"],
            "ttl": "11h",
        }))
        .unwrap()
    }

    fn cache_cfg() -> vadis_core::config::CacheCfg {
        serde_json::from_value(serde_json::json!({
            "sticky": true,
            "breakeven": { "enabled": true, "min_remaining_turns": 2, "safety_factor": 1.1 },
        }))
        .unwrap()
    }

    fn trace_cfg() -> vadis_core::config::TraceCfg {
        serde_json::from_value(serde_json::json!({
            "dir": "./state/traces",
            "rollover": "hourly",
        }))
        .unwrap()
    }
}
