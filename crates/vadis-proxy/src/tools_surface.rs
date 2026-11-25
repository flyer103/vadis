//! Spec §4.23: the tools surface — `POST /v1/tools/{search,fetch,tts,asr}`.
//!
//! The four verbs answer on a **provider's own tool endpoints**, not on a
//! model route: a search returns results, a fetch returns a page, `tts`
//! returns audio bytes and `asr` returns a transcript. vadis therefore does
//! not synthesize or parse the body the way §4.22's media surface does — it
//! relays the client's request **byte-for-byte** (multipart framing
//! included), injects the provider credential, and hands the upstream's own
//! status, content type and bytes back unchanged. That is the whole
//! contract: the surface adds one thing the caller cannot add itself (the
//! key) and records one thing the caller cannot record (the call's §6 row).
//!
//! Everything here is a function of the revision's config: the mount is
//! built once at publish ([`ToolsMount::from_config`]), the URL per verb is
//! the one written on the provider entry (never composed from a base), and
//! the credential rotates over the entry's pool exactly like the chat path's
//! (ADR-049). The HTTP wiring — the guard layer, the body bound, the 501
//! arms — belongs to `vadis-cli`, as with `models_read` and `media_tools`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use vadis_core::config::{ToolKind, ToolsCfg};
use vadis_core::error::ErrorCode;
use vadis_core::trace::{
    CostRec, DecisionRec, DecisionRecord, IdentityRec, PrefixRec, ProtocolRec, ResultRec, StateRec,
    ToolsRec, TraceError, TRACE_SCHEMA_VERSION,
};
use vadis_core::transform::TransformMode;
use vadis_core::{Currency, Nano, Usage};

use crate::forward::ProviderTransport;

/// The revision's tools mount (spec §4.23): either the surface is served —
/// the config named a provider, that provider declares at least one verb URL
/// and holds a credential — or it is not, and this value carries the
/// sentence that says which fact is missing. A refusal is always a `501`
/// with the reason in it, never a bare `404`: the route exists, the
/// deployment does not serve it, and the difference is exactly what an
/// operator needs from the error.
pub enum ToolsMount {
    Unserved(String),
    Served(Arc<ToolSurface>),
}

impl ToolsMount {
    /// The mount for one revision. `keys` and `transport` are the provider
    /// entry's own resolved credential material (`p.key_pool()`'s values,
    /// read at publish) and the transport built for its attempt timeout;
    /// an empty pool or no transport means the entry cannot be dialled, which
    /// is the keyless arm — the surface answers `501`, never a request with
    /// no credential on it.
    pub fn from_config(
        cfg: &ToolsCfg,
        provider: &vadis_core::config::ProviderCfg,
        keys: Vec<String>,
        transport: Option<Arc<dyn ProviderTransport>>,
    ) -> Self {
        let declared: BTreeMap<ToolKind, String> = ToolKind::ALL
            .into_iter()
            .filter_map(|verb| {
                provider
                    .tools
                    .as_ref()
                    .and_then(|t| t.url(verb))
                    .map(|url| (verb, url.trim().to_string()))
            })
            .collect();
        if declared.is_empty() {
            return Self::Unserved(format!(
                "provider '{}' declares no tools: URL, so /v1/tools/* has nothing to relay \
                 (spec §4.23)",
                cfg.provider
            ));
        }
        let Some(transport) = transport.filter(|_| !keys.is_empty()) else {
            return Self::Unserved(format!(
                "provider '{}' holds no credential, so the tools surface cannot dial it \
                 (spec §4.23)",
                cfg.provider
            ));
        };
        Self::Served(Arc::new(ToolSurface {
            provider: cfg.provider.clone(),
            max_body_bytes: cfg.max_body_bytes,
            max_response_bytes: cfg.max_response_bytes,
            urls: declared,
            keys,
            transport,
            cursor: AtomicUsize::new(0),
        }))
    }

    /// The sentence to answer with when the surface is not served — `None`
    /// when it is (the caller then holds the surface).
    pub fn unserved(&self) -> Option<&str> {
        match self {
            Self::Unserved(message) => Some(message.as_str()),
            Self::Served(_) => None,
        }
    }
}

/// One revision's tools surface: the provider the four verbs are dialled
/// through, the URL written for each declared verb, the credential pool, and
/// the two bounds. Immutable after the publish; the only shared state is the
/// round-robin cursor over the pool.
pub struct ToolSurface {
    provider: String,
    max_body_bytes: i64,
    max_response_bytes: i64,
    urls: BTreeMap<ToolKind, String>,
    keys: Vec<String>,
    transport: Arc<dyn ProviderTransport>,
    cursor: AtomicUsize,
}

impl ToolSurface {
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// Spec §4.23 `tools.max_body_bytes`, the inbound bound.
    pub fn max_body_bytes(&self) -> usize {
        self.max_body_bytes.max(0) as usize
    }

    /// Spec §4.23 `tools.max_response_bytes`, the bound on what is relayed.
    pub fn max_response_bytes(&self) -> usize {
        self.max_response_bytes.max(0) as usize
    }

    /// The URL this deployment wrote for `verb`, or `None` — the per-verb
    /// `501` arm: a surface may declare one verb and not another, and the
    /// one not written is not served here.
    pub fn verb_url(&self, verb: ToolKind) -> Option<&str> {
        self.urls.get(&verb).map(String::as_str)
    }

