//! CONF-64 (spec §6's cooldown row + §8's condition-N record clause, ADR-024
//! ruling 1 / DESIGN §12.8/§12.10.9): **a pre-attempt cooldown skip's
//! `result.failover_from` survives a condition-N refusal, on both media.**
//! The field names the route the cooldown projection abandoned before any
//! attempt, whether the walk then serves, fails or ends with nothing served
//! — the record is a function of the request's history, not of the code path
//! that noticed the refusal.
//!
//! Rig (the ADR-024 measured shape): chain `d/m` (chat, keyed, **in
//! cooldown** — demoted by a real `403 quota_exhausted` + `retry-after: 60`
//! on the rig's first request, no seeded row, no sleep) → `k/m` (keyless) →
//! `w/m` (responses wire). Nothing is attemptable, so both media's requests
//! are condition N. Controls: (a) the demotion request itself is condition E
//! — an attempted 403 with no candidate after it writes **no**
//! `failover_from` (spec §6 row 2, CONF-58 (a), unchanged by ADR-024); (b) a
//! cooling-free condition-N chain (`k/m` keyless head + `w/m` wire) keeps
//! `failover_from` null on both media (CONF-57 (b), unchanged); (c) a
//! cooldown skip never writes a `failover.triggered` event and nothing is
//! charged.

#![forbid(unsafe_code)]

use router_conformance::testkit::{self, CannedResponse};

/// The frozen condition-N sentence, verbatim (spec §8).
const NO_ROUTE_SENTENCE: &str =
    "no available route: every candidate provider is demoted, keyless or unavailable";

/// The demotion trigger (ADR-011 item 4): the classifier's QUOTA_EXHAUSTED
/// wording with a 60s `Retry-After`, so the provider cooldown outlives the
/// test. Fired at `d/m` it demotes provider `d` provider-wide; the request
/// itself is refused 502 (condition E — its own single-candidate resolution
/// has no servable candidate after the attempt).
fn quota_403(retry_after_s: u64) -> CannedResponse {
    CannedResponse::json(
        403,
        "Forbidden",
        br#"{"error":{"message":"You have exceeded your current quota","type":"insufficient_quota","code":"insufficient_quota"}}"#,
    )
    .with_header("retry-after", &retry_after_s.to_string())
}

const MODEL_BLOCK: &str = r#"      - id: m
        context: 128k
        price:
          input_miss: 0.001
          input_hit: 0.0001
          cache_write: 0.0
          output: 0.002
          peak: { multiplier: 1.0, windows: [] }
        source: "mock upstream (no price; test fixture)"
"#;

/// The rig: `d` (chat, keyed) at the chain head, `k` (chat, no key in this
/// process) and `w` (responses wire, keyed) behind it on the fallback chain.
/// The client names `d/m` for the cooling chain, `k/m` for the cooling-free
/// control.
fn config_yaml(d_port: u16, k_port: u16, w_port: u16, listen_port: u16) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: d
    urls:
      chat: http://127.0.0.1:{d_port}/v1/chat/completions
    api_key_env: CONF64_D_KEY
    wire_api: chat
    supports: [chat]
    account: api
    models:
{MODEL_BLOCK}  - name: k
    urls:
      chat: http://127.0.0.1:{k_port}/v1/chat/completions
    api_key_env: CONF64_K_KEY_UNSET
    wire_api: chat
    supports: [chat]
    account: api
    models:
{MODEL_BLOCK}  - name: w
    urls:
      responses: http://127.0.0.1:{w_port}/v1/responses
    api_key_env: CONF64_W_KEY
    wire_api: responses
    supports: [responses]
    account: api
    models:
{MODEL_BLOCK}aliases: {{}}
plugins: []
fallback:
  - k/m
  - w/m"#,
    )
}

/// One rig instance: three mocks, the server, its tempdir.
struct Rig {
    d: testkit::MockUpstream,
    k: testkit::MockUpstream,
    w: testkit::MockUpstream,
    listen_addr: String,
    dir: std::path::PathBuf,
    serve_task: tokio::task::JoinHandle<i32>,
}

async fn rig(tag: &str) -> Rig {
    let d = testkit::MockUpstream::start().await.unwrap();
    let k = testkit::MockUpstream::start().await.unwrap();
    let w = testkit::MockUpstream::start().await.unwrap();
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let dir = testkit::tempdir(tag);
    std::fs::write(
        dir.join("config.yaml"),
        config_yaml(d.addr.port(), k.addr.port(), w.addr.port(), listen_port),
    )
    .unwrap();
    std::env::set_var("CONF64_D_KEY", "sk-d");
    std::env::set_var("CONF64_W_KEY", "sk-w");
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    Rig {
        d,
        k,
        w,
        listen_addr,
        dir,
        serve_task,
    }
}

impl Rig {
    async fn stop(self) -> std::path::PathBuf {
        self.serve_task.abort();
        let _ = self.serve_task.await;
        self.dir
    }
}

/// One request against the rig, parsed: (status, json body).
async fn post(addr: &str, model: &str, session: &str, stream: bool) -> (u16, serde_json::Value) {
    let s = if stream { "true" } else { "false" };
    let body = format!(
        r#"{{"model":"{model}","messages":[{{"role":"user","content":"conf64 {session}"}}],"prompt_cache_key":"{session}","stream":{s}}}"#
    );
    let (status, bytes, _h) =
        testkit::http_post(addr, "/v1/chat/completions", body.as_bytes(), &[]);
    let v: serde_json::Value = serde_json::from_slice(&bytes).expect("refusal json");
    (status, v)
}

