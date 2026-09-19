//! The trace contract (spec §6, DESIGN §12.6): one `DecisionRecord` per
//! request, append-only JSONL, rolled hourly to
//! `<trace.dir>/YYYY-MM-DDTHH.jsonl` (UTC). The record is the **analysis
//! truth** and the only product → autowork channel (ADR-005); the state
//! truth is the event log, and the two are paired on
//! `request_id` + `event_id` — never on a timestamp.
//!
//! Wire shape: serde `snake_case` with the spec §6 field names verbatim;
//! `errors` is always present (an empty array when nothing failed, spec
//! §6 "no failure = empty array, do not omit").

use serde::Serialize;

use crate::cost::NanoUsd;
use crate::error::ErrorCode;
use crate::quota::{OverQuota, QuotaPlan, QuotaState, QuotaVerdict};
use crate::Usage;

/// The trace schema version. It only increments on a **breaking** change;
/// adding an optional field does not (the autowork side tolerates unknown
/// fields, ADR-005).
pub const TRACE_SCHEMA_VERSION: u16 = 1;

/// spec §6, one line per request.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DecisionRecord {
    pub schema_version: u16,
    /// RFC3339 UTC with millisecond precision, e.g. `2026-09-19T07:41:02.123Z`.
    pub ts: String,
    pub identity: IdentityRec,
    pub protocol: ProtocolRec,
    pub decision: DecisionRec,
    pub state: StateRec,
    pub prefix: PrefixRec,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub transforms: Vec<TransformRecord>,
    pub usage: Usage,
    pub usage_missing: bool,
    pub cost: CostRec,
    pub result: ResultRec,
    pub errors: Vec<TraceError>,
}

