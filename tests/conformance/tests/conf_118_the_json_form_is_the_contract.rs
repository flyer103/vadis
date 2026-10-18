//! CONF-118 (spec §9.4.1's `--last`/`--json` rows, §9.4.3 column 16,
//! §9.4.5's `by class` and ADR-057 §2.2's header caveat): **the JSON
//! form is the contract, read where the bytes are made** —
//! `steps::report_json` / `steps::render_text`, the same builders the
//! CLI prints. Root cause of the defects this case pins: no conformance
//! case read the `--json` form at all, so `--last` was a silent no-op
//! under it (the review measured 441 rows for `--last 3 --json`) and
//! `status` shipped as a string where column 16 says integer.
//!
//! Limbs, over the trace a real `serve` wrote (the shared step-ledger
//! rig — never a hand-written fixture):
//!
//! - **`--last` under `--json`** — `rows` is the SAME tail slice the
//!   text form prints (steps 5–7 of the seven-request session), the
//!   summary states `showing {last, of}`, and `steps` stays the
//!   pre-`--last` count (§9.4.5). **Red** for a builder that ignores
//!   `last`;
//! - **`status` typed per column 16** — an integer on every normal
//!   (mock-200) row, and the `"<status>→<upstream_status>"` string
//!   ONLY on a row whose upstream status differs. The mock answers 200
//!   to everything, so the arrow arm is derived on a **copy** of one
//!   loaded record (conf_116's precedent: a mutation says so) —
//!   **red** for the unconditional `status: String`;
//! - **the five counts, in both forms** — `by_class` carries §9.4.2's
//!   five words and the text form prints all five lines. The canned
//!   session contains all five classes, so the zero arm is asserted on
//!   a three-record **prefix** of the same generated trace (first,
//!   append, append — no repeat, no shorten, no rewrite). **Red** for
//!   a summary that prints only the classes that occur;
//! - **the header caveat** — the text header names the class as a name
//!   over the verified block relation (ADR-057 §2.2). **Red** for a
//!   header that lets a reader read `shorten` as "the client
//!   compacted".

#![forbid(unsafe_code)]

mod common;

use common::{run_steps_session, STEPS_SESSION};
use vadis_cli::steps::{self, SessionTrace};

/// §9.4.2's five words, in the table's order — the same canned order
/// the renderers print (kept in one place in `steps`; duplicated here
/// so the case fails if the vocabulary itself drifts).
const FIVE: [&str; 5] = ["first", "append", "repeat", "shorten", "rewrite"];

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_118_the_json_form_is_the_contract() {
    let (config_path, _records) = common::run_steps_session("conf118", false).await;
    let st = steps::load_session(&config_path.to_string_lossy(), STEPS_SESSION)
        .expect("the session loads");
    let rep = steps::reduce(&st).expect("the reducer runs");
    assert_eq!(rep.rows.len(), 7, "the canned session is seven requests");

    // -- --last under --json: the same tail slice, the fact stated ----
    let v = steps::report_json(&rep, Some(3));
    let rows = v["rows"].as_array().expect("rows is an array");
    assert_eq!(rows.len(), 3, "the SAME tail slice the text form prints");
    let shown: Vec<u64> = rows
        .iter()
        .map(|r| r["step"].as_u64().expect("step is an integer"))
        .collect();
    assert_eq!(shown, vec![5, 6, 7], "the LAST three steps");
    assert_eq!(
        v["summary"]["steps"], 7,
        "steps stays the pre---last count (§9.4.5)"
    );
    assert_eq!(v["summary"]["showing"]["last"], 3);
    assert_eq!(v["summary"]["showing"]["of"], 7);
    // Without --last there is nothing to state and nothing to truncate.
    let full = steps::report_json(&rep, None);
    assert_eq!(full["rows"].as_array().unwrap().len(), 7);
    assert!(
        full["summary"].get("showing").is_none(),
        "showing exists only when --last actually truncated"
    );

    // -- status typed per §9.4.3 column 16 --------------------------------
    for r in full["rows"].as_array().unwrap() {
        assert!(
            r["status"].is_u64(),
            "a normal row's status is an integer, not {:?}",
            r["status"]
        );
    }
    // The arrow arm: the mock answers 200 to everything, so a differing
    // upstream_status is DERIVED — on a copy of the loaded session
    // (conf_116's precedent), never by editing the trace files.
    let mut differed: SessionTrace = SessionTrace {
        records: st.records.clone(),
        files_read: st.files_read,
        plan_providers: st.plan_providers.clone(),
        trace_dir: st.trace_dir.clone(),
    };
    differed.records[2]["result"]["upstream_status"] = serde_json::json!(502);
    let rep2 = steps::reduce(&differed).expect("the derived session reduces");
    let v2 = steps::report_json(&rep2, None);
    for (i, r) in v2["rows"].as_array().unwrap().iter().enumerate() {
        if i == 2 {
            assert_eq!(
                r["status"].as_str(),
                Some("200→502"),
                "the arrow string ONLY when the two differ"
            );
        } else {
            assert!(
                r["status"].is_u64(),
                "an absent/equal upstream_status keeps the integer ({i})"
            );
        }
    }

    // -- the five counts, in both forms ----------------------------------
    for word in FIVE {
        assert!(
            v2["summary"]["by_class"].get(word).is_some(),
            "the JSON by_class carries all five words ({word})"
        );
    }
    // The zero arm: a three-record prefix of the SAME generated trace —
    // first, append, append; repeat/shorten/rewrite never occur there.
    let prefix: SessionTrace = SessionTrace {
        records: st.records[..3].to_vec(),
        files_read: st.files_read,
        plan_providers: st.plan_providers.clone(),
        trace_dir: st.trace_dir.clone(),
    };
    let rep3 = steps::reduce(&prefix).expect("the prefix reduces");
    let v3 = steps::report_json(&rep3, None);
    assert_eq!(v3["summary"]["by_class"]["first"]["count"], 1);
    assert_eq!(v3["summary"]["by_class"]["append"]["count"], 2);
    for word in ["repeat", "shorten", "rewrite"] {
        assert_eq!(
            v3["summary"]["by_class"][word]["count"], 0,
            "a class that never occurs prints count 0, never an absence ({word})"
        );
        assert_eq!(v3["summary"]["by_class"][word]["new_tok"], 0);
    }
    // The text form prints the same five lines, and the header carries
    // ADR-057 §2.2's caveat (the class names the relation, not the
    // client's intent — a shorten may be a compaction, a trim, a rewind).
    let text = steps::render_text(&rep3, None);
    assert!(
        text.contains("class names the verified block relation"),
        "the header says what class is"
    );
    for (word, count) in [
        ("first", "count 1"),
        ("append", "count 2"),
        ("repeat", "count 0"),
        ("shorten", "count 0"),
        ("rewrite", "count 0"),
    ] {
        assert!(
            text.lines()
                .any(|l| l.trim_start().starts_with(word) && l.contains(count)),
            "the text summary prints the {word} line with {count}"
        );
    }
}
