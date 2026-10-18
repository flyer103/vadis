//! CONF-115 (spec §9.4.5 / §6's three-bucket predicate, ADR-056 §2.4,
//! ADR-057 §4 limb (e)): **the partition is ADR-056's.** With the
//! truncated-tail eighth request appended (a streamed head, one event,
//! then silence past the idle bound — 2xx with an `errors[]` entry whose
//! `details.error_class` is `stream_truncated`), the summary puts that
//! row in `stream_incomplete`, **not** `succeeded`; the three counts sum
//! to `steps`; and the two rows without usage (the usage-less answer and
//! the truncated one) move no bucket. **Red** for a reducer that counts
//! by `result.status` alone — it would report the truncated row as a
//! success, which is exactly the flattery §9.2 refuses.

#![forbid(unsafe_code)]

mod common;

use common::{run_steps_session, STEPS_SESSION};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_115_the_partition_is_adr_056s() {
    let (config_path, records) = common::run_steps_session("conf115", true).await;
    assert_eq!(records.len(), 8, "the truncated tail makes eight requests");
    // Ground truth: the eighth record is 2xx with a stream_truncated
    // entry and no usage (CONF-108/109's own rig shape).
    let trunc = &records[7];
    assert_eq!(trunc["result"]["status"], 200);
    assert!(trunc["errors"]
        .as_array()
        .expect("errors is always an array")
        .iter()
        .any(|e| e["details"]["error_class"] == "stream_truncated"));
    assert_eq!(trunc["usage_missing"], true);

    let st = vadis_cli::steps::load_session(&config_path.to_string_lossy(), STEPS_SESSION)
        .expect("the session loads");
    let rep = vadis_cli::steps::reduce(&st).expect("the reducer runs");
    assert_eq!(rep.rows.len(), 8);

    // The three buckets, reused — not redefined — from ADR-056 §2.4.
    // Seven clean successes (the usage-less answer among them — its
    // absence moves no bucket) and the truncated stream as the one
    // incomplete: counting by `result.status` alone would read 8.
    assert_eq!(rep.succeeded, 7, "the truncated row is NOT a success");
    assert_eq!(rep.failed, 0);
    assert_eq!(rep.stream_incomplete, 1, "the truncated row IS incomplete");
    assert_eq!(
        rep.succeeded + rep.failed + rep.stream_incomplete,
        rep.rows.len() as u64,
        "the three buckets PARTITION steps"
    );

    // The truncated row itself: a repeat (the bytes were resent), the
    // trunc flag, absences — never zeros — for the usage columns.
    let row = &rep.rows[7];
    assert_eq!(row.class, "repeat");
    assert!(row.flags.iter().any(|f| f == "trunc"), "{:?}", row.flags);
    assert_eq!(row.r#in, None);
    assert_eq!(row.cached, None);
    assert_eq!(row.money, None);

    // usage_missing is the orthogonal second partition: TWO rows lack a
    // measurement here (the usage-less answer and the truncated one),
    // and neither moved a bucket. The usage-less row rides a success;
    // the truncated row rides an incomplete — same absence, different
    // buckets, exactly why the two axes are separate.
    assert_eq!(rep.usage_missing, 2);
    assert_eq!(rep.cache_excluded, 2);
}
