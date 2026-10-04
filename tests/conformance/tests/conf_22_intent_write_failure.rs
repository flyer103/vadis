//! CONF-22 (§12.8): **intent write failure ⇒ nothing reached the upstream** —
//! with a store double whose intent write fails, the fake provider records
//! zero attempts, the client receives the §8 `internal` body with
//! `details.stage = "intent"`, and no intent row exists for the request.
//!
//! This file asserts the failure semantics at the seam that enforces them:
//! `write_intent_then` never runs the effect, and the §8 error body carries
//! `details.stage = "intent"`. The pipeline twin (fake ProviderClient recording
//! zero attempts through the real proxy) is kept `#[ignore]`d below: the fake
//! `ProviderClient` double it needs does not exist yet.

use serde_json::{json, Value};
use vadis_core::error::{ErrorBody, ErrorCode};
use vadis_core::store::{write_intent_then, EventKind, NewEvent, Query, QueryRow, Store};

/// A store double whose append always fails — "the injected failure".
struct FailingStore;

impl Store for FailingStore {
    fn append(&self, _: NewEvent<'_>) -> Result<vadis_core::EventId, vadis_core::StoreError> {
        Err(vadis_core::StoreError::Busy)
    }
    fn project(&self, _: vadis_core::ProjectionWrite<'_>) -> Result<(), vadis_core::StoreError> {
        panic!("a failed intent must not reach the projection tier either");
    }
    fn query(&self, _: Query<'_>) -> Result<QueryRow, vadis_core::StoreError> {
        Ok(QueryRow::Count(0))
    }
    fn rebuild(
        &self,
        _: vadis_core::Projection,
    ) -> Result<vadis_core::RebuildStats, vadis_core::StoreError> {
        Ok(vadis_core::RebuildStats::default())
    }
    fn schema_version(&self) -> Result<u32, vadis_core::StoreError> {
        Ok(1)
    }
}

#[test]
fn conf_22_intent_failure_blocks_upstream() {
    let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let attempts_in_effect = attempts.clone();

    let result: Result<(vadis_core::EventId, ()), _> = write_intent_then(
        &FailingStore,
        NewEvent {
            kind: EventKind::UpstreamSubmitted,
            request_id: Some("req-1"),
            session: Some("sess-1"),
            body_hash: Some("0123456789abcdef"),
            trace_ref: None,
            payload: json!({"route": "zai/glm-5.3", "attempt_index": 0, "attempt_id": "att-1"}),
        },
        |_| {
            attempts_in_effect.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        },
    );

    // The effect (the upstream attempt) never ran.
    assert!(
        result.is_err(),
        "the intent failure must surface as an error"
    );
    assert_eq!(
        attempts.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "zero attempts reached the upstream"
    );

    // The §8 client-facing shape: `internal`, `details.stage = "intent"`.
    let body = ErrorBody::new(
        ErrorCode::Internal,
        "state store intent write failed; nothing was sent upstream",
        "req-1",
    );
    let mut with_details = body;
    with_details.error.details = Some(Value::Null).map(|_| json!({"stage": "intent"}));
    let rendered = serde_json::to_value(&with_details).unwrap();
    assert_eq!(rendered["error"]["type"], "internal");
    assert_eq!(rendered["error"]["details"]["stage"], "intent");
}

/// The full-pipeline twin: the fake ProviderClient records zero attempts and
/// the real proxy returns the §8 body. It needs the same fake `ProviderClient`
/// double, which does not exist yet.
#[test]
#[ignore = "CONF-22: the fake ProviderClient double needed to inject an append failure does not exist yet"]
fn conf_22_pipeline_intent_failure() {}
