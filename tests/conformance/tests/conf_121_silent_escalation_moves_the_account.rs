//! CONF-121 (spec §4.21; ADR-064; spec §4.6's two movers; §6's `plan_switch`
//! producer table; §9.1's `escalation` member): **two counted silence facts
//! inside the window spill the family — with the reason word the contract
//! names, without retrying the request that produced the second fact, and
//! without losing the run across a restart.**
//!
//! Parked: `R76-1` lands the counter, the predicate, the cause word and the
//! `/health` member. Until then the key `silent_escalation` is refused by the
//! loader (`deny_unknown_fields`) and this file must not run.
//!
//! Landings asserted, never inferred. Every value is read the way a stranger
//! would read it after `serve` stopped — the trace JSONL, the event log
//! through `Query::AllEvents`, the `plan_state` projection through
//! `Query::PlanState`, `/health` over the loopback port — plus the two mocks'
//! own request counts, which are what prove nothing was retried.
//!
//! Phases (one test function, so the run is one window of one process):
//! - (a) **two silence facts spill the family.** `p-plan` answers a head plus
//!   one SSE event and then holds the socket open, twice; the second fact
//!   completes a run of K = 2 inside W = 5 m and the family moves. One
//!   `plan.switched` row, `reason: "primary_silent"`, `probe: false`, and the
//!   `silence: {count, threshold, window_us}` evidence that decided it.
//!   **Neither** request is retried (`p-plan` saw 2, `p-api` 0) and both
//!   records carry the truncation with `plan_switch: null`.
//! - (b) **the next request is displaced, and says why.** It is served by
//!   `p-api`, its `result.plan_switch.reason` is `primary_silent` and its
//!   `result.failover_from` is `null` — nothing failed in that request; the
//!   state moved it.
//! - (c) **the head that never arrived counts.** A raw listener that accepts,
//!   reads and never answers: two such attempts leave two `error.classified`
//!   rows (`reason: "timeout"`, `stage: "unknown_outcome"`, `stream: true`, a
//!   `route` member) and one `primary_silent` switch.
//! - (d) **the return trip resets the run.** A session's first request is
//!   admitted as a probe, succeeds, writes `primary_recovered`, and
//!   `/health`'s `escalation.count` reads 0; one further silence afterwards
//!   leaves the family on `primary` with a count of 1 (a fresh run).
//! - (e) **the run survives a restart.** With that run at 1, `serve` is
//!   stopped and started again on the same state directory; the next silence
//!   completes a run of 2 and spills the family again. An in-memory counter
//!   would have zeroed here.
//!
//! Red at the base: the config is refused (the key does not exist). Red for an
//! implementation that spills on one fact, retries the truncated request, labels
//! the move `primary_exhausted`, drops the head-timeout half, leaves the run
//! alive across the return, or keeps the count in memory.
//!
//! No network egress: loopback mocks only; `plugins: []`.
#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde_json::Value;
use vadis_conformance::testkit::{self, SseChunk};
use vadis_core::store::{Query, QueryRow, Store as _};

const KEY_ENV: &str = "CONF121_KEY";

/// The family's policy: spill, probe, `cooldown: 0s` (the testkit's
/// convention — it removes "the cooldown has not elapsed" as an alternative
/// explanation wherever a request must or must not probe), and §4.21's knobs
/// at their contracted defaults.
const POLICY: &str = "  family: m1
  primary: p-plan/m1
  overflow: p-api/m1
  on_primary_exhausted: spill
  recover: probe
  cooldown: 0s
  silent_escalation:
    enabled: true
    threshold: 2
    window: 5m";

