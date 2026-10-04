//! CONF-77 (ADR-016 §13.3 L1c, fixed this round): **the streaming
//! relay's pre-relay walk consumes the same route-availability read as
//! everything else.** The SSE relay kept its own `Query::Cooldown`
//! read (`stream_forward.rs`'s free `in_cooldown`), so a cooled-down
//! provider could be skipped by the buffered walk but attempted by the
//! streaming one — the same request answered differently by medium.
//! It now delegates to `availability::provider_in_cooldown`, the same
//! single owner the buffered walk's skip and the probe gate call.
//!
//! Witness, on the real `serve` with a streamed request: a real spill
//! moves the family to overflow, the spill's 403 demotes `p-plan` for
//! the fixture's 1s (`Retry-After: 1`), and — while that demotion is
//! live — a **streaming** boundary request from a fresh session is
//! served by the overflow account with the plan mock never reached:
//! the relay's walk skipped the cooling primary exactly as the
//! buffered walk would. After the demotion passes, the same streaming
//! shape probes the primary and recovers the family (the skip was the
//! projection's, not a permanent refusal).
//!
//! The spill phase itself runs buffered (the rig's canned 403 +
//! `Retry-After`), pinning the demotion write; the assertions under
//! test are made on the streaming medium.
//!
//! R58 — how the inside-window rows are asserted without racing the
//! demotion's deadline (the R56-1 load red at :92 — "fixture invariant:
//! the demotion is still live" — red on byte-identical input, the same
//! class R56 fixed in conf_72/conf_73). The old text *assumed* the
//! turn-2 return trip, one health read and one streamed request would
//! all land inside the 1s demotion window; under load the demotion
//! legitimately expires first. The fixed case never assumes a position
//! in time: the spill's response is **bracketed** (`t2_0`/`t2_1` around
//! the turn-2 round trip, one clock, one machine), so the demotion's
//! deadline — written by the server inside that bracket, plus the
//! fixture's 1s — lies in a known interval, and every later instant is
//! *provably inside* (`t < t2_0 + 1s`), *provably past*
//! (`t >= t2_1 + 1s`), or honestly unprovable. The live word and the
//! walk's skip fire at full strictness where the bracket proves inside,
//! the walk's probe is forced where the bracket proves past, and a
//! straddling run still asserts the vocabulary, the `admitted ⟺ no
//! word` self-consistency, and that the walk's outcome is exactly one
//! of the owner's two answers (skip, or probe-and-recover — never a
//! third shape). The recovery witness keeps its oversleep-forced
//! position and, when the straddling request already performed it, is
//! read from the recovery's own event and trace rows instead — the
//! same property, proven from a record rather than a schedule. No
//! window was widened (the fixture's `Retry-After: 1` stands), no
//! assertion sleeps easier; the falsifiability control captured when
//! this case was made still witnesses the strictness.

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, CannedResponse, PlanRig, SseChunk};
use vadis_core::store::{Query, QueryRow, Store as _};

fn now_us() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0)
}