    /// The outbound request for one tool call: the verb's URL verbatim, the
    /// next credential of the pool as `authorization: Bearer`, the client's
    /// content type **as received** (the multipart boundary lives in it — a
    /// rewritten value would make the body unparsable upstream), and the
    /// body byte-for-byte (AGENTS constraint 1: this is the client's own
    /// request; the only rewrite is the credential the caller cannot send).
    pub fn prepare(
        &self,
        verb: ToolKind,
        content_type: Option<&str>,
        body: Bytes,
    ) -> Result<http::Request<Bytes>, String> {
        let url = self.verb_url(verb).ok_or_else(|| {
            format!(
                "provider '{}' declares no tools.{} URL (spec §4.23)",
                self.provider,
                verb.as_str()
            )
        })?;
        let key = self
            .keys
            .get(self.cursor.fetch_add(1, Ordering::Relaxed) % self.keys.len())
            .ok_or_else(|| format!("provider '{}' holds no credential", self.provider))?;
        let mut send = http::Request::builder()
            .method(http::Method::POST)
            .uri(url)
            .header("authorization", format!("Bearer {key}"));
        if let Some(content_type) = content_type {
            send = send.header("content-type", content_type);
        }
        send.body(body).map_err(|e| {
            format!(
                "the tools.{} URL '{url}' is not a usable request target: {e}",
                verb.as_str()
            )
        })
    }

    /// One attempt on the verb's endpoint. The transport never retries and
    /// never fails over (ADR-011): a replay would be a second billed call.
    pub async fn send(&self, req: http::Request<Bytes>) -> vadis_providers::AttemptOutcome {
        self.transport.send_boxed(req).await
    }
}

/// The facts one tool call's record carries (spec §6 / §4.23). A struct
/// rather than ten positional arguments: every member is a field of the
/// record, and a reader of the call site can check them one by one.
pub struct ToolCallFacts<'a> {
    pub request_id: &'a str,
    pub verb: &'a str,
    pub provider: &'a str,
    /// The status the caller received (§8's envelope's own value).
    pub status: u16,
    /// The upstream's status when an answer arrived at all.
    pub upstream_status: Option<u16>,
    pub request_bytes: u64,
    pub response_bytes: u64,
    pub now_us: i64,
    pub overhead_ms: u32,
    pub upstream_ms: u32,
    pub config_digest: &'a str,
    pub client: &'static str,
    /// The failure to record in `errors[]`: our own refusal or transport
    /// failure, and the relayed upstream status when it was not 2xx — a
    /// relayed `4xx` is still a call that produced no answer, and a reader
    /// who only sees `result.status` should not have to guess why.
    pub error: Option<(ErrorCode, String)>,
}

/// One tool call, one §6 row. The tools group ([`ToolsRec`]) is what makes
/// this row distinguishable from a model call at a glance, and it carries
/// `priced: false` because v0.1 has no price table for the four verbs: the
/// record's zero cost is an **unpriced** call, and `usage_missing` is true
/// for the same reason — no chat envelope reports token usage for a search.
pub fn tool_record(facts: &ToolCallFacts<'_>) -> DecisionRecord {
    DecisionRecord {
        schema_version: TRACE_SCHEMA_VERSION,
        ts: crate::accounting::rfc3339_millis(facts.now_us),
        config_digest: facts.config_digest.to_string(),
        identity: IdentityRec {
            request_id: facts.request_id.to_string(),
            // The tools surface has no `request.received` row: vadis writes
            // no events for a tool call, so the pre-pipeline sentinel is the
            // honest value (the same `0` the boundary refusals carry).
            event_id: 0,
            client: facts.client,
            session: None,
            thread_id: None,
            turn_index: 0,
        },
        protocol: ProtocolRec {
            protocol_in: "tools".to_string(),
            // No wire protocol answered this call: the upstream is a tool
            // endpoint, and the tool group names the verb.
            protocol_out: None,
            translated: false,
            lossy: Vec::new(),
        },
        decision: DecisionRec {
            provider: facts.provider.to_string(),
            // No model id is resolved on this surface (spec §4.23): the
            // caller names a verb, the deployment names a provider, and the
            // body that reaches the upstream is the client's own.
            model: String::new(),
            requested_model: None,
            selection_source: "explicit".to_string(),
            plugin_chain: Vec::new(),
            decision_ms: 0,
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
        transform_mode: TransformMode::Passthrough,
        transforms: Vec::new(),
        cache: None,
        tools: Some(ToolsRec {
            verb: facts.verb.to_string(),
            provider: facts.provider.to_string(),
            upstream_status: facts.upstream_status,
            request_bytes: facts.request_bytes,
            response_bytes: facts.response_bytes,
            priced: false,
        }),
        usage: Usage::default(),
        usage_missing: true,
        cost: CostRec {
            input_miss: Nano(0),
            input_hit: Nano(0),
            cache_write: Nano(0),
            output: Nano(0),
            peak_applied_pct: 100,
            total: Nano(0),
            // No table priced this call; the tools group says so explicitly.
            currency: Currency::Usd,
            quota_after: None,
        },
        result: ResultRec {
            status: facts.status,
            upstream_status: facts.upstream_status,
            attempts: 1,
            failover_from: None,
            plan_switch: None,
            failover_order: None,
            overhead_ms: facts.overhead_ms,
            upstream_ms: Some(facts.upstream_ms),
        },
        errors: facts
            .error
            .as_ref()
            .map(|(code, message)| {
                vec![TraceError {
                    kind: TraceError::kind_for_code(*code).to_string(),
                    message: message.clone(),
                    plugin: None,
                    details: None,
                }]
            })
            .unwrap_or_default(),
    }
}
