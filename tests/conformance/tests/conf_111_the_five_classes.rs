//! CONF-111 (spec §9.4.2, ADR-057 §4 limb (a)): **the five step classes,
//! over a trace a real `serve` wrote.** The rig drives the canned session
//! (two appends, one byte-identical resend, one trimmed resend, one
//! mid-prefix change) through the real `serve` assembly over a loopback
//! mock, then asserts the reducer's classification **over the trace it
//! generated** — never a hand-written fixture (ADR-057 §4: a fixture
//! asserts its author's idea of the writer).
//!
//! The expected sequence is `first, append, append, repeat, shorten,
//! rewrite, append`, with each row's `blocks`/`kept`/`at` matching the
//! diff the script produced. **Red** for a reducer that classifies by
//! `identity.turn_index` (which reports `2` for every row after the
//! first — ADR-057 §9 F2) and for one that treats a `shorten` as a
//! `rewrite` (the dropped tail is not a divergence).

#![forbid(unsafe_code)]

mod common;

use common::{run_steps_session, STEPS_SESSION};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_111_the_five_classes_in_the_canned_order() {
    let (config_path, records) = common::run_steps_session("conf111", false).await;
    assert_eq!(records.len(), 7, "the canned session is seven requests");
    let st = vadis_cli::steps::load_session(&config_path.to_string_lossy(), STEPS_SESSION)
        .expect("the session loads");
    let rep = vadis_cli::steps::reduce(&st).expect("the reducer runs");

    let classes: Vec<&str> = rep.rows.iter().map(|r| r.class).collect();
    assert_eq!(
        classes,
        ["first", "append", "append", "repeat", "shorten", "rewrite", "append"],
        "the five §9.4.2 words, over the generated trace"
    );

    // The relation facts beside the words (§9.4.2: the class is a NAME
    // over a verified relation; the row carries the relation).
    let first = &rep.rows[0];
    assert_eq!(
        (first.kept, first.blocks, first.at.as_deref()),
        (0, 1, None)
    );
    let append1 = &rep.rows[1];
    assert_eq!(
        (append1.kept, append1.blocks, append1.at.as_deref()),
        (1, 1, None)
    );
    let append2 = &rep.rows[2];
    assert_eq!(
        (append2.kept, append2.blocks, append2.at.as_deref()),
        (2, 1, None)
    );
    let repeat = &rep.rows[3];
    assert_eq!(
        (repeat.kept, repeat.blocks, repeat.at.as_deref()),
        (3, 0, None)
    );
    assert_eq!(repeat.new_tok, 0, "a repeat attributes no new tokens");
    // The trimmed resend: [A,B,C] -> [A,B] — a strict prefix, the
    // divergence named at the first DROPPED block (#2, message C).
    let shorten = &rep.rows[4];
    assert_eq!(
        (shorten.kept, shorten.blocks, shorten.at.as_deref()),
        (2, -1, Some("#2:message")),
        "the earlier list's tail was dropped, not rewritten"
    );
    // A shorten re-sends a KEPT PREFIX: `lcp == len(L(n))`, so column
    // 12 (Σ blocks[i ≥ lcp].tokens) is empty BY CONSTRUCTION — the 0
    // is the defined measure, not the kept-prefix token count (the
    // review's trap: "fixing" it would swap measures, silently).
    assert_eq!(
        shorten.new_tok, 0,
        "a shorten attributes no new tokens — the kept prefix is not new"
    );
    // The mid-prefix change: [A,B] -> [A,X,C] — kept 1, +2 blocks, the
    // first differing block named at #1.
    let rewrite = &rep.rows[5];
    assert_eq!(
        (rewrite.kept, rewrite.blocks, rewrite.at.as_deref()),
        (1, 2, Some("#1:message"))
    );
    let append3 = &rep.rows[6];
    assert_eq!(
        (append3.kept, append3.blocks, append3.at.as_deref()),
        (3, 1, None)
    );

    // The step ordinal is the command's own, never the record's
    // `identity.turn_index` (ADR-057 §9 F2: that field reports 2 for
    // every row after the session's first — an analysis surface
    // reading it as an ordinal is off by the sticky hits).
    for (i, row) in rep.rows.iter().enumerate() {
        assert_eq!(row.step as usize, i + 1);
    }

    // Each `by class` new_tok is the SUM of that class's printed rows —
    // a relation, never a constant (a constant would survive a reducer
    // that miscounts the very rows it labels; the review's arm).
    for (class, agg) in &rep.by_class {
        let sum: u64 = rep
            .rows
            .iter()
            .filter(|r| r.class == *class)
            .map(|r| r.new_tok)
            .sum();
        assert_eq!(
            agg.new_tok, sum,
            "by-class new_tok restates the rows it summarises ({class})"
        );
        let count: u64 = rep.rows.iter().filter(|r| r.class == *class).count() as u64;
        assert_eq!(
            agg.count, count,
            "by-class count restates the rows ({class})"
        );
    }
}
