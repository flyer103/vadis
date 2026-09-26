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

use crate::cost::{Currency, Nano};
use crate::error::ErrorCode;
use crate::quota::{OverQuota, QuotaPlan, QuotaState, QuotaVerdict};
use crate::transform::TransformMode;
use crate::Usage;

/// The trace schema version. It only increments on a **breaking** change;
/// adding an optional field does not (the autowork side tolerates unknown
/// fields, ADR-005). Version **2** (ADR-018) adds `cost.currency` and
/// `plan_switch.cost_currency` — not the optional-field exemption: the new
/// fields change how the existing money fields are **read**, and a consumer
/// that ignores them would sum CNY into USD. A v1 record is USD by
/// definition (no non-USD route was configurable when it was written), so
/// a window may hold both vintages unambiguously (DESIGN §12.6).
pub const TRACE_SCHEMA_VERSION: u16 = 2;

/// spec §6, one line per request.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DecisionRecord {
    pub schema_version: u16,
    /// RFC3339 UTC with millisecond precision, e.g. `2026-09-19T07:41:02.123Z`.
    pub ts: String,
    /// The identity of the **effective configuration** that priced this
    /// record (ADR-037 D6; spec §6, §4.14): the byte digest
    /// `sha16(root_sha16 + ":" + roster_sha16)`, each half the first 16 hex
    /// chars of SHA-256 over that file's own bytes, the roster half the
    /// empty string when the roster is inline. Additive — no existing
    /// field changes meaning, so `schema_version` stays 2; a record
    /// **without** the key predates the field and must never be read as an
    /// empty digest. Computed once by the loader, carried as an immutable
    /// value; nothing behind a request opens, reads or hashes a file.
    pub config_digest: String,
    pub identity: IdentityRec,
    pub protocol: ProtocolRec,
    pub decision: DecisionRec,
    pub state: StateRec,
    pub prefix: PrefixRec,
    /// The transform group (spec §6 / ADR-019): the mode in effect for this
    /// request's outbound body — **always present**, `passthrough` on every
    /// request that did not ask (and on every request refused before the
    /// transform chain ran), so an empty `transforms[]` is disambiguated
    /// between "no mode" and "mode on, nothing matched" — the reason the
    /// mode is a sibling of the array and never derived from it.
    pub transform_mode: TransformMode,
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
    /// What one step rewrote, and how much of it (spec §6 `edited_paths[]`,
    /// ADR-019): one entry per payload node the step changed — the audit
    /// surface of a content edit. A reviewer compares spans, not documents.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub edited_paths: Vec<EditedPath>,
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

/// One edited payload node of one transform step (spec §6 `edited_paths[]`,
/// DESIGN §12.6): the node address (`input[7].output` — never a byte offset,
/// the scan is what resolves it, per attempt) and the two byte counts, which
/// are the **payload text's** length before and after the step, not the
/// encoded span's (escaping belongs to the splicer and shows up in that
/// attempt's `body_hash`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct EditedPath {
    pub path: String,
    pub bytes_in: usize,
    pub bytes_out: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct CostRec {
    pub input_miss: Nano,
    pub input_hit: Nano,
    pub cache_write: Nano,
    pub output: Nano,
    pub peak_applied_pct: u32,
    pub total: Nano,
    /// spec §4.8: the unit every amount in this group is denominated in
    /// — the currency of the provider entry `decision.provider` names.
    /// Always written on a v2 record (a v1 record is USD by definition).
    pub currency: Currency,
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
    /// spec §6 `plan_switch` (ADR-014): present-and-null whenever the plan
    /// policy did not displace this request's account — the same stance
    /// as `prefix.continuity`, never omitted. It is **not** a second name
    /// for `failover_from`: a failure-class fact sets that field, the
    /// plan policy's account state sets this one (spec §6's own table).
    pub plan_switch: Option<PlanSwitchRec>,
    pub overhead_ms: u32,
    pub upstream_ms: Option<u32>,
}

