//! CONF-122 (spec §4.21.2's exclusions, §4.21.4's `spill`-only and per-route
//! keys, §4.21.5's reload rule; ADR-064): **the over-trigger controls — nothing
//! escalates that the contract does not count, and nothing outside a `spill`
//! family moves at all.**
//!
//! Parked: `R76-1` lands the counter, the predicate and the `/health` member.
//! Until then `silent_escalation` is a refused key (`deny_unknown_fields`) and
//! this file must not run.
//!
//! The limb the card names as the red control is **(a)**: an implementation that
//! demotes on one row spills after the first silence, and this case reads
//! `plan_state.account == "overflow"` where the contract says the family never
//! moved. The rest are the other ways a counter can be wrong: a window that
//! never closes, a predicate that counts everything, a key that belongs to the
//! route it was measured on, a mode that forbids spending, and a reload that
//! re-zeroes state it does not own.
//!
//! No network egress: loopback mocks only; `plugins: []`.
#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde_json::Value;
use vadis_conformance::testkit::{self, SseChunk};
use vadis_core::store::{Query, QueryRow, Store as _};

const KEY_ENV: &str = "CONF122_KEY";

/// The family, with §4.21's knobs written explicitly so each limb can retune
/// one of them. `block` is the refusal limb.
const POLICY_SPILL: &str = "  family: m1
  primary: p-plan/m1
  overflow: p-api/m1
  on_primary_exhausted: spill
  recover: probe
  cooldown: 0s
  silent_escalation:
    enabled: true
    threshold: 2
    window: 5m";

const POLICY_BLOCK: &str = "  family: m1
  primary: p-plan/m1
  overflow: p-api/m1
  on_primary_exhausted: block
  recover: probe
  cooldown: 0s
  silent_escalation:
    threshold: 2
    window: 5m";

/// The plan-first pair with a family policy. `server` is a parameter because
/// limb (c) needs the **outer** bound nearer than the gap bound, and the other
/// limbs need the opposite.
fn config_yaml(
    _listen_port: u16,
    plan_port: u16,
    api_port: u16,
    server: &str,
    policy: &str,
) -> String {
    format!(
        r#"server:   {{ {server} }}
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

/// The same roster with **no** `plan_policy` — the no-plan configuration
/// (spec §4.6: writing neither spelling is legal, and there is then no family).
fn config_yaml_without_policy(
    listen_port: u16,
    p1_port: u16,
    p2_port: u16,
    server: &str,
) -> String {
    let full = config_yaml(listen_port, p1_port, p2_port, server, POLICY_SPILL);
    let head = full
        .split("\nplan_policy:")
        .next()
        .expect("the policy block is last")
        .to_string();
    head.replace("fallback: []", "fallback: [\"p-api/m1\"]")
}

/// The rig's windows: the gap bound is nearer than the outer bound, so a
/// stalling body trips `idle_timeout` (a counted fact).
fn server_idle(listen_port: u16) -> String {
    format!(
        r#"addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 1s, request_timeout: 8s, stream_idle_timeout: 400ms"#
    )
}

/// Limb (c)'s windows: the **outer** bound is nearer, so a stalling body trips
/// `request_timeout` (not a counted fact).
fn server_deadline_first(listen_port: u16) -> String {
    format!(
        r#"addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 1s, request_timeout: 600ms, stream_idle_timeout: 3s"#
    )
}

fn stream_body(model: &str, session: &str) -> String {
    format!(
        r#"{{"model":"{model}","messages":[{{"role":"user","content":"hi"}}],"stream":true,"prompt_cache_key":"{session}"}}"#
    )
}

fn buffered_body(model: &str, session: &str) -> String {
    format!(
        r#"{{"model":"{model}","messages":[{{"role":"user","content":"hi"}}],"prompt_cache_key":"{session}"}}"#
    )
}

/// A head + one event, then silence with the socket held open — the counted S1
/// fact, on whichever route it is queued for.
fn silence() -> testkit::CannedResponse {
    testkit::CannedResponse::sse(vec![
        SseChunk::event(b"data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\n"),
        SseChunk::event_after(b"data: never\n\n", 60_000),
    ])
}

/// An SSE body that **ends** after one event, no terminal event — the clean
/// early close (`upstream_ended_early`), which must never count.
fn ended_early() -> testkit::CannedResponse {
    testkit::CannedResponse::sse(vec![SseChunk::event(
        b"data: {\"choices\":[{\"delta\":{\"content\":\"cut\"}}]}\n\n",
    )])
}

/// A clean, byte-complete stream.
fn clean() -> testkit::CannedResponse {
    testkit::CannedResponse::sse(vec![
        SseChunk::event(b"data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n"),
        SseChunk::event(b"data: [DONE]\n\n"),
    ])
}

/// A raw listener that answers a head plus a PARTIAL body, then stalls — the
/// **buffered** path's mid-body stall, which carries no `details.stream` and
/// must never be counted (CONF-105's fixture).
async fn midbody_stall_listener() -> u16 {
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 8192];
                let _ = s.read(&mut buf).await;
                let head = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 100\r\nconnection: close\r\n\r\n";
                if s.write_all(head.as_bytes()).await.is_err() {
                    return;
                }
                if s.write_all(b"{\"partial\":").await.is_err() {
                    return;
                }
                // Hold the connection open: the body read never finishes.
                tokio::time::sleep(Duration::from_secs(5)).await;
            });
        }
    });
    port
}