/// identity: `request_id` + `event_id` are the join key into `events`
/// (spec §4.5); `event_id` is the request's `request.received` row.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct IdentityRec {
    pub request_id: String,
    /// The `request.received` event row id (CONF-24's anchor).
    pub event_id: i64,
    /// UA-normalized client kind: `codex` / `hermes` / `claude-code` /
    /// `other` (DESIGN §12.2). Never the raw UA string.
    pub client: &'static str,
    pub session: Option<String>,
    pub thread_id: Option<String>,
    pub turn_index: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ProtocolRec {
    pub protocol_in: String,
    pub protocol_out: Option<String>,
    pub translated: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lossy: Vec<LossyNote>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct LossyNote {
    pub field: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DecisionRec {
    pub provider: String,
    pub model: String,
    /// The client's own `model` string, verbatim; `None` when the request
    /// carried none — present-and-null, never omitted (the same stance as
    /// `prefix.continuity`; DESIGN §12.6, spec §6).
    pub requested_model: Option<String>,
    /// `explicit` / `alias` / `plugin`.
    pub selection_source: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub plugin_chain: Vec<String>,
    /// Decision time in milliseconds (selector + guards).
    pub decision_ms: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct StateRec {
    pub stateful_inbound: bool,
    pub sticky_hit: bool,
    pub cache_control_breaks: u16,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrefixRec {
    pub blocks: Vec<PrefixBlockRec>,
    /// Longest common block ratio vs the previous request of the session;
    /// `None` (serialized as null) when there is no previous request — an
    /// absent measurement is absent, not 1.0 and not 0.0 (§12.10.6).
    pub continuity: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PrefixBlockRec {
    /// `system` / `message` / `tool` / `input_item`.
    pub kind: &'static str,
    pub index: u32,
    /// Proportional attribution of the measured `usage.input_total`
    /// (GAP-Q14) — an `inferred` figure.
    pub tokens: u64,
    pub hash: String,
}

/// One transform step's accounting (spec §6 "transform"). This round's
/// chain is empty; the verdict vocabulary lands with the transform chain
/// and is asserted by CONF-17.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct TransformRecord {
    pub plugin: String,
    pub added_input_tokens: i64,
    pub saved_input_tokens: i64,
    pub saved_output_tokens: i64,
    /// `neutral` / `risky` / `broken`.
    pub cache_impact: &'static str,
    /// `verified` / `inferred` (spec §7; only `verified` may enter a gate).
    pub verdict: &'static str,
    pub tee_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct CostRec {
    pub input_miss: NanoUsd,
    pub input_hit: NanoUsd,
    pub cache_write: NanoUsd,
    pub output: NanoUsd,
    pub peak_applied_pct: u32,
    pub total: NanoUsd,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_after: Option<QuotaAfter>,
}

/// The post-charge state of the plan that was charged (spec §6
/// `cost.quota_after`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct QuotaAfter {
    pub provider: String,
    pub plan_idx: usize,
    pub tokens_used: u64,
    pub tokens_limit: u64,
    pub over_quota: String,
    /// `inside` / `spill` / `blocked` — the pure `charge` verdict
    /// (router-core), one stable word per shape.
    pub verdict: String,
}

impl QuotaAfter {
    /// From the pure quota model's own verdict (router-core `charge`) so
    /// the trace word and the accounting decision cannot drift.
    pub fn from_verdict(
        provider: &str,
        plan: &QuotaPlan,
        st: &QuotaState,
        v: &QuotaVerdict,
    ) -> Self {
        Self {
            provider: provider.to_string(),
            plan_idx: st.plan_idx,
            tokens_used: st.tokens_used,
            tokens_limit: plan.tokens,
            over_quota: match plan.over_quota {
                OverQuota::Block => "block",
                OverQuota::Spill => "spill",
            }
            .to_string(),
            verdict: match v {
                QuotaVerdict::Inside { .. } => "inside",
                QuotaVerdict::Spill { .. } => "spill",
                QuotaVerdict::Blocked { .. } => "blocked",
            }
            .to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ResultRec {
    pub status: u16,
    pub upstream_status: Option<u16>,
    /// `"<provider>/<model>"` when the request switched away from a failed
    /// route (spec §6 `failover_from`).
    pub failover_from: Option<String>,
    pub overhead_ms: u32,
    pub upstream_ms: Option<u32>,
}

/// spec §6 "failure details": the internal failure record — several
/// possible, distinct from the single §8 client-facing error body (the two
/// share the kind vocabulary). Always an array on the wire; never omitted.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct TraceError {
    /// `transform_error` / `upstream_error` / `trace_write_failed` /
    /// `internal` — serialized from the client-facing `ErrorCode`'s own
    /// vocabulary mapping (spec §8 table).
    pub kind: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl TraceError {
    /// The trace-side kind for a client-facing `ErrorCode` (spec §8 table:
    /// every upstream-shaped failure is `upstream_error` in `errors[]`).
    pub fn kind_for_code(code: ErrorCode) -> &'static str {
        match code {
            ErrorCode::UpstreamError | ErrorCode::UpstreamTimeout | ErrorCode::QuotaExceeded => {
                "upstream_error"
            }
            ErrorCode::NotImplemented => "internal",
            ErrorCode::InvalidRequest
            | ErrorCode::AutoNotSupported
            | ErrorCode::CapabilityUnsupported
            | ErrorCode::StatefulUnsupported
            | ErrorCode::UnknownProvider
            | ErrorCode::UnknownModel => "transform_error",
            ErrorCode::CostCapExceeded => "upstream_error",
            ErrorCode::Internal => "internal",
        }
    }

    /// The `trace_write_failed` entry (spec §8: a trace write failure does
    /// not block the request; the missing observation must be explicit).
    pub fn trace_write_failed(reason: &str) -> Self {
        Self {
            kind: "trace_write_failed".to_string(),
            message: reason.to_string(),
            plugin: None,
            details: None,
        }
    }
}

/// `verified_savings_tokens` (spec §6 metric definitions): counts only the
/// transform gains whose `verdict` is `verified` — the gate-side filter
/// that makes "only `verified` may enter a gate" (spec §7) a function, not
/// a convention. Everything else an `inferred` row reports is deliberately
/// invisible here.
pub fn verified_savings_tokens(rec: &DecisionRecord) -> i64 {
    rec.transforms
        .iter()
        .filter(|t| t.verdict == "verified")
        .map(|t| t.saved_input_tokens.saturating_add(t.saved_output_tokens))
        .sum()
}

/// The trace-writing seam the data plane accounts through (the I/O-free
/// core's side of DESIGN §12.6): append one record, get back the durable
/// `"file:line"` pointer the accounting rows carry as `trace_ref`
/// (§12.10.5 note R2), or `None` when the write failed — the request is
/// unaffected and the caller records `errors[].kind = trace_write_failed`
/// (spec §8).
pub trait TraceWriter: Send + Sync {
    fn write(&self, rec: &DecisionRecord) -> Result<Option<String>, String>;
}

/// The no-op writer for tests and tools that run without a trace dir.
pub struct NullTraceWriter;

impl TraceWriter for NullTraceWriter {
    fn write(&self, _: &DecisionRecord) -> Result<Option<String>, String> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_for_covers_the_spec8_table() {
        assert_eq!(
            TraceError::kind_for_code(ErrorCode::UpstreamError),
            "upstream_error"
        );
        assert_eq!(
            TraceError::kind_for_code(ErrorCode::UpstreamTimeout),
            "upstream_error"
        );
        assert_eq!(TraceError::kind_for_code(ErrorCode::Internal), "internal");
        assert_eq!(
            TraceError::kind_for_code(ErrorCode::InvalidRequest),
            "transform_error"
        );
    }

    #[test]
    fn serializes_errors_as_always_present_array() {
        let rec = DecisionRecord {
            schema_version: TRACE_SCHEMA_VERSION,
            ts: "2026-09-19T07:41:02.123Z".into(),
            identity: IdentityRec {
                request_id: "req-1".into(),
                event_id: 7,
                client: "other",
                session: Some("sess-1".into()),
                thread_id: None,
                turn_index: 1,
            },
            protocol: ProtocolRec {
                protocol_in: "chat".into(),
                protocol_out: Some("chat".into()),
                translated: false,
                lossy: Vec::new(),
            },
            decision: DecisionRec {
                provider: "zai".into(),
                model: "glm-5.3".into(),
                requested_model: Some("zai/glm-5.3".into()),
                selection_source: "explicit".into(),
                plugin_chain: Vec::new(),
                decision_ms: 0,
            },
            state: StateRec {
                stateful_inbound: false,
                sticky_hit: false,
                cache_control_breaks: 0,
            },
            prefix: PrefixRec {
                blocks: vec![PrefixBlockRec {
                    kind: "message",
                    index: 0,
                    tokens: 100,
                    hash: "0123456789abcdef".into(),
                }],
                continuity: Some(1.0),
            },
            transforms: Vec::new(),
            usage: Usage::default(),
            usage_missing: false,
            cost: CostRec {
                input_miss: NanoUsd(1),
                input_hit: NanoUsd(2),
                cache_write: NanoUsd(0),
                output: NanoUsd(3),
                peak_applied_pct: 100,
                total: NanoUsd(6),
                quota_after: None,
            },
            result: ResultRec {
                status: 200,
                upstream_status: Some(200),
                failover_from: None,
                overhead_ms: 1,
                upstream_ms: Some(2),
            },
            errors: Vec::new(),
        };
        let v = serde_json::to_value(&rec).unwrap();
        // spec §6: no failure = an EMPTY array, not an omitted field.
        assert!(v.get("errors").is_some(), "errors must serialize");
        assert_eq!(v["errors"].as_array().map(Vec::len), Some(0));
        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["identity"]["event_id"], 7);
        assert_eq!(v["prefix"]["continuity"], 1.0);
    }
}
