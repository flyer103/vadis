//! ADR-060 D1 (spec §6's cancelled-request class): a request that
//! reached the process has a terminal row, and the row says which kind
//! of unknown it is. The client closing mid-request drops the future
//! (buffered medium) or the response body (SSE medium) `hyper` is
//! running — that drop is the **only** path that can leave a request
//! with no terminal outcome of its own, so the row is written from
//! exactly there: a guard whose `Drop` is the mechanism.
//!
//! The invariant is mechanical, not editorial (spec §6, "the
//! invariant"): **at most one terminal row per request**. The record's
//! context is owned by one slot; every ordinary terminal writer
//! **takes** it ([`CancelSlot::take`]) before it writes, and the guard
//! writes only if it still holds the context. The take comes before the
//! write deliberately — if the ordinary write then fails, the row is
//! lost rather than duplicated (the `trace_write_failed` residual we
//! already tolerate beats a second, unknown-outcome row on a request
//! that already has a terminal one).
//!
//! The guard owns the facts its row needs, because the drop destroys
//! the relay's own state (spec §6): the head's status when one arrived,
//! a byte counter the relay updates as it forwards (shared, never
//! copied at drop time), the attempt count, and the request's
//! `wrote_full_request` evidence. The arm follows from one fact alone —
//! the relay was entered (a `2xx` head forwarded) ⇒ arm B, it was not ⇒
//! arm A:
//!
//! - **arm A** — no response body in flight: `result.status: 499`
//!   (nginx's own "client closed request"; a word this format owns,
//!   never written to a socket), `errors[0].kind: "canceled"`,
//!   `details: { stage, canceled_by, attempt, wrote_full_request }`.
//! - **arm B** — a `2xx` head was forwarded and the body was in
//!   flight: `result.status` is the head's, the entry is §4.20.3's
//!   truncation shape with `stream_end: "client_closed"` and the
//!   §4.2 retry-refusal word.
//!
//! `canceled_by` is `"shutdown"` when the process's graceful drain is
//! what dropped the request ([`set_shutdown_draining`]) and `"client"`
//! otherwise — the two producers spec §6 names. The honest boundary is
//! stated, not hidden: a `SIGKILL` writes nothing (no in-process
//! mechanism can), and that population is the event log's
//! `upstream.submitted` intent, not this row.
//!
//! The writer the drop calls is the same **synchronous**
//! [`vadis_core::TraceWriter`] every other record uses — no runtime, no
//! timer at drop time — and the guard holds its own `Arc`s to the
//! store and the sink, so the trace sink cannot close while an
//! in-flight request exists (the graceful-shutdown ordering ADR-060 §5
//! names as an obligation holds structurally: `serve`'s frame, and with
//! it the sink, outlives every guard).

use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::json;
use vadis_core::config::WireApi;
use vadis_core::prefix::PrefixBlock;
use vadis_core::store::{EventId, Store};
use vadis_core::trace::{PlanSwitchRec, TraceError, TransformRecord};
use vadis_core::transform::TransformMode;

use crate::accounting::{AccountCtx, Accountant};

/// Set by `serve` the moment the shutdown signal fires — BEFORE the
/// graceful drain begins dropping work — so a guard whose owner is
/// dropped during the drain writes `canceled_by: "shutdown"` instead of
/// blaming the client. A process that never sets it (tests, tools)
/// reports `"client"` for every cancellation, which is the only other
/// producer.
static SHUTDOWN_DRAINING: AtomicBool = AtomicBool::new(false);

/// `serve` calls this on SIGINT/SIGTERM, before the drain.
pub fn set_shutdown_draining() {
    SHUTDOWN_DRAINING.store(true, Ordering::SeqCst);
}

fn is_shutdown_draining() -> bool {
    SHUTDOWN_DRAINING.load(Ordering::SeqCst)
}