/// `GET /health`, parsed.
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

fn read_events(dir: &std::path::Path) -> Vec<(String, Value, i64)> {
    let store = vadis_store::SqliteStore::open(&dir.join("state/vadis.db")).unwrap();
    let QueryRow::Events(rows) = store.query(Query::AllEvents).unwrap() else {
        panic!("events query");
    };
    rows.into_iter()
        .map(|r| (r.kind_raw, r.payload, r.ts_us))
        .collect()
}

fn plan_state(dir: &std::path::Path) -> Option<(String, String)> {
    let store = vadis_store::SqliteStore::open(&dir.join("state/vadis.db")).unwrap();
    match store.query(Query::PlanState { family: "m1" }).unwrap() {
        QueryRow::PlanState(row) => row.map(|r| (r.account, r.route)),
        other => panic!("plan_state query: {other:?}"),
    }
}

fn switch_count(events: &[(String, Value, i64)]) -> usize {
    events
        .iter()
        .filter(|(k, _, _)| k == "plan.switched")
        .count()
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
    std::env::set_var(KEY_ENV, "sk-conf122");
    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move {
        let _ = vadis_cli::serve(&cfg).await;
    });
    testkit::wait_listening(&format!("127.0.0.1:{listen_port}"));
    serve_task
}

/// One streaming request, read to EOF.
fn post(addr: &str, body: &str) -> (u16, Vec<u8>, Vec<(String, String)>) {
    testkit::http_post(addr, "/v1/chat/completions", body.as_bytes(), &[])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "CONF-122: depends on R76-1"]