/// spec §6 `result.plan_switch` (ADR-014 / DESIGN §12.10.8): the plan
/// policy moved this request's account. `from` / `to` are
/// `"<provider>/<model>"` strings, the same wire form `failover_from`
/// uses; `reason` is the stable word; `probe` says whether the move was
/// an admitted probe's return trip. The two figures are the switch's
/// cache price under spec §7's convention (`inferred` at decision time).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PlanSwitchRec {
    pub from: String,
    pub to: String,
    /// `primary_exhausted` / `primary_cooling_down` / `primary_recovered`.
    pub reason: &'static str,
    pub probe: bool,
    /// The session's prefix token count (the §6 block-token attribution,
    /// GAP-Q14) — an `inferred` figure; `None` when no session/ledger
    /// exists (serialized as null, spec §6).
    pub reprefill_tokens: Option<u64>,
    /// `reprefill_tokens × p_miss(destination account)`, integer Nano;
    /// an in-plan destination's marginal price is 0, so the return trip
    /// records the work and a money cost of 0.
    pub switch_cost_nano: Option<u64>,
    /// spec §4.8: the unit `switch_cost_nano` is denominated in — the
    /// **destination** route's currency, priced by the destination's
    /// table. Present whenever `plan_switch` is not null, even where
    /// `switch_cost_nano` is null (the unit is a fact about the
    /// destination, not about the figure). It deliberately does not
    /// borrow `cost.currency`: a family may span two currencies, and
    /// when the destination's attempt fails and the chain serves a route
    /// of another currency, the switch's price and the record's own cost
    /// are in different units.
    pub cost_currency: Currency,
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
            ErrorCode::Unauthorized => "unauthorized",
            // §4.13's bound is a boundary refusal with a kind of its
            // own, shared with §8's error.type table (the two move
            // together).
            ErrorCode::RequestTooLarge => "request_too_large",
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

    /// The identity of the configuration this writer's process loaded
    /// (ADR-037 D6; spec §6 `config_digest`): every record a process
    /// writes carries the one value, so the writer — the one sink every
    /// record passes through — is the constructors' one source for it,
    /// and a record and its write can never disagree about it.
    ///
    /// **Required, on purpose (R43-F4; spec §6's empty-string bullet;
    /// DESIGN §12.6's writer seam): there is no default to fall into, so
    /// a writer cannot *forget* the identity — forgetting is a compile
    /// error, never a silent `""`.** A writer with no configuration
    /// behind it (tests, tools — `NullTraceWriter`; the store's bare
    /// sink) states the empty string *explicitly*, and its records never
    /// land in a served trace: the empty string means "the configuration
    /// is not recorded for this request", never an identity. The one
    /// writer wired into the serving path (`router-cli`'s
    /// `ConfigTraceWriter`) carries the loader-computed digest — never
    /// empty, spec §4.14's recipe hashes the root's half always — and
    /// refuses construction with an empty one.
    fn config_digest(&self) -> &str;
}

/// The no-op writer for tests and tools that run without a trace dir.
pub struct NullTraceWriter;

impl TraceWriter for NullTraceWriter {
    fn write(&self, _: &DecisionRecord) -> Result<Option<String>, String> {
        Ok(None)
    }

    /// The explicit empty (R43-F4): there is no configuration behind
    /// this writer and its records never land in a served trace, so the
    /// empty string is *stated here* — never defaulted into.
    fn config_digest(&self) -> &str {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R43-F4: the empty digest is **explicit and confined to unserved
    /// writers**. The trait method is required — a writer that forgets
    /// the identity does not compile (there is no default to fall into;
    /// the RED probe at the round's base showed the forgotten override
    /// compiling and silently returning `""`). What remains expressible
    /// is the *stated* empty, and it is confined to writers with no
    /// configuration behind them, whose records never land in a served
    /// trace (spec §6's empty-string bullet).
    #[test]
    fn the_empty_digest_is_explicit_and_confined_to_unserved_writers() {
        assert_eq!(
            NullTraceWriter.config_digest(),
            "",
            "the sanctioned exception: no configuration behind this writer, stated explicitly"
        );
        // The compile-time half is the trait's shape itself: the probe
        // writer at the fix's base (`struct ForgottenWriter; impl
        // TraceWriter for ForgottenWriter { write only }`) compiled and
        // returned "" — against this seam it does not compile.
    }

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
        // §4.7's guard: the boundary refusal has a kind of its own.
        assert_eq!(
            TraceError::kind_for_code(ErrorCode::Unauthorized),
            "unauthorized"
        );
    }

    #[test]
    fn serializes_errors_as_always_present_array() {
        let rec = DecisionRecord {
            schema_version: TRACE_SCHEMA_VERSION,
            ts: "2026-09-19T07:41:02.123Z".into(),
            config_digest: "0123456789abcdef".into(),
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
            transform_mode: TransformMode::Passthrough,
            transforms: Vec::new(),
            usage: Usage::default(),
            usage_missing: false,
            cost: CostRec {
                input_miss: Nano(1),
                input_hit: Nano(2),
                cache_write: Nano(0),
                output: Nano(3),
                peak_applied_pct: 100,
                total: Nano(6),
                currency: Currency::Usd,
                quota_after: None,
            },
            result: ResultRec {
                status: 200,
                upstream_status: Some(200),
                failover_from: None,
                plan_switch: None,
                overhead_ms: 1,
                upstream_ms: Some(2),
            },
            errors: Vec::new(),
        };
        let v = serde_json::to_value(&rec).unwrap();
        // spec §6: no failure = an EMPTY array, not an omitted field.
        assert!(v.get("errors").is_some(), "errors must serialize");
        assert_eq!(v["errors"].as_array().map(Vec::len), Some(0));
        assert_eq!(v["schema_version"], 2); // v2 carries cost.currency (ADR-018)
        assert_eq!(
            v["cost"]["currency"], "USD",
            "the cost group states its unit (spec 4.8)"
        );
        assert_eq!(v["identity"]["event_id"], 7);
        assert_eq!(v["prefix"]["continuity"], 1.0);
    }
}