/// The plan-first pair, the rig's two windows and the policy. The windows are
/// sub-second/millisecond values so no phase rests on machine speed: the idle
/// bound cuts a silent stream in ~400 ms, the attempt bound fires in ~1 s.
fn config_yaml(listen_port: u16, plan_port: u16, api_port: u16, policy: &str) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 1s, request_timeout: 8s, stream_idle_timeout: 400ms }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: p-plan
    urls:
      chat: http://127.0.0.1:{plan_port}/v1/chat/completions
    api_key_env: {KEY_ENV}
    wire_api: chat
    supports: [chat]
    account: coding_plan
    models:
      - id: m1
        context: 128k
        price:
          input_miss: 0.001
          input_hit: 0.0001
          cache_write: 0.0
          output: 0.002
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "mock upstream (no price; test fixture)"
  - name: p-api
    urls:
      chat: http://127.0.0.1:{api_port}/v1/chat/completions
    api_key_env: {KEY_ENV}
    wire_api: chat
    supports: [chat]
    account: api
    models:
      - id: m1
        context: 128k
        price:
          input_miss: 0.002
          input_hit: 0.0002
          cache_write: 0.0
          output: 0.004
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "mock upstream (no price; test fixture)"

aliases: {{}}
plugins: []
fallback: []

plan_policy:
{policy}
"#
    )
}

fn stream_body(session: &str) -> String {
    format!(
        r#"{{"model":"p-plan/m1","messages":[{{"role":"user","content":"hi"}}],"stream":true,"prompt_cache_key":"{session}"}}"#
    )
}

/// A head + one event, then silence with the socket held open — the S1 fact.
fn silence() -> testkit::CannedResponse {
    testkit::CannedResponse::sse(vec![
        SseChunk::event(b"data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\n"),
        SseChunk::event_after(b"data: never\n\n", 60_000),
    ])
}

/// A clean, byte-complete stream.
fn clean() -> testkit::CannedResponse {
    testkit::CannedResponse::sse(vec![
        SseChunk::event(b"data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n"),
        SseChunk::event(b"data: [DONE]\n\n"),
    ])
}

/// A raw listener that accepts, reads the request and never answers — the S2
/// fact (the testkit's canned responses cannot delay a head). Returns the port
/// and the connection count.
async fn slow_head_listener() -> (u16, std::sync::Arc<std::sync::Mutex<u32>>) {
    use tokio::io::AsyncReadExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(0u32));
    let seen_clone = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            *seen_clone.lock().unwrap() += 1;
            tokio::spawn(async move {
                let mut buf = [0u8; 8192];
                loop {
                    match tokio::time::timeout(Duration::from_secs(5), s.read(&mut buf)).await {
                        Ok(Ok(0)) | Err(_) | Ok(Err(_)) => return,
                        Ok(Ok(_)) => {}
                    }
                }
            });
        }
    });
    (port, seen)
}