fn events(dir: &std::path::Path) -> Vec<(String, serde_json::Value)> {
    let store = router_store::SqliteStore::open(&dir.join("state/router.db")).unwrap();
    use router_core::store::{Query, QueryRow, Store as _};
    let QueryRow::Events(rows) = store.query(Query::AllEvents).unwrap() else {
        panic!("events query");
    };
    rows.into_iter().map(|r| (r.kind_raw, r.payload)).collect()
}

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

fn record_for<'a>(dir: &std::path::Path, session: &str) -> serde_json::Value {
    trace_records(dir)
        .into_iter()
        .find(|r| r["identity"]["session"] == session)
        .unwrap_or_else(|| panic!("the {session} record"))
}

/// The produced case (ADR-024 ruling 1): a condition-N refusal whose chain
/// opened on a cooling route carries the abandoned route in
/// `result.failover_from` — on **both** media. The buffered arm's value is
/// the one the frozen assertion `conf_42_non_primary_abandon_is_failover_only`
/// already fixes; this case witnesses the streaming arm joining it, and that
/// the skip never writes a `failover.triggered` event.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_64_condition_n_record_carries_the_cooling_abandon_on_both_media() {
    let rig = rig("conf64-cooling").await;
    // The real demotion: 403 quota_exhausted + 60s retry-after on `d/m`.
    rig.d.queue(quota_403(60));

    // Condition E, the demotion request itself: `d/m` was attempted and
    // failed with no servable candidate after it.
    let (st_d0, v_d0) = post(&rig.listen_addr, "d/m", "D0", false).await;
    assert_eq!(st_d0, 502);
    assert_eq!(
        v_d0["error"]["details"]["stage"],
        serde_json::Value::Null,
        "condition E: an upstream was contacted, so no stage/skipped members"
    );

    // The produced case, both media: same chain, same body modulo
    // \"stream\": true, nothing attemptable.
    let (st_b, v_b) = post(&rig.listen_addr, "d/m", "B1", false).await;
    let (st_s, v_s) = post(&rig.listen_addr, "d/m", "S1", true).await;
    assert_eq!(st_b, 502);
    assert_eq!(st_s, 502);
    for (medium, v) in [("buffered", &v_b), ("streaming", &v_s)] {
        assert_eq!(v["error"]["message"], NO_ROUTE_SENTENCE, "{medium}");
        assert_eq!(
            v["error"]["details"]["stage"], "no_available_route",
            "{medium}"
        );
        assert_eq!(
            v["error"]["details"]["skipped"].as_array().map(|a| a.len()),
            Some(3),
            "{medium}: one entry per offered candidate (completeness, ADR-023 Decision 3)"
        );
    }

    // Only the demotion request reached a mock; nothing is charged.
    assert_eq!(rig.d.requests().len(), 1);
    assert_eq!(rig.k.requests().len(), 0);
    assert_eq!(rig.w.requests().len(), 0);

    let dir = rig.stop().await;

    // The ruling: both media's condition-N records name the abandoned route.
    for session in ["B1", "S1"] {
        let rec = record_for(&dir, session);
        assert_eq!(rec["result"]["status"], 502, "{session}");
        assert_eq!(
            rec["result"]["failover_from"], "d/m",
            "{session}: the cooldown skip's abandon survives the refusal (ADR-024 ruling 1)"
        );
        assert_eq!(rec["usage_missing"], true, "{session}");
        assert_eq!(rec["cost"]["total"], 0, "{session}: nothing was charged");
    }

    // Control (a): the demotion request's own record is condition E — a
    // failed attempt with no candidate after it writes no failover_from
    // (spec §6 row 2, CONF-58 (a); unchanged by ADR-024).
    let rec_d0 = record_for(&dir, "D0");
    assert_eq!(rec_d0["result"]["failover_from"], serde_json::Value::Null);

    // Control (c): a cooldown skip is not a failover — no event row, ever.
    let evs = events(&dir);
    assert_eq!(
        evs.iter()
            .filter(|(k, _)| k == "failover.triggered")
            .count(),
        0,
        "the skip wrote no failover.triggered row (spec §6's cooldown row)"
    );
}

/// Control (b): a condition-N chain that offered **no** cooling route keeps
/// `failover_from` null on both media (CONF-57 (b), narrowed by ADR-024 to
/// exactly this shape — the cooldown row is the one producer that needs no
/// destination).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_64_control_cooling_free_condition_n_keeps_null_on_both_media() {
    let rig = rig("conf64-control").await;
    // Demote `d` so the rig matches the produced case's state, then never
    // offer it: the control chain is `k/m` (keyless) + `w/m` (wire).
    rig.d.queue(quota_403(60));
    let (st_d0, _v) = post(&rig.listen_addr, "d/m", "D0", false).await;
    assert_eq!(st_d0, 502);

    let (st_b, v_b) = post(&rig.listen_addr, "k/m", "CB", false).await;
    let (st_s, v_s) = post(&rig.listen_addr, "k/m", "CS", true).await;
    assert_eq!(st_b, 502);
    assert_eq!(st_s, 502);
    for (medium, v) in [("buffered", &v_b), ("streaming", &v_s)] {
        assert_eq!(
            v["error"]["details"]["stage"], "no_available_route",
            "{medium}"
        );
        assert_eq!(
            v["error"]["details"]["skipped"].as_array().map(|a| a.len()),
            Some(2),
            "{medium}: two offered candidates, two entries"
        );
    }

    let dir = rig.stop().await;
    for session in ["CB", "CS"] {
        let rec = record_for(&dir, session);
        assert_eq!(
            rec["result"]["failover_from"],
            serde_json::Value::Null,
            "{session}: no cooling route was offered, so nothing was abandoned (CONF-57 (b))"
        );
    }
}