/// Everything the cancelled row needs that does not change while the
/// request is in flight — an owned snapshot of the accounting inputs,
/// refreshed at each attempt's start (the only moments a drop can strand
/// an in-flight attempt with stale facts).
pub(crate) struct CancelCtx {
    pub request_id: String,
    pub received_event: Option<EventId>,
    pub proto_in: WireApi,
    pub proto_out: Option<WireApi>,
    pub session: Option<String>,
    pub turn_index: u32,
    pub requested_model: Option<String>,
    pub selection_source: &'static str,
    pub decision_ms: u32,
    pub started: Instant,
    pub now_epoch_s: u64,
    pub now_us: i64,
    pub plan_switch: Option<PlanSwitchRec>,
    pub sticky_hit: bool,
    pub transform_mode: TransformMode,
    pub transforms: Vec<TransformRecord>,
    pub transform_error: Option<TraceError>,
    pub blocks: Vec<PrefixBlock>,
    /// The route the record names: the last attempted route when one
    /// was, else the route the request resolved to.
    pub provider: String,
    pub model: String,
    pub failover_from: Option<String>,
}

/// Build the owned snapshot from the request's facts (the walks call
/// this where the request commits to being served, and again at each
/// attempt's start so a drop never strands an in-flight attempt on
/// stale facts). `provider`/`model` name the route the record names:
/// the route being attempted when one is, else the resolved route.
pub(crate) fn snapshot_facts(
    facts: &crate::forward::RequestFacts<'_>,
    provider: &str,
    model: &str,
) -> CancelCtx {
    CancelCtx {
        request_id: facts.request_id.to_string(),
        received_event: facts.received_event,
        proto_in: facts.proto_in,
        proto_out: facts.proto_out,
        session: facts.session.clone(),
        turn_index: facts.turn_index,
        requested_model: facts.requested_model.clone(),
        selection_source: facts.selection_source,
        decision_ms: facts.decision_ms,
        started: facts.started,
        now_epoch_s: facts.now_epoch_s,
        now_us: facts.now_us,
        plan_switch: facts.plan_switch.clone(),
        sticky_hit: facts.sticky_hit,
        transform_mode: facts.transform_mode,
        transforms: facts.transform_records.clone(),
        transform_error: facts.transform_error.clone(),
        blocks: facts.blocks.clone(),
        provider: provider.to_string(),
        model: model.to_string(),
        failover_from: facts
            .failover_from
            .as_ref()
            .map(|r| format!("{}/{}", r.provider, r.model)),
    }
}

/// The one-owner slot (spec §6's invariant): the static context behind
/// a mutex the ordinary terminal writer empties with [`Self::take`],
/// plus the facts the relay and the walks update as they run — shared
/// atomics, read at drop time, never a stale copy.
pub(crate) struct CancelSlot {
    ctx: Mutex<Option<CancelCtx>>,
    attempts: AtomicU32,
    wrote_full_request: AtomicBool,
    /// The last head status that arrived, when one did (an error head
    /// being composed when the client left is arm A with its
    /// `upstream_status` intact).
    upstream_status: AtomicU16,
    has_upstream_status: AtomicBool,
    upstream_ms: AtomicU32,
    has_upstream_ms: AtomicBool,
    /// The relay was entered — a `2xx` head was forwarded and the body
    /// was in flight. This one fact alone decides the arm.
    relay_entered: AtomicBool,
    relayed_bytes: AtomicU64,
    /// Epoch-ms instants for §4.20.3's `since_last_byte_ms` (the relay
    /// updates `last_byte_epoch_ms` as it forwards; `head_epoch_ms` is
    /// the fallback when no byte ever arrived).
    head_epoch_ms: AtomicU64,
    last_byte_epoch_ms: AtomicU64,
}

impl CancelSlot {
    pub(crate) fn new() -> Self {
        Self {
            ctx: Mutex::new(None),
            attempts: AtomicU32::new(0),
            wrote_full_request: AtomicBool::new(false),
            upstream_status: AtomicU16::new(0),
            has_upstream_status: AtomicBool::new(false),
            upstream_ms: AtomicU32::new(0),
            has_upstream_ms: AtomicBool::new(false),
            relay_entered: AtomicBool::new(false),
            relayed_bytes: AtomicU64::new(0),
            head_epoch_ms: AtomicU64::new(0),
            last_byte_epoch_ms: AtomicU64::new(0),
        }
    }

    /// Install (or replace) the static snapshot. Called where the
    /// request commits to being served — after the boundary refusals
    /// and the response-cache replay (both write their own row
    /// synchronously and hold no in-flight attempt), before the first
    /// upstream attempt — and refreshed at each attempt's start.
    pub(crate) fn refresh(&self, ctx: CancelCtx) {
        *self.ctx.lock().unwrap() = Some(ctx);
    }

