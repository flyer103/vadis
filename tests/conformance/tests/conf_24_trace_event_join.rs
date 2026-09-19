//! CONF-24 (§12.8): **trace ↔ event join** — every `DecisionRecord` carries
//! an `event_id` that exists in `events` with `kind = request.received` and
//! the same `request_id`; the request's accounting rows carry a `trace_ref`
//! that resolves to that record's own line.
//!
//! Depends on the trace writer (R2-2e/R2-3 land the DecisionRecord path);
//! the store side that the join anchors on is already asserted by CONF-20
//! (the `request.received` row exists and is first for the request).

#[test]
#[ignore = "CONF-24: depends on the trace writer and the DecisionRecord identity group (R2-2e/R2-3)"]
fn conf_24_trace_event_join() {}
