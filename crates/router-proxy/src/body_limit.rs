//! The inbound request-body limit (spec §4.13, DESIGN §12.15 — the
//! landing). One boundary check above the pipeline: a declared
//! `Content-Length` above the configured bound is refused **without
//! reading a byte of the body**; an undeclared (chunked) body is read
//! bounded and refused the moment the bound is passed. Both arms answer
//! with the same complete, non-SSE `413 request_too_large` in §8's
//! unified body, leave exactly one §6 pre-pipeline trace record, and
//! close the connection (the body may not have been drained, and a
//! keep-alive connection holding unread request bytes would parse them
//! as the next request).
//!
//! Pure: no HTTP types, no I/O — the same shape `auth.rs` established,
//! so the verdict unit-tests without a rig. The wiring (route layer,
//! the bounded read, the 413 response, `DefaultBodyLimit::disable()`)
//! is router-cli's (§12.15: "one middleware … runs inside the token
//! guard and above the transform-mode resolution and the path split").

use router_core::config::WireApi;
use router_core::cost::Nano;
use router_core::error::ErrorCode;
use router_core::trace::{
    CostRec, DecisionRec, DecisionRecord, IdentityRec, PrefixRec, ProtocolRec, ResultRec, StateRec,
    TraceError, TRACE_SCHEMA_VERSION,
};
use router_core::transform::TransformMode;

/// The header-first check: does a declared `Content-Length` already
/// refuse this request? `content_length` is the parsed value of the
/// header the request actually carried. `Ok(())` means the read may
/// proceed (bounded); the refusal carries the declared length for
/// `details.content_length`.
pub fn check_declared(limit: usize, content_length: Option<u64>) -> Result<(), u64> {
    match content_length {
        Some(len) if len > limit as u64 => Err(len),
        _ => Ok(()),
    }
}

/// The frozen message (§12.11's discipline: no implementer invents
/// one): names the bound and the declared length when there was one,
/// never any byte of the body.
pub fn too_large_message(limit: usize, content_length: Option<u64>) -> String {
    match content_length {
        Some(len) => format!(
            "request body exceeds server.max_body_bytes: {len} bytes declared, \
             the configured bound is {limit}"
        ),
        None => format!(
            "request body exceeds server.max_body_bytes: the length was not declared \
             and the bounded read passed the configured bound of {limit}"
        ),
    }
}

/// The refused request's record (spec §6's pre-pipeline class, field by
/// field — the same class the auth guard's `refused_record` and the
/// mode refusal's `mode_refused_record` write: decided at the boundary,
/// nothing parsed, planned or attempted, `event_id: 0`, `usage_missing:
/// true`, nothing priced, no store row).
pub fn too_large_record(
    request_id: &str,
    proto_in: WireApi,
    limit: usize,
    content_length: Option<u64>,
    now_epoch_s: u64,
    overhead_ms: u32,
    config_digest: &str,
) -> DecisionRecord {
    DecisionRecord {
        schema_version: TRACE_SCHEMA_VERSION,
        ts: crate::accounting::rfc3339_millis(now_epoch_s),
        config_digest: config_digest.to_string(),
        identity: IdentityRec {
            request_id: request_id.to_string(),
            // The bound runs before §12.10.5's row 1: the same `0`
            // sentinel every pre-pipeline refusal writes.
            event_id: 0,
            client: "other",
            session: None,
            thread_id: None,
            turn_index: 0,
        },
        protocol: ProtocolRec {
            protocol_in: proto_in.as_str().to_string(),
            // No route was selected; no upstream was contacted.
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
        // The chain never ran — the bound answers before the mode header
        // is adjudicated (spec §4.13's own ordering rule): the mode word
        // is `passthrough` and nothing is claimed.
        transform_mode: TransformMode::Passthrough,
        transforms: Vec::new(),
        usage: router_core::Usage::default(),
        usage_missing: true,
        cost: CostRec {
            input_miss: Nano(0),
            input_hit: Nano(0),
            cache_write: Nano(0),
            output: Nano(0),
            peak_applied_pct: 100,
            total: Nano(0),
            // The pre-pipeline refusal default (the same stance as the
            // auth guard's): no table priced this record.
            currency: router_core::Currency::Usd,
            quota_after: None,
        },
        result: ResultRec {
            status: ErrorCode::RequestTooLarge.http_status(),
            upstream_status: None,
            failover_from: None,
            plan_switch: None,
            overhead_ms,
            upstream_ms: None,
        },
        errors: vec![TraceError {
            kind: TraceError::kind_for_code(ErrorCode::RequestTooLarge).to_string(),
            message: too_large_message(limit, content_length),
            plugin: None,
            details: Some(serde_json::json!({
                "limit_bytes": limit,
                "content_length": content_length,
            })),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_length_above_the_bound_refuses_without_a_read() {
        assert_eq!(check_declared(2048, Some(2049)), Err(2049));
        // At and below the bound: the read proceeds (bounded).
        assert_eq!(check_declared(2048, Some(2048)), Ok(()));
        assert_eq!(check_declared(2048, Some(0)), Ok(()));
        // No declaration (chunked): the bounded read decides.
        assert_eq!(check_declared(2048, None), Ok(()));
    }

    #[test]
    fn the_record_is_the_spec6_pre_pipeline_class() {
        let rec = too_large_record(
            "req-1",
            WireApi::Chat,
            4096,
            Some(5000),
            1_789_256_462,
            2,
            "0123456789abcdef",
        );
        let v = serde_json::to_value(&rec).unwrap();
        assert_eq!(v["config_digest"], "0123456789abcdef");
        assert_eq!(v["identity"]["event_id"], 0);
        assert_eq!(v["usage_missing"], true);
        assert_eq!(v["result"]["status"], 413);
        assert_eq!(v["result"]["upstream_ms"], serde_json::Value::Null);
        assert_eq!(v["errors"][0]["kind"], "request_too_large");
        assert_eq!(v["errors"][0]["details"]["limit_bytes"], 4096);
        assert_eq!(v["errors"][0]["details"]["content_length"], 5000);
        assert_eq!(v["decision"]["provider"], "");
        assert_eq!(v["cost"]["total"], 0);
    }

    #[test]
    fn the_chunked_arm_names_the_undeclared_length_as_null() {
        let rec = too_large_record("req-2", WireApi::Responses, 4096, None, 0, 0, "");
        let v = serde_json::to_value(&rec).unwrap();
        assert_eq!(
            v["errors"][0]["details"]["content_length"],
            serde_json::Value::Null
        );
    }
}