    /// The ordinary terminal writer's half of the invariant: **take**
    /// the context before writing the row. A request that already has
    /// a terminal row can therefore never acquire a second one — the
    /// guard's `Drop` sees `None` and writes nothing.
    pub(crate) fn take(&self) -> Option<CancelCtx> {
        self.ctx.lock().unwrap().take()
    }

    /// The attempt bookkeeping at an attempt's start: the count and the
    /// evidence flag, reset to "not (yet) fully written" — the outcome
    /// sets it true when the request bytes were fully written, and an
    /// attempt dropped mid-write keeps the honest `false` (nothing this
    /// process observed reached a full write).
    pub(crate) fn note_attempt(&self, attempts: u32) {
        self.attempts.store(attempts, Ordering::SeqCst);
        self.wrote_full_request.store(false, Ordering::SeqCst);
    }

    /// The outcome's evidence: the request bytes were fully written
    /// (`Responded` / `WrittenNoResponse` / a stream head arrived).
    pub(crate) fn note_wrote_full_request(&self) {
        self.wrote_full_request.store(true, Ordering::SeqCst);
    }

    /// A head arrived (any status): its status and head latency, kept
    /// for whichever arm the drop later takes.
    pub(crate) fn note_upstream_status(&self, status: u16, upstream_ms: Option<u32>) {
        self.upstream_status.store(status, Ordering::SeqCst);
        self.has_upstream_status.store(true, Ordering::SeqCst);
        if let Some(ms) = upstream_ms {
            self.upstream_ms.store(ms, Ordering::SeqCst);
            self.has_upstream_ms.store(true, Ordering::SeqCst);
        }
    }

    /// The relay was entered: a `2xx` head was adopted and forwarded —
    /// from here the client holds a head, and a drop is arm B.
    pub(crate) fn note_relay_entered(&self) {
        self.relay_entered.store(true, Ordering::SeqCst);
        self.head_epoch_ms.store(epoch_ms_now(), Ordering::SeqCst);
    }

    /// The relay's own byte counter (spec §4.20.3's `relayed_bytes`):
    /// the running total and the arrival instant of the last upstream
    /// byte, updated as the relay forwards — shared state, never a
    /// copy, because the drop destroys the relay.
    pub(crate) fn note_bytes_relayed(&self, total: u64) {
        self.relayed_bytes.store(total, Ordering::SeqCst);
        self.last_byte_epoch_ms
            .store(epoch_ms_now(), Ordering::SeqCst);
    }
}

fn epoch_ms_now() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The guard itself: the slot plus the handles its row writes through.
/// Owned by what the server actually runs — the buffered medium's
/// handler future (a local in [`crate::forward::Forwarder::forward`]'s
/// frame) or, on the SSE medium, the response body ([`guard_body`]
/// moves it in, so dropping the body drops the guard) — so the drop
/// path `hyper` runs on a client disconnect is the path that runs this
/// `Drop`.
pub struct CancelGuard {
    slot: Arc<CancelSlot>,
    store: Option<Arc<dyn Store>>,
    trace: Option<Arc<dyn vadis_core::TraceWriter>>,
}

impl CancelGuard {
    pub(crate) fn new(
        store: Option<Arc<dyn Store>>,
        trace: Option<Arc<dyn vadis_core::TraceWriter>>,
    ) -> Self {
        Self {
            slot: Arc::new(CancelSlot::new()),
            store,
            trace,
        }
    }

    /// The walks' handle: refresh, take and note through this.
    pub(crate) fn slot(&self) -> &CancelSlot {
        &self.slot
    }