/// A streamed 200 whose final data event carries usage (CONF-30's
/// carrier shape) — so the relay's accounting closes normally.
fn sse_ok(content: &str) -> CannedResponse {
    let body = format!(r#"{{"choices":[{{"index":0,"delta":{{"content":"{content}"}}}}]}}"#);
    let usage = r#"{"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":5,"total_tokens":105,"prompt_tokens_details":{"cached_tokens":20}}}"#;
    CannedResponse::sse(vec![
        SseChunk::event(format!("data: {body}\n\n").as_bytes()),
        SseChunk::event(format!("data: {usage}\n\n").as_bytes()),
        SseChunk::event(b"data: [DONE]\n\n"),
    ])
}

/// The live `/health` section — the availability question's report
/// answer, read through the same single owner under test.
fn http_get_health(addr: &str) -> serde_json::Value {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(addr).expect("connect");
    let req = format!("GET /health HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    let text = String::from_utf8_lossy(&buf).into_owned();
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("");
    serde_json::from_str(body.trim()).expect("health json")
}

/// Every stored event as (kind_raw, payload), read after the server
/// stopped (CONF-44's reader).
fn events(dir: &std::path::Path) -> Vec<(String, serde_json::Value)> {
    let store = vadis_store::SqliteStore::open(&dir.join("state/router.db")).unwrap();
    let QueryRow::Events(rows) = store.query(Query::AllEvents).unwrap() else {
        panic!("events query");
    };
    rows.into_iter().map(|r| (r.kind_raw, r.payload)).collect()
}

/// Every trace record, in write order (the CONF-42 reader).
fn trace_records(dir: &std::path::Path) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir.join("state/traces")).expect("trace dir") {
        let entry = entry.unwrap();
        if entry.path().extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        for line in std::fs::read_to_string(entry.path()).unwrap().lines() {
            out.push(serde_json::from_str(line).expect("record json"));
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_77_streaming_walk_skips_a_cooling_primary_like_the_buffered_one() {
    let (plan, api, dir, listen_addr) =
        testkit::plan_rig_parts("conf77", "", testkit::PLAN_POLICY_DEFAULT).await;
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    let rig = PlanRig {
        plan,
        api,
        listen_addr,
        dir,
        serve_task,
    };

    // The spill, buffered: turn 1 answers on the plan, turn 2 meets the
    // 403 (which also demotes p-plan for 1s) and spills to the api.
    // The plan's third canned answer is SSE — it serves the streamed
    // probe at the end (the mock's queue is shared across media).
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_forbidden_403());
    rig.plan.queue(sse_ok("probe"));
    rig.api.queue(testkit::plan_ok("spilled"));
    // The streamed spill below (a lone queued response would be cloned
    // to every later request — the mock's authoring rule).
    rig.api.queue(sse_ok("stream-spilled"));
    let (s1, _b, _h) = rig.post(Some("S1"), 1);
    assert_eq!(s1, 200);

    // Bracket the spill's round trip: the demotion's write (Retry-After:
    // 1 → write instant + 1s) happens inside the server's turn-2
    // handling, so the deadline lies in (t2_0 + 1s, t2_1 + 1s]. A later
    // instant t is PROVABLY inside the demotion iff t < t2_0 + 1s and
    // provably past iff t >= t2_1 + 1s; between those, no position is
    // provable and the case says so instead of assuming one (the old
    // :92 assumed it — that assumption was the flake).
    let t2_0 = now_us();
    let (s2, _b2, _h2) = rig.post(Some("S1"), 2);
    let t2_1 = now_us();
    assert_eq!(s2, 200, "the 403 spills the family to overflow");
    let inside_if_before_us = t2_0 + 1_000_000;
    let past_if_at_or_after_us = t2_1 + 1_000_000;

    // While the demotion is live: a streaming boundary request from a
    // fresh session must be served by the overflow account — the
    // relay's walk skipped the cooling primary. (Cooldown 0s means the
    // only thing refusing the primary is ADR-011's projection; the
    // live /health answers that same availability question through the
    // same single owner, so its word here is the timing witness.) The
    // read is bracketed; the word is asserted only where the bracket
    // proves the position.
    let th0 = now_us();
    let h = http_get_health(&rig.listen_addr);
    let th1 = now_us();
    let blocked_by = &h["plan"]["probe"]["blocked_by"];
    let admitted = &h["plan"]["probe"]["admitted"];
    if th1 < inside_if_before_us {
        // Provably inside — the old :92 assertion, now position-proven.
        eprintln!("conf77 health branch: provably-inside");
        assert_eq!(
            *blocked_by, "primary_cooling_down",
            "provably inside the demotion: the surface must name it"
        );
        assert_eq!(*admitted, false);
    } else if th0 >= past_if_at_or_after_us {
        // Provably past: the demotion has legitimately expired.
        eprintln!("conf77 health branch: provably-past");
        assert_eq!(
            *blocked_by,
            serde_json::Value::Null,
            "provably past the demotion: no refusal word"
        );
        assert_eq!(*admitted, true);
    } else {
        // The bracket straddles the deadline: no position is provable.
        // What still holds at every instant: the surface speaks only
        // this configuration's vocabulary, and it agrees with itself.
        eprintln!("conf77 health branch: straddle");
        assert_eq!(
            admitted.as_bool().expect("admitted is a bool"),
            blocked_by.is_null(),
            "admitted and blocked_by are the same fact on the surface"
        );
        if let Some(w) = blocked_by.as_str() {
            assert_eq!(
                w, "primary_cooling_down",
                "the only refusal word this configuration can produce, got {w:?}"
            );
        }
    }

    let key = r#","prompt_cache_key":"S2""#;
    let body = format!(
        r#"{{"model":"p-plan/m1","messages":[{{"role":"user","content":"stream turn 1"}}]{key},"stream":true,"stream_options":{{"include_usage":true}}}}"#
    );
    let ts0 = now_us();
    let (ss, sb, _sh) = testkit::http_post(
        &rig.listen_addr,
        "/v1/chat/completions",
        body.as_bytes(),
        &[],
    );
    let ts1 = now_us();
    assert_eq!(ss, 200, "the streamed request was served");
    assert!(
        testkit::dechunk(&sb).ends_with(b"data: [DONE]\n\n"),
        "the stream relayed to completion"
    );
    // The walk's answer, read from the mock counters. Before this
    // request: plan 2 (t1, the 403), api 1 (the buffered spill).
    let plan_n = rig.plan.requests().len();
    let api_n = rig.api.requests().len();
    let skipped = plan_n == 2 && api_n == 2;
    let probed = plan_n == 3 && api_n == 1;
    if ts1 < inside_if_before_us {
        // Provably inside the demotion: the relay's walk must have
        // skipped the cooling primary — the old :114/:118 assertions,
        // now position-proven.
        eprintln!("conf77 stream branch: provably-inside");
        assert!(
            skipped,
            "provably inside the demotion: the relay's walk skipped the \
             cooling primary (no third plan hit, the overflow served) — \
             got plan {plan_n}, api {api_n}"
        );
    } else if ts0 >= past_if_at_or_after_us {
        // Provably past: the owner had already lifted the demotion when
        // the walk evaluated — the request must probe the primary (the
        // skip is the projection's verdict, never a permanent refusal).
        eprintln!("conf77 stream branch: provably-past");
        assert!(
            probed,
            "provably past the demotion: the walk probed the primary — \
             got plan {plan_n}, api {api_n}"
        );
    } else {
        // Position unprovable: the request raced the deadline. The
        // outcome must still be exactly one of the availability owner's
        // two answers — skip (plan 2, api 2) or probe-and-recover
        // (plan 3, api 1) — never a third shape.
        eprintln!("conf77 stream branch: straddle");
        assert!(
            skipped || probed,
            "the walk's outcome is exactly one of the owner's two answers: \
             skip (plan 2, api 2) or probe-and-recover (plan 3, api 1) — \
             got plan {plan_n}, api {api_n}"
        );
    }

    // After the demotion passes: the skip was the projection's verdict,
    // not a permanent refusal. The sleep starts after the spill's
    // bracket closed and can only overshoot, so this arm's position
    // (past) is a property of the fixture's 1s TTL, not of scheduling.
    tokio::time::sleep(std::time::Duration::from_millis(1_300)).await;
    let h = http_get_health(&rig.listen_addr);

    if skipped {
        // The family is still on overflow; the section's probe block is
        // live and provably past the demotion.
        assert_eq!(h["plan"]["account"], "overflow");
        assert_eq!(
            h["plan"]["probe"]["blocked_by"],
            serde_json::Value::Null,
            "the demotion has passed (provably past)"
        );
        assert_eq!(h["plan"]["probe"]["admitted"], true);

        // The same streaming shape now probes the primary and recovers
        // the family (CONF-39's sequence on the streaming medium, the
        // original arm unchanged).
        eprintln!("conf77 recovery arm: via S3 probe");
        let key = r#","prompt_cache_key":"S3""#;
        let body = format!(
            r#"{{"model":"p-plan/m1","messages":[{{"role":"user","content":"stream probe"}}]{key},"stream":true,"stream_options":{{"include_usage":true}}}}"#
        );
        let (sp, sp_body, _sph) = testkit::http_post(
            &rig.listen_addr,
            "/v1/chat/completions",
            body.as_bytes(),
            &[],
        );
        assert_eq!(sp, 200);
        assert!(
            testkit::dechunk(&sp_body).ends_with(b"data: [DONE]\n\n"),
            "the probe's stream relayed to completion"
        );
        assert_eq!(
            rig.plan.requests().len(),
            3,
            "the streaming boundary probed the primary (t1, the 403, the probe)"
        );
        assert_eq!(
            rig.api.requests().len(),
            2,
            "no further metered spend after the recovery"
        );
        rig.stop();
    } else {
        // The straddling boundary request already probed the primary and
        // recovered the family (the owner had lifted the demotion at the
        // walk's instant): the recovery the arm above witnesses on S3
        // happened on S2 instead. Assert it from the record — timing-
        // independent by construction.
        eprintln!("conf77 recovery arm: via S2 record");
        assert!(
            probed,
            "not skipped and not probed is impossible (the disjunction above)"
        );
        // The family is already recovered: a family on its primary has
        // nothing to probe back to (§4.6 rule 2), so the section's probe
        // block is inert — the account word is the live witness here,
        // and the recovery the skipped arm witnesses on S3 happened on
        // S2 instead (asserted from the record below, timing-independent
        // by construction).
        assert_eq!(
            h["plan"]["account"], "primary",
            "the straddling request's admitted probe already recovered the family"
        );
        let dir = rig.stop();
        let evs = events(&dir);
        let switches: Vec<&(String, serde_json::Value)> =
            evs.iter().filter(|(k, _)| k == "plan.switched").collect();
        assert_eq!(
            switches.len(),
            2,
            "the spill and the recovery, nothing else"
        );
        let back = switches
            .iter()
            .find(|(_, p)| p["to_account"] == "primary")
            .expect("the recovery row");
        assert_eq!(back.1["from_account"], "overflow");
        assert_eq!(back.1["reason"], "primary_recovered");
        assert_eq!(
            back.1["probe"], true,
            "the admitted probe's success IS the transition"
        );
        // The probing request's own trace record: the return trip,
        // labelled by direction with the probe flag set (spec §6) — on
        // the streaming medium, which is this case's whole point.
        let rec = trace_records(&dir)
            .into_iter()
            .find(|r| r["identity"]["session"] == "S2")
            .expect("the probing request's record");
        assert_eq!(rec["decision"]["provider"], "p-plan");
        let ps = &rec["result"]["plan_switch"];
        assert_eq!(ps["from"], "p-api/m1");
        assert_eq!(ps["to"], "p-plan/m1");
        assert_eq!(ps["reason"], "primary_recovered");
        assert_eq!(ps["probe"], true);
    }
}
