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

#![forbid(unsafe_code)]

use router_conformance::testkit::{self, CannedResponse, PlanRig, SseChunk};

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_77_streaming_walk_skips_a_cooling_primary_like_the_buffered_one() {
    let (plan, api, dir, listen_addr) =
        testkit::plan_rig_parts("conf77", "", testkit::PLAN_POLICY_DEFAULT).await;
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
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
    let (s2, _b2, _h2) = rig.post(Some("S1"), 2);
    assert_eq!(s2, 200, "the 403 spills the family to overflow");

    // While the demotion is live: a streaming boundary request from a
    // fresh session must be served by the overflow account — the
    // relay's walk skipped the cooling primary. (Cooldown 0s means the
    // only thing refusing the primary is ADR-011's projection; the
    // live /health answers that same availability question through the
    // same single owner, so its word here is the timing witness.)
    let h = http_get_health(&rig.listen_addr);
    assert_eq!(
        h["plan"]["probe"]["blocked_by"], "primary_cooling_down",
        "fixture invariant: the demotion is still live"
    );
    let key = r#","prompt_cache_key":"S2""#;
    let body = format!(
        r#"{{"model":"p-plan/m1","messages":[{{"role":"user","content":"stream turn 1"}}]{key},"stream":true,"stream_options":{{"include_usage":true}}}}"#
    );
    let (ss, sb, _sh) = testkit::http_post(
        &rig.listen_addr,
        "/v1/chat/completions",
        body.as_bytes(),
        &[],
    );
    assert_eq!(ss, 200, "the streamed request was served");
    assert!(
        testkit::dechunk(&sb).ends_with(b"data: [DONE]\n\n"),
        "the stream relayed to completion"
    );
    assert_eq!(
        rig.plan.requests().len(),
        2,
        "the relay's walk skipped the cooling primary: no third plan hit"
    );
    assert_eq!(
        rig.api.requests().len(),
        2,
        "the streamed turn was served by the overflow account"
    );

    // After the demotion passes, the same streaming shape probes the
    // primary and recovers the family — the skip was the projection's
    // verdict, not a permanent refusal.
    tokio::time::sleep(std::time::Duration::from_millis(1_300)).await;
    let h = http_get_health(&rig.listen_addr);
    assert_eq!(
        h["plan"]["probe"]["blocked_by"],
        serde_json::Value::Null,
        "the demotion has passed"
    );
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
}