/// `GET /health`, parsed — the section a stranger reads.
fn health(addr: &str) -> Value {
    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let req = format!("GET /health HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    let body_start = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("health head")
        + 4;
    serde_json::from_slice(&buf[body_start..]).expect("health body is json")
}

/// A readable snapshot of the run's store. `serve` holds the writer's
/// EXCLUSIVE lock (ADR-009 item 8), so a second open of the live file
/// is refused `Locked` — the rig copies the db **with its `-wal`/`-shm`
/// siblings** (the projection lives in the WAL) and opens the copy.
/// The same rows, the same values; no limb's assertion is touched.
fn snapshot_store(dir: &std::path::Path) -> vadis_store::SqliteStore {
    let db = dir.join("state/vadis.db");
    let snap = dir.join(format!("snapshot-{}", std::process::id()));
    std::fs::create_dir_all(&snap).unwrap();
    for ext in ["", "-wal", "-shm"] {
        let from = db.with_file_name(format!("vadis.db{ext}"));
        let to = snap.join(format!("vadis.db{ext}"));
        if from.exists() {
            std::fs::copy(&from, &to).unwrap();
        }
    }
    vadis_store::SqliteStore::open(&snap.join("vadis.db")).unwrap()
}

/// Every stored event as (kind_raw, payload, ts_us), read after `serve` stopped.
fn read_events(dir: &std::path::Path) -> Vec<(String, Value, i64)> {
    let store = snapshot_store(dir);
    let QueryRow::Events(rows) = store.query(Query::AllEvents).unwrap() else {
        panic!("events query");
    };
    rows.into_iter()
        .map(|r| (r.kind_raw, r.payload, r.ts_us))
        .collect()
}

/// The family's account state, if any row exists (absent ⇒ `primary`).
fn plan_state(dir: &std::path::Path) -> Option<(String, String)> {
    let store = snapshot_store(dir);
    match store.query(Query::PlanState { family: "m1" }).unwrap() {
        QueryRow::PlanState(row) => row.map(|r| (r.account, r.route)),
        other => panic!("plan_state query: {other:?}"),
    }
}

/// Every `plan.switched` row's payload, in event order.
fn switches(events: &[(String, Value, i64)]) -> Vec<Value> {
    events
        .iter()
        .filter(|(k, _, _)| k == "plan.switched")
        .map(|(_, p, _)| p.clone())
        .collect()
}

fn read_records(trace_dir: &std::path::Path) -> Vec<Value> {
    let mut records = Vec::new();
    for entry in std::fs::read_dir(trace_dir).expect("trace dir exists") {
        let entry = entry.unwrap();
        if entry.path().extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        for line in std::fs::read_to_string(entry.path()).unwrap().lines() {
            records.push(serde_json::from_str(line).expect("record json"));
        }
    }
    records
}

async fn start_vadis(
    config_path: &std::path::Path,
    listen_port: u16,
) -> tokio::task::JoinHandle<()> {
    std::env::set_var(KEY_ENV, "sk-conf121");
    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move {
        let _ = vadis_cli::serve(&cfg).await;
    });
    testkit::wait_listening(&format!("127.0.0.1:{listen_port}"));
    serve_task
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_121_silent_escalation_moves_the_account() {
    // ---------------------------------------------------------------- (a)+(b)
    let dir = testkit::tempdir("conf121ab");
    let plan = testkit::MockUpstream::start().await.unwrap();
    let api = testkit::MockUpstream::start().await.unwrap();
    // p-plan: silence, silence, (probe) clean, silence, silence again after the
    // restart. p-api: every spill-served request.
    plan.queue(silence());
    plan.queue(silence());
    plan.queue(clean());
    plan.queue(silence());
    plan.queue(silence());
    for _ in 0..4 {
        api.queue(clean());
    }

    let listen_port = testkit::free_port();
    let addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(
        &config_path,
        config_yaml(listen_port, plan.addr.port(), api.addr.port(), POLICY),
    )
    .unwrap();
    let serve_task = start_vadis(&config_path, listen_port).await;

    // One session for (a)+(b): its later turns are **not** session boundaries,
    // so no probe is admitted inside the spill.
    let sess = "conf121-sess";
    let (s1, _b1, _h1) = testkit::http_post(
        &addr,
        "/v1/chat/completions",
        stream_body(sess).as_bytes(),
        &[],
    );
    assert_eq!(s1, 200, "the head was committed (a partial body follows)");
    let (s2, _b2, _h2) = testkit::http_post(
        &addr,
        "/v1/chat/completions",
        stream_body(sess).as_bytes(),
        &[],
    );
    assert_eq!(
        s2, 200,
        "the second fact's request is a truncation, not a refusal"
    );
    assert_eq!(
        plan.requests().len(),
        2,
        "neither truncated request was retried inside its own request"
    );
    assert_eq!(
        api.requests().len(),
        0,
        "the metered route had not been attempted when the family moved"
    );

    let events = read_events(&dir);
    let moved = switches(&events);
    assert_eq!(
        moved.len(),
        1,
        "exactly one plan.switched row for the escalation: {moved:?}"
    );
    assert_eq!(
        moved[0]["reason"], "primary_silent",
        "ADR-064 §2.4.3's cause word"
    );
    assert_eq!(moved[0]["probe"], false, "an escalation is not a probe");
    assert_eq!(moved[0]["from_route"], "p-plan/m1");
    assert_eq!(
        moved[0]["to_route"], "p-api/m1",
        "a singleton tier spills to its overflow"
    );
    assert_eq!(
        moved[0]["silence"]["count"], 2,
        "the run's count at the decision"
    );
    assert_eq!(moved[0]["silence"]["threshold"], 2, "the K in force");
    assert_eq!(
        moved[0]["silence"]["window_us"], 300_000_000_i64,
        "the W in force, in microseconds"
    );
    let (account, route) = plan_state(&dir).expect("the escalation writes the projection");
    assert_eq!(account, "overflow");
    assert_eq!(route, "p-api/m1");

    let records = read_records(&dir.join("state/traces"));
    assert_eq!(records.len(), 2, "two requests, two records");
    for r in &records {
        assert_eq!(
            r["errors"][0]["details"]["error_class"], "stream_truncated",
            "the fact is the truncation: {r:?}"
        );
        assert!(
            r["result"]["plan_switch"].is_null(),
            "nothing displaced either request: {r:?}"
        );
    }
    assert_eq!(
        health(&addr)["plan"]["escalation"]["count"],
        0,
        "a spilled family's active route is metered: nothing would escalate"
    );

    // (b) the next turn of the same session is displaced by the state.
    let (s3, _b3, _h3) = testkit::http_post(
        &addr,
        "/v1/chat/completions",
        stream_body(sess).as_bytes(),
        &[],
    );
    assert_eq!(s3, 200);
    assert_eq!(api.requests().len(), 1, "the metered route served it");
    assert_eq!(
        plan.requests().len(),
        2,
        "and the plan route was not attempted"
    );
    let records = read_records(&dir.join("state/traces"));
    let r3 = records
        .iter()
        .find(|r| r["decision"]["provider"] == "p-api")
        .expect("the displaced request's record");
    assert_eq!(
        r3["result"]["plan_switch"]["reason"], "primary_silent",
        "the trace's word equals the state's cause: {r3:?}"
    );
    assert_eq!(r3["result"]["plan_switch"]["from"], "p-plan/m1");
    assert_eq!(r3["result"]["plan_switch"]["to"], "p-api/m1");
    assert!(
        r3["result"]["failover_from"].is_null(),
        "nothing failed in the displaced request"
    );

    // ------------------------------------------------------------------- (d)
    // A new session's first request is a session boundary, so the probe is
    // admitted (cooldown 0s); it succeeds and the family returns.
    let probe_sess = "conf121-probe";
    let (s4, _b4, _h4) = testkit::http_post(
        &addr,
        "/v1/chat/completions",
        stream_body(probe_sess).as_bytes(),
        &[],
    );
    assert_eq!(s4, 200);
    assert_eq!(
        plan.requests().len(),
        3,
        "the probe attempted the plan route"
    );
    let events = read_events(&dir);
    let moved = switches(&events);
    assert_eq!(moved.len(), 2, "the return trip is its own row: {moved:?}");
    assert_eq!(moved[1]["reason"], "primary_recovered");
    let (account, route) = plan_state(&dir).expect("state row");
    assert_eq!(account, "primary");
    assert_eq!(route, "p-plan/m1");
    assert_eq!(
        health(&addr)["plan"]["escalation"]["count"],
        0,
        "the return trip clears the run (ADR-064 §2.5.4)"
    );

    // One further silence: a FRESH run of 1. The family must not re-spill —
    // this is the loop §2.5.4 exists to close.
    let (s5, _b5, _h5) = testkit::http_post(
        &addr,
        "/v1/chat/completions",
        stream_body(probe_sess).as_bytes(),
        &[],
    );
    assert_eq!(s5, 200);
    assert_eq!(
        plan.requests().len(),
        4,
        "the returned family serves from the plan again"
    );
    assert_eq!(
        health(&addr)["plan"]["escalation"]["count"],
        1,
        "a swallowed read: one counted fact, no move"
    );
    assert_eq!(
        switches(&read_events(&dir)).len(),
        2,
        "no third switch: the single stall did not re-spill the family"
    );

    // ------------------------------------------------------------------- (e)
    // The run (count 1) must survive a restart: stop serve, start it again on
    // the same state directory, then send the second fact.
    serve_task.abort();
    let _ = serve_task.await;
    let serve_task = start_vadis(&config_path, listen_port).await;
    let (s6, _b6, _h6) = testkit::http_post(
        &addr,
        "/v1/chat/completions",
        stream_body(probe_sess).as_bytes(),
        &[],
    );
    assert_eq!(s6, 200);
    let events = read_events(&dir);
    let moved = switches(&events);
    assert_eq!(
        moved.len(),
        3,
        "the restart did not zero the run: its second fact completes K: {moved:?}"
    );
    assert_eq!(moved[2]["reason"], "primary_silent");
    assert_eq!(moved[2]["silence"]["count"], 2);
    serve_task.abort();
    let _ = serve_task.await;

    // ------------------------------------------------------------------- (c)
    // The head that never arrives is the second counted fact.
    let dir2 = testkit::tempdir("conf121c");
    let (slow_port, slow_seen) = slow_head_listener().await;
    let api2 = testkit::MockUpstream::start().await.unwrap();
    for _ in 0..2 {
        api2.queue(clean());
    }
    let listen_port2 = testkit::free_port();
    let addr2 = format!("127.0.0.1:{listen_port2}");
    let config_path2 = dir2.join("config.yaml");
    // A short attempt bound: the head window, not the idle one, is what trips.
    let mut cfg2 = config_yaml(listen_port2, slow_port, api2.addr.port(), POLICY);
    cfg2 = cfg2.replace(
        "upstream_attempt_timeout: 1s",
        "upstream_attempt_timeout: 400ms",
    );
    std::fs::write(&config_path2, cfg2).unwrap();
    let serve_task2 = start_vadis(&config_path2, listen_port2).await;

    let sess2 = "conf121-head";
    let (h1, _hb1, _hh1) = testkit::http_post(
        &addr2,
        "/v1/chat/completions",
        stream_body(sess2).as_bytes(),
        &[],
    );
    assert_eq!(h1, 502, "the head bound's conservative refusal");
    let (h2, _hb2, _hh2) = testkit::http_post(
        &addr2,
        "/v1/chat/completions",
        stream_body(sess2).as_bytes(),
        &[],
    );
    assert_eq!(h2, 502);
    assert_eq!(
        *slow_seen.lock().unwrap(),
        2,
        "two attempts reached the silent upstream — and neither was retried"
    );
    let events2 = read_events(&dir2);
    let timeouts: Vec<&Value> = events2
        .iter()
        .filter(|(k, p, _)| {
            k == "error.classified"
                && p["reason"] == "timeout"
                && p["stream"] == true
                && p["stage"] == "unknown_outcome"
        })
        .map(|(_, p, _)| p)
        .collect();
    assert_eq!(
        timeouts.len(),
        2,
        "the log finally answers `how many head timeouts hit this route`: {events2:?}"
    );
    for p in &timeouts {
        assert_eq!(
            p["route"]["provider"], "p-plan",
            "the counted fact names its route"
        );
        assert_eq!(
            p["route"]["model"], "m1",
            "the native model id, never an alias"
        );
    }
    let moved2 = switches(&events2);
    assert_eq!(
        moved2.len(),
        1,
        "two head timeouts within W spill the family: {moved2:?}"
    );
    assert_eq!(moved2[0]["reason"], "primary_silent");
    assert_eq!(moved2[0]["silence"]["count"], 2);
    let (account2, route2) = plan_state(&dir2).expect("state row");
    assert_eq!(account2, "overflow");
    assert_eq!(route2, "p-api/m1");
    serve_task2.abort();
    let _ = serve_task2.await;
}