    /// An `Arc` clone of the slot, for contexts that outlive the
    /// handler future (the relay's own context).
    pub(crate) fn slot_arc(&self) -> Arc<CancelSlot> {
        Arc::clone(&self.slot)
    }
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        // The invariant's other half: an ordinary writer already took
        // the context ⇒ this request already has its terminal row ⇒
        // write nothing (a second row is impossible by construction,
        // not by discipline).
        let Some(ctx) = self.slot.take() else {
            return;
        };
        let canceled_by = if is_shutdown_draining() {
            "shutdown"
        } else {
            "client"
        };
        let slot = &self.slot;
        let attempts = slot.attempts.load(Ordering::SeqCst);
        let wrote_full_request = slot.wrote_full_request.load(Ordering::SeqCst);
        let (status, upstream_status, error) = if slot.relay_entered.load(Ordering::SeqCst) {
            // Arm B (spec §6's table): a 2xx head was forwarded, the
            // body was in flight. §4.20.3's truncation shape with the
            // fourth cause word; §4.20.5's rule writes no cooldown for
            // it (the route was healthy and the reader left) — nothing
            // in this arm orders one.
            let head_status = if slot.has_upstream_status.load(Ordering::SeqCst) {
                slot.upstream_status.load(Ordering::SeqCst)
            } else {
                200
            };
            let relayed_bytes = slot.relayed_bytes.load(Ordering::SeqCst);
            // §4.2's closed word set via §4.20.3's rule: output already
            // on the wire ⇒ "forwarded"; the cut happened before the
            // first byte and the table refused it as a billed attempt
            // (a 200 head is a model already generating).
            let retry_refused = if relayed_bytes > 0 {
                "forwarded"
            } else {
                "billed"
            };
            let anchor = {
                let last = slot.last_byte_epoch_ms.load(Ordering::SeqCst);
                if last > 0 {
                    last
                } else {
                    slot.head_epoch_ms.load(Ordering::SeqCst)
                }
            };
            let since_last_byte_ms = epoch_ms_now().saturating_sub(anchor);
            (
                head_status,
                Some(head_status),
                TraceError {
                    kind: "upstream_error".to_string(),
                    message: "stream truncated: the response body was dropped at the \
                              client (client_closed); not retried"
                        .to_string(),
                    plugin: None,
                    details: Some(json!({
                        "error_class": "stream_truncated",
                        "stream": true,
                        "stream_end": "client_closed",
                        "relayed_bytes": relayed_bytes,
                        "since_last_byte_ms": since_last_byte_ms,
                        "retry_refused": retry_refused,
                    })),
                },
            )
        } else {
            // Arm A: no response body was in flight. `stage` is
            // ADR-011 item 6's evidence word — an attempt was submitted
            // and the request bytes were fully written ⇒ the attempt
            // may have been billed (`unknown_outcome`); anything else
            // could not have been charged (`not_billed`).
            let stage = if attempts >= 1 && wrote_full_request {
                "unknown_outcome"
            } else {
                "not_billed"
            };
            (
                499,
                slot.has_upstream_status
                    .load(Ordering::SeqCst)
                    .then(|| slot.upstream_status.load(Ordering::SeqCst)),
                TraceError {
                    kind: "canceled".to_string(),
                    message: "the client closed the request before any response head \
                              existed; no answer was produced"
                        .to_string(),
                    plugin: None,
                    details: Some(json!({
                        "stage": stage,
                        "canceled_by": canceled_by,
                        "attempt": attempts,
                        "wrote_full_request": wrote_full_request,
                    })),
                },
            )
        };
        let upstream_ms = slot
            .has_upstream_ms
            .load(Ordering::SeqCst)
            .then(|| slot.upstream_ms.load(Ordering::SeqCst));
        let acc_ctx = AccountCtx {
            request_id: &ctx.request_id,
            received_event: ctx.received_event,
            proto_in: ctx.proto_in.as_str(),
            proto_out: ctx.proto_out.as_ref().map(|w| w.as_str()),
            session: ctx.session.as_deref(),
            turn_index: ctx.turn_index,
            selection_source: ctx.selection_source,
            requested_model: ctx.requested_model.as_deref(),
            // The cancelled row names no credential: the attempt's pool
            // cursor is a walk fact the drop destroyed.
            key_index: None,
            decision_ms: ctx.decision_ms,
            started: ctx.started,
            now_epoch_s: ctx.now_epoch_s,
            now_us: ctx.now_us,
            plan_switch: ctx.plan_switch.clone(),
            sticky_hit: ctx.sticky_hit,
            transform_mode: ctx.transform_mode,
            transforms: ctx.transforms.clone(),
            transform_error: ctx.transform_error.clone(),
        };
        Accountant {
            store: self.store.as_deref(),
            trace: self.trace.as_deref(),
            // A cancelled request is usage-missing by construction: no
            // price table is consulted and nothing is charged (spec §6's
            // cancelled table).
            accounting: None,
        }
        .finish_canceled(
            &acc_ctx,
            status,
            upstream_status,
            &ctx.provider,
            &ctx.model,
            ctx.failover_from.clone(),
            vec![error],
            &ctx.blocks,
            upstream_ms,
            attempts,
        );
    }
}