async fn conf_122_silent_escalation_refuses_to_over_trigger() {
    // ------------------------------------------------------------------- (a)
    // One fact never escalates — **the over-trigger control**.
    let dir = testkit::tempdir("conf122a");
    let plan = testkit::MockUpstream::start().await.unwrap();
    let api = testkit::MockUpstream::start().await.unwrap();
    plan.queue(silence());
    plan.queue(clean());
    for _ in 0..2 {
        api.queue(clean());
    }
    let listen_port = testkit::free_port();
    let addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(
        &config_path,
        config_yaml(
            listen_port,
            plan.addr.port(),
            api.addr.port(),
            &server_idle(listen_port),
            POLICY_SPILL,
        ),
    )
    .unwrap();
    let serve_task = start_vadis(&config_path, listen_port).await;

    let sess = "conf122-a";
    let (s1, _b1, _h1) = post(&addr, &stream_body("p-plan/m1", sess));
    assert_eq!(s1, 200, "the truncated head was committed");
    let (s2, _b2, _h2) = post(&addr, &stream_body("p-plan/m1", sess));
    assert_eq!(s2, 200, "the second request is served from the plan route");
    assert_eq!(plan.requests().len(), 2, "both requests reached the plan");
    assert_eq!(api.requests().len(), 0, "nothing spilled");
    assert!(
        plan_state(&dir).is_none(),
        "one fact is not an account fact: no state row exists"
    );
    assert_eq!(switch_count(&read_events(&dir)), 0, "no plan.switched row");
    assert_eq!(
        health(&addr)["plan"]["escalation"]["count"],
        1,
        "it WAS counted — the rule just did not fire (an implementation that \
         does not count cannot pass CONF-121 (a) either)"
    );
    serve_task.abort();
    let _ = serve_task.await;

    // ------------------------------------------------------------------- (b)
    // Facts outside W never accumulate.
    let dir_b = testkit::tempdir("conf122b");
    let plan_b = testkit::MockUpstream::start().await.unwrap();
    let api_b = testkit::MockUpstream::start().await.unwrap();
    for _ in 0..2 {
        plan_b.queue(silence());
        api_b.queue(clean());
    }
    let listen_b = testkit::free_port();
    let addr_b = format!("127.0.0.1:{listen_b}");
    let cfg_b = dir_b.join("config.yaml");
    let policy_b = POLICY_SPILL.replace("window: 5m", "window: 1s");
    std::fs::write(
        &cfg_b,
        config_yaml(
            listen_b,
            plan_b.addr.port(),
            api_b.addr.port(),
            &server_idle(listen_b),
            &policy_b,
        ),
    )
    .unwrap();
    let serve_b = start_vadis(&cfg_b, listen_b).await;
    let sess_b = "conf122-b";
    let (bs1, _bb1, _bh1) = post(&addr_b, &stream_body("p-plan/m1", sess_b));
    assert_eq!(bs1, 200);
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    let (bs2, _bb2, _bh2) = post(&addr_b, &stream_body("p-plan/m1", sess_b));
    assert_eq!(bs2, 200);
    assert_eq!(
        health(&addr_b)["plan"]["escalation"]["count"],
        1,
        "1.5 s > W = 1s, so the second fact starts a fresh run"
    );
    assert_eq!(
        switch_count(&read_events(&dir_b)),
        0,
        "two runs of 1, not one run of 2"
    );
    assert!(plan_state(&dir_b).is_none(), "the family never moved");
    serve_b.abort();
    let _ = serve_b.await;

    // ------------------------------------------------------------------- (c)
    // The causes that must not count: a clean early close, our own deadline, a
    // client's departure, and a buffered mid-body stall.
    let dir_c = testkit::tempdir("conf122c");
    let plan_c = testkit::MockUpstream::start().await.unwrap();
    let api_c = testkit::MockUpstream::start().await.unwrap();
    plan_c.queue(ended_early()); //  upstream_ended_early
    plan_c.queue(silence()); //      request_timeout (the outer bound is nearer)
    plan_c.queue(silence()); //      client_closed (the reader leaves)
    for _ in 0..4 {
        api_c.queue(clean());
    }
    let listen_c = testkit::free_port();
    let addr_c = format!("127.0.0.1:{listen_c}");
    let cfg_c = dir_c.join("config.yaml");
    std::fs::write(
        &cfg_c,
        config_yaml(
            listen_c,
            plan_c.addr.port(),
            api_c.addr.port(),
            &server_deadline_first(listen_c),
            POLICY_SPILL,
        ),
    )
    .unwrap();
    let serve_c = start_vadis(&cfg_c, listen_c).await;
    let sess_c = "conf122-c";
    let (cs1, _cb1, _ch1) = post(&addr_c, &stream_body("p-plan/m1", sess_c));
    assert_eq!(cs1, 200, "a clean early close is still a 200 head");
    let (cs2, _cb2, _ch2) = post(&addr_c, &stream_body("p-plan/m1", sess_c));
    assert_eq!(cs2, 200, "our own deadline is still a 200 head");
    let (cs3, _cb3, _ch3) = testkit::http_post_with_opts(
        &addr_c,
        "/v1/chat/completions",
        stream_body("p-plan/m1", sess_c).as_bytes(),
        &[],
        testkit::ReadOpts {
            stop_after_body_bytes: Some(1),
        },
    );
    assert_eq!(cs3, 200, "the head was read before the abandonment");
    assert!(
        plan_c.peer_aborts() >= 1,
        "the mock saw the reader leave — the client_closed fixture is real"
    );

    // The buffered path's mid-body stall, on a roster of its own.
    let stall_port = midbody_stall_listener().await;
    let dir_c2 = testkit::tempdir("conf122c-buffered");
    let listen_c2 = testkit::free_port();
    let addr_c2 = format!("127.0.0.1:{listen_c2}");
    let cfg_c2 = dir_c2.join("config.yaml");
    std::fs::write(
        &cfg_c2,
        config_yaml(
            listen_c2,
            stall_port,
            api_c.addr.port(),
            &format!(
                r#"addr: "127.0.0.1:{listen_c2}", upstream_attempt_timeout: 1s, request_timeout: 1500ms, stream_idle_timeout: 3s"#
            ),
            POLICY_SPILL,
        ),
    )
    .unwrap();
    let serve_c2 = start_vadis(&cfg_c2, listen_c2).await;
    let (bs, _bb, _bh) = testkit::http_post(
        &addr_c2,
        "/v1/chat/completions",
        buffered_body("p-plan/m1", "conf122-c2").as_bytes(),
        &[],
    );
    assert_eq!(bs, 502, "the buffered mid-body stall's carriage");
    serve_c.abort();
    let _ = serve_c.await;
    serve_c2.abort();
    let _ = serve_c2.await;

    let records_c = read_records(&dir_c.join("state/traces"));
    let causes: Vec<String> = records_c
        .iter()
        .filter_map(|r| {
            r["errors"][0]["details"]["stream_end"]
                .as_str()
                .map(str::to_owned)
        })
        .collect();
    for expected in ["upstream_ended_early", "request_timeout", "client_closed"] {
        assert!(
            causes.contains(&expected.to_string()),
            "the rig really produced `{expected}`: {records_c:?}"
        );
    }
    for r in &records_c {
        assert_ne!(
            r["errors"][0]["details"]["stream_end"], "idle_timeout",
            "no streaming rig here produced a counted fact: {r:?}"
        );
    }
    assert_eq!(
        switch_count(&read_events(&dir_c)),
        0,
        "three non-counted causes on the plan route, zero escalations"
    );
    assert_eq!(
        read_records(&dir_c2.join("state/traces")).len(),
        1,
        "the buffered leg wrote its one record"
    );
    assert!(plan_state(&dir_c).is_none(), "the family never moved");

    // ------------------------------------------------------------------- (d)
    // A metered route's facts never reach a plan route's run.
    let dir_d = testkit::tempdir("conf122d");
    let plan_d = testkit::MockUpstream::start().await.unwrap();
    let api_d = testkit::MockUpstream::start().await.unwrap();
    plan_d.queue(silence());
    plan_d.queue(silence());
    for _ in 0..3 {
        api_d.queue(silence());
    }
    let listen_d = testkit::free_port();
    let addr_d = format!("127.0.0.1:{listen_d}");
    let cfg_d = dir_d.join("config.yaml");
    std::fs::write(
        &cfg_d,
        config_yaml(
            listen_d,
            plan_d.addr.port(),
            api_d.addr.port(),
            &server_idle(listen_d),
            POLICY_SPILL,
        ),
    )
    .unwrap();
    let serve_d = start_vadis(&cfg_d, listen_d).await;
    let sess_d = "conf122-d";
    let _ = post(&addr_d, &stream_body("p-plan/m1", sess_d));
    let _ = post(&addr_d, &stream_body("p-plan/m1", sess_d));
    let (account_d, route_d) = plan_state(&dir_d).expect("the spill wrote the state");
    assert_eq!(account_d, "overflow");
    assert_eq!(route_d, "p-api/m1");
    let switches_before = switch_count(&read_events(&dir_d));
    assert_eq!(switches_before, 1);
    // Two stalls on the METERED route: recorded against p-api/m1, and inert.
    let _ = post(&addr_d, &stream_body("p-plan/m1", sess_d));
    let _ = post(&addr_d, &stream_body("p-plan/m1", sess_d));
    let events_d = read_events(&dir_d);
    let metered_facts = events_d
        .iter()
        .filter(|(k, p, _)| {
            k == "error.classified"
                && p["stream_end"] == "idle_timeout"
                && p["route"]["provider"] == "p-api"
        })
        .count();
    assert_eq!(
        metered_facts, 2,
        "the metered route's facts are recorded against it: {events_d:?}"
    );
    assert_eq!(
        switch_count(&events_d),
        switches_before,
        "and they escalate nothing: a metered route has no plan tier to leave"
    );
    assert_eq!(
        health(&addr_d)["plan"]["escalation"]["count"],
        0,
        "the family's active route is metered, so nothing would escalate"
    );
    serve_d.abort();
    let _ = serve_d.await;

    // ------------------------------------------------------------------- (e)
    // `block` never escalates; a no-policy roster has no family at all.
    let dir_e = testkit::tempdir("conf122e");
    let plan_e = testkit::MockUpstream::start().await.unwrap();
    let api_e = testkit::MockUpstream::start().await.unwrap();
    for _ in 0..2 {
        plan_e.queue(silence());
        api_e.queue(clean());
    }
    let listen_e = testkit::free_port();
    let addr_e = format!("127.0.0.1:{listen_e}");
    let cfg_e = dir_e.join("config.yaml");
    std::fs::write(
        &cfg_e,
        config_yaml(
            listen_e,
            plan_e.addr.port(),
            api_e.addr.port(),
            &server_idle(listen_e),
            POLICY_BLOCK,
        ),
    )
    .unwrap();
    let serve_e = start_vadis(&cfg_e, listen_e).await;
    let sess_e = "conf122-e";
    let (e1, _eb1, _eh1) = post(&addr_e, &stream_body("p-plan/m1", sess_e));
    let (e2, _eb2, _eh2) = post(&addr_e, &stream_body("p-plan/m1", sess_e));
    assert_eq!(
        e1, 200,
        "the attempt's own outcome, not a fabricated refusal"
    );
    assert_eq!(e2, 200);
    assert_eq!(
        switch_count(&read_events(&dir_e)),
        0,
        "`block` means `do not spend`: there is no destination to move to"
    );
    assert!(
        plan_state(&dir_e).is_none(),
        "and no state move — a silent plan is not a declared exhaustion"
    );
    assert_eq!(
        api_e.requests().len(),
        0,
        "nothing was served from the metered account under `block`"
    );
    serve_e.abort();
    let _ = serve_e.await;

    let dir_f = testkit::tempdir("conf122f");
    let p1 = testkit::MockUpstream::start().await.unwrap();
    let p2 = testkit::MockUpstream::start().await.unwrap();
    for _ in 0..2 {
        p1.queue(silence());
        p2.queue(clean());
    }
    let listen_f = testkit::free_port();
    let addr_f = format!("127.0.0.1:{listen_f}");
    let cfg_f = dir_f.join("config.yaml");
    std::fs::write(
        &cfg_f,
        config_yaml_without_policy(
            listen_f,
            p1.addr.port(),
            p2.addr.port(),
            &server_idle(listen_f),
        ),
    )
    .unwrap();
    let serve_f = start_vadis(&cfg_f, listen_f).await;
    for _ in 0..2 {
        let _ = post(&addr_f, &stream_body("p-plan/m1", "conf122-f"));
    }
    let health_f = health(&addr_f);
    assert_eq!(
        health_f["plan"]["configured"], false,
        "no policy ⇒ no family, and no invented state: {health_f:?}"
    );
    assert!(
        health_f["plan"].get("escalation").is_none(),
        "and therefore no escalation member: {health_f:?}"
    );
    assert!(
        plan_state(&dir_f).is_none(),
        "nothing account-level moves outside a plan family"
    );
    assert_eq!(switch_count(&read_events(&dir_f)), 0);
    serve_f.abort();
    let _ = serve_f.await;

    // ------------------------------------------------------------------- (f)
    // A reload does not zero the run, and does not escalate by itself.
    let dir_g = testkit::tempdir("conf122g");
    let plan_g = testkit::MockUpstream::start().await.unwrap();
    let api_g = testkit::MockUpstream::start().await.unwrap();
    for _ in 0..3 {
        plan_g.queue(silence());
        api_g.queue(clean());
    }
    let listen_g = testkit::free_port();
    let addr_g = format!("127.0.0.1:{listen_g}");
    let cfg_g = dir_g.join("config.yaml");
    std::fs::write(
        &cfg_g,
        config_yaml(
            listen_g,
            plan_g.addr.port(),
            api_g.addr.port(),
            &server_idle(listen_g),
            POLICY_SPILL,
        ),
    )
    .unwrap();
    let serve_g = start_vadis(&cfg_g, listen_g).await;
    let sess_g = "conf122-g";
    let _ = post(&addr_g, &stream_body("p-plan/m1", sess_g));
    let digest_before = health(&addr_g)["config"]["config_digest"]
        .as_str()
        .expect("a digest")
        .to_string();
    // Raise K to 3 in the file: the run must survive, and the new K must judge
    // the facts that arrive after it — never the ones already counted.
    let raised_policy = POLICY_SPILL.replace("threshold: 2", "threshold: 3");
    std::fs::write(
        &cfg_g,
        config_yaml(
            listen_g,
            plan_g.addr.port(),
            api_g.addr.port(),
            &server_idle(listen_g),
            &raised_policy,
        ),
    )
    .unwrap();
    testkit::wait_for_digest_move(&addr_g, &digest_before, "the reload applied");
    assert_eq!(
        health(&addr_g)["plan"]["escalation"]["count"],
        1,
        "the reload did not zero the run it does not own"
    );
    assert_eq!(
        health(&addr_g)["plan"]["escalation"]["threshold"],
        3,
        "and the new K is what the next fact is judged against"
    );
    let _ = post(&addr_g, &stream_body("p-plan/m1", sess_g));
    assert_eq!(
        health(&addr_g)["plan"]["escalation"]["count"],
        2,
        "the second fact advanced the same run"
    );
    assert_eq!(
        switch_count(&read_events(&dir_g)),
        0,
        "K is now 3, so two facts move nothing — and the reload itself moved nothing"
    );
    let _ = post(&addr_g, &stream_body("p-plan/m1", sess_g));
    let events_g = read_events(&dir_g);
    let switches_g: Vec<&Value> = events_g
        .iter()
        .filter(|(k, _, _)| k == "plan.switched")
        .map(|(_, p, _)| p)
        .collect();
    assert_eq!(switches_g.len(), 1, "the third fact completes K = 3");
    assert_eq!(switches_g[0]["reason"], "primary_silent");
    assert_eq!(switches_g[0]["silence"]["count"], 3);
    assert_eq!(
        switches_g[0]["silence"]["threshold"], 3,
        "the evidence rides the row with the K that decided it"
    );
    serve_g.abort();
    let _ = serve_g.await;
}
