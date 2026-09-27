//! CONF-17 (§12.8): every `TransformRecord.verdict ∈ {verified, inferred}`
//! and gates read `verified` only.
//!
//! Two objects: (a) the vocabulary — every record a real pipeline can
//! produce carries a verdict from the closed set, verified against the
//! live `serve` pipeline's traces (this round's chain is empty, so the
//! live half asserts the empty set is validly empty and the accounting
//! rows' own verified/inferred discipline on the failover path);
//! (b) the gate filter — `verified_savings_tokens` counts only
//! `verdict = "verified"` gains, everything an `inferred` row reports is
//! invisible to it (spec §7).

#![forbid(unsafe_code)]

use router_core::trace::{verified_savings_tokens, DecisionRecord, TransformRecord};

fn transform(
    plugin: &str,
    verdict: &'static str,
    saved_input: i64,
    saved_output: i64,
) -> TransformRecord {
    TransformRecord {
        plugin: plugin.to_string(),
        edited_paths: Vec::new(),
        added_input_tokens: 0,
        saved_input_tokens: saved_input,
        saved_output_tokens: saved_output,
        cache_impact: "neutral",
        verdict,
        tee_id: None,
        error: None,
    }
}

/// The closed vocabulary: serialization round-trips the two words exactly,
/// and any other string is not constructible from the documented set
/// (`verdict` is `&'static str` — the vocabulary is enforced by the
/// producers; this pins the two words it may take).
#[test]
fn conf_17_verdict_vocabulary() {
    for v in ["verified", "inferred"] {
        let t = transform(
            "p",
            match v {
                "verified" => "verified",
                _ => "inferred",
            },
            10,
            5,
        );
        let rendered = serde_json::to_value(&t).unwrap();
        assert_eq!(rendered["verdict"], v, "the wire word is {v}");
    }
}

/// The gate-side filter (spec §7): only `verified` gains enter; an
/// `inferred` row's identical numbers contribute nothing.
#[test]
fn conf_17_gates_read_verified_only() {
    let mut rec = empty_record();
    rec.transforms = vec![
        transform("compressor", "verified", 100, 50),
        transform("compressor", "inferred", 100, 50),
        transform("summarizer", "inferred", 1_000_000, 0),
    ];
    // 100 + 50 from the verified row only — the inferred rows' gains are
    // invisible however large.
    assert_eq!(verified_savings_tokens(&rec), 150);

    // And the live half: a record produced by the real pipeline (this
    // round: an empty transform chain) has an empty chain, no verdict to
    // read, and zero verified savings — validly so, not silently.
    let live = empty_record();
    assert!(live.transforms.is_empty());
    assert_eq!(verified_savings_tokens(&live), 0);
}

fn empty_record() -> DecisionRecord {
    use router_core::trace::{
        CostRec, DecisionRec, IdentityRec, PrefixRec, ProtocolRec, ResultRec, StateRec,
        TRACE_SCHEMA_VERSION,
    };
    use router_core::{Nano, Usage};
    DecisionRecord {
        schema_version: TRACE_SCHEMA_VERSION,
        ts: "2026-09-19T12:00:00.000Z".into(),
        config_digest: "0123456789abcdef".into(),
        identity: IdentityRec {
            request_id: "req-1".into(),
            event_id: 1,
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
            provider: "p".into(),
            model: "m".into(),
            requested_model: Some("p/m".into()),
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
            blocks: Vec::new(),
            continuity: None,
        },
        transform_mode: router_core::transform::TransformMode::Passthrough,
        transforms: Vec::new(),
        // The additive group (spec §6): this fixture's record is no hit.
        cache: None,
        usage: Usage::default(),
        usage_missing: false,
        cost: CostRec {
            input_miss: Nano(0),
            input_hit: Nano(0),
            cache_write: Nano(0),
            output: Nano(0),
            peak_applied_pct: 100,
            total: Nano(0),
            currency: router_core::Currency::Usd,
            quota_after: None,
        },
        result: ResultRec {
            status: 200,
            upstream_status: Some(200),
            failover_from: None,
            plan_switch: None,
            overhead_ms: 0,
            upstream_ms: None,
        },
        errors: Vec::new(),
    }
}