/// Moves the guard into the SSE medium's response body (ADR-060 D1:
/// "the body owns the relay stream, so dropping the body drops the
/// guard"): every polled item is the inner stream's, verbatim, and the
/// guard drops exactly when hyper drops the body.
pub(crate) fn guard_body(
    guard: CancelGuard,
    body: std::pin::Pin<Box<dyn futures::Stream<Item = bytes::Bytes> + Send>>,
) -> std::pin::Pin<Box<dyn futures::Stream<Item = bytes::Bytes> + Send>> {
    Box::pin(GuardedBody { guard, body })
}

struct GuardedBody {
    guard: CancelGuard,
    body: std::pin::Pin<Box<dyn futures::Stream<Item = bytes::Bytes> + Send>>,
}

impl futures::Stream for GuardedBody {
    type Item = bytes::Bytes;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        // The guard's whole purpose is to be dropped WITH this body
        // (ADR-060 D1); the read here only states that it rides along.
        let _guard = &self.guard;
        self.body.as_mut().poll_next(cx)
    }
}

// The unused-import dance stays honest: `AtomicU8` is not used and is
// not imported.
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use vadis_core::trace::DecisionRecord;

    /// The drain flag is process-global and crate tests run in parallel
    /// threads: every test that asserts a `canceled_by` word holds this
    /// lock, so the flag's set/reset window cannot leak into another
    /// test's row.
    static FLAG_LOCK: StdMutex<()> = StdMutex::new(());

    /// Captures every record the guard writes.
    #[derive(Default)]
    struct CaptureTrace {
        records: StdMutex<Vec<String>>,
    }

    impl vadis_core::TraceWriter for CaptureTrace {
        fn write(&self, rec: &DecisionRecord) -> Result<Option<String>, String> {
            self.records
                .lock()
                .unwrap()
                .push(serde_json::to_string(rec).unwrap());
            Ok(Some("test:0".into()))
        }
        fn config_digest(&self) -> &str {
            ""
        }
    }

    fn ctx(request_id: &str) -> CancelCtx {
        CancelCtx {
            request_id: request_id.to_string(),
            received_event: None,
            proto_in: WireApi::Chat,
            proto_out: Some(WireApi::Chat),
            session: None,
            turn_index: 1,
            requested_model: Some("mock/m".into()),
            selection_source: "explicit",
            decision_ms: 0,
            started: Instant::now(),
            now_epoch_s: 1_789_992_000,
            now_us: 1_789_992_000_000_000,
            plan_switch: None,
            sticky_hit: false,
            transform_mode: TransformMode::Passthrough,
            transforms: Vec::new(),
            transform_error: None,
            blocks: Vec::new(),
            provider: "mock".into(),
            model: "m".into(),
            failover_from: None,
        }
    }

    /// Arm A, the no-attempt shape: dropping an untouched guard writes
    /// exactly one row — `499`, `kind: canceled`, `stage: not_billed`,
    /// `canceled_by: client`, `attempts: 0`.
    #[test]
    fn drop_writes_the_arm_a_row_when_no_attempt_was_submitted() {
        let _flag = FLAG_LOCK.lock().unwrap();
        let trace = std::sync::Arc::new(CaptureTrace::default());
        let t = trace.clone() as std::sync::Arc<dyn vadis_core::TraceWriter>;
        let guard = CancelGuard::new(None, Some(t));
        guard.slot().refresh(ctx("req-a"));
        drop(guard);
        let records = trace.records.lock().unwrap().clone();
        assert_eq!(records.len(), 1, "exactly one terminal row");
        let rec: serde_json::Value = serde_json::from_str(&records[0]).unwrap();
        assert_eq!(rec["result"]["status"], 499);
        assert_eq!(rec["result"]["attempts"], 0);
        assert_eq!(rec["result"]["upstream_status"], serde_json::Value::Null);
        assert_eq!(rec["usage_missing"], true);
        let e = &rec["errors"][0];
        assert_eq!(e["kind"], "canceled");
        assert_eq!(e["details"]["stage"], "not_billed");
        assert_eq!(e["details"]["canceled_by"], "client");
        assert_eq!(e["details"]["attempt"], 0);
        assert_eq!(e["details"]["wrote_full_request"], false);
    }

    /// Arm A, the maybe-billed shape: an attempt was submitted and its
    /// request fully written ⇒ `stage: unknown_outcome`, the attempt
    /// count carried.
    #[test]
    fn arm_a_names_unknown_outcome_when_the_attempt_fully_wrote() {
        let _flag = FLAG_LOCK.lock().unwrap();
        let trace = std::sync::Arc::new(CaptureTrace::default());
        let t = trace.clone() as std::sync::Arc<dyn vadis_core::TraceWriter>;
        let guard = CancelGuard::new(None, Some(t));
        guard.slot().refresh(ctx("req-b"));
        guard.slot().note_attempt(1);
        guard.slot().note_wrote_full_request();
        drop(guard);
        let rec: serde_json::Value =
            serde_json::from_str(&trace.records.lock().unwrap()[0]).unwrap();
        assert_eq!(rec["result"]["status"], 499);
        assert_eq!(rec["result"]["attempts"], 1);
        assert_eq!(rec["errors"][0]["details"]["stage"], "unknown_outcome");
        assert_eq!(rec["errors"][0]["details"]["wrote_full_request"], true);
    }

    /// Arm B: the relay was entered (a 2xx head forwarded, bytes on the
    /// wire) ⇒ the truncation shape with `stream_end: client_closed`
    /// and `retry_refused: forwarded`, status the head's.
    #[test]
    fn arm_b_is_the_client_closed_truncation_with_the_forwarded_refusal() {
        let trace = std::sync::Arc::new(CaptureTrace::default());
        let t = trace.clone() as std::sync::Arc<dyn vadis_core::TraceWriter>;
        let guard = CancelGuard::new(None, Some(t));
        guard.slot().refresh(ctx("req-c"));
        guard.slot().note_attempt(1);
        guard.slot().note_wrote_full_request();
        guard.slot().note_upstream_status(200, Some(12));
        guard.slot().note_relay_entered();
        guard.slot().note_bytes_relayed(512);
        drop(guard);
        let rec: serde_json::Value =
            serde_json::from_str(&trace.records.lock().unwrap()[0]).unwrap();
        assert_eq!(rec["result"]["status"], 200);
        assert_eq!(rec["result"]["upstream_status"], 200);
        let e = &rec["errors"][0];
        assert_eq!(e["kind"], "upstream_error");
        assert_eq!(e["details"]["error_class"], "stream_truncated");
        assert_eq!(e["details"]["stream_end"], "client_closed");
        assert_eq!(e["details"]["relayed_bytes"], 512);
        assert_eq!(e["details"]["retry_refused"], "forwarded");
    }

    /// The invariant: a request whose ordinary writer took the slot
    /// gets no second row from the drop — the take, not discipline, is
    /// what makes it impossible.
    #[test]
    fn a_taken_slot_writes_no_row_on_drop() {
        let trace = std::sync::Arc::new(CaptureTrace::default());
        let t = trace.clone() as std::sync::Arc<dyn vadis_core::TraceWriter>;
        let guard = CancelGuard::new(None, Some(t));
        guard.slot().refresh(ctx("req-d"));
        guard.slot().note_relay_entered();
        // The ordinary terminal writer's half: take BEFORE the write.
        assert!(guard.slot().take().is_some());
        drop(guard);
        assert!(
            trace.records.lock().unwrap().is_empty(),
            "no second row: the slot was already taken"
        );
    }

    /// `canceled_by: shutdown`: the drain flag flips the word. Reset
    /// afterwards so other tests in this process keep the client word.
    #[test]
    fn the_drain_flag_names_shutdown_as_the_canceller() {
        let _flag = FLAG_LOCK.lock().unwrap();
        let trace = std::sync::Arc::new(CaptureTrace::default());
        let t = trace.clone() as std::sync::Arc<dyn vadis_core::TraceWriter>;
        let guard = CancelGuard::new(None, Some(t));
        guard.slot().refresh(ctx("req-e"));
        set_shutdown_draining();
        drop(guard);
        SHUTDOWN_DRAINING.store(false, Ordering::SeqCst);
        let rec: serde_json::Value =
            serde_json::from_str(&trace.records.lock().unwrap()[0]).unwrap();
        assert_eq!(rec["errors"][0]["details"]["canceled_by"], "shutdown");
    }
}
