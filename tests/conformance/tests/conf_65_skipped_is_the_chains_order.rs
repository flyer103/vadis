//! CONF-65 (spec §8's `skipped[]` order clause, ADR-024 ruling 2 / DESIGN
//! §12.8/§12.10.9): **`skipped[]` is a property of the chain, not of the
//! medium** — in a `no_available_route` refusal the `route` sequence is
//! exactly the offered candidates that were not attempted, in the chain's own
//! order (the resolved route first, then the fallback entries in config
//! order, spec §4.2), and the two media's arrays are equal element for
//! element — so the whole client body differs by `"stream": true` alone.
//!
//! Rig: the exact class of chain a cooldown creates — the head `d/m` is
//! **skipped in-walk** (demoted by a real `403 quota_exhausted` +
//! `retry-after: 60` on the rig's first request, no seeded row, no sleep),
//! the tail `k/m` (keyless) and `w/m` (responses wire) are **skipped at
//! construction time**. A streaming list seeded with the construction-time
//! skips and appended with the in-walk `demoted` entry puts the head last —
//! this case is the witness that it may not (the streaming walk must order
//! its skips by the chain). Completeness is ADR-023 Decision 3 (CONF-59)
//! and is asserted here only as the untouched count half. The control swaps
//! the fallback order in config: the array reports the chain as configured,
//! not a sorted or reason-grouped vocabulary.

#![forbid(unsafe_code)]

use router_conformance::testkit::{self, CannedResponse};

/// The frozen condition-N sentence, verbatim (spec §8).
const NO_ROUTE_SENTENCE: &str =
    "no available route: every candidate provider is demoted, keyless or unavailable";

/// The demotion trigger (ADR-011 item 4): quota-exhausted 403 + 60s
/// `Retry-After`, so provider `d`'s cooldown outlives the test.
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

/// The rig's config: `d` (chat, keyed) at the chain head, `k` (chat,
/// keyless) and `w` (responses wire, keyed) on the fallback chain. The
/// client names `d/m`; the offered chain is therefore `d/m`, `k/m`, `w/m`
/// in that order — `k/m` and `w/m` swapped when `swap_fallback` is set.
fn config_yaml(
    d_port: u16,
    k_port: u16,
    w_port: u16,
    listen_port: u16,
    swap_fallback: bool,
) -> String {
    let fallback = if swap_fallback {
        "  - w/m\n  - k/m"
    } else {
        "  - k/m\n  - w/m"
    };
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: d
    urls:
      chat: http://127.0.0.1:{d_port}/v1/chat/completions
    api_key_env: CONF65_D_KEY
    wire_api: chat
    supports: [chat]
    account: api
    models:
{MODEL_BLOCK}  - name: k
    urls:
      chat: http://127.0.0.1:{k_port}/v1/chat/completions
    api_key_env: CONF65_K_KEY_UNSET
    wire_api: chat
    supports: [chat]
    account: api
    models:
{MODEL_BLOCK}  - name: w
    urls:
      responses: http://127.0.0.1:{w_port}/v1/responses
    api_key_env: CONF65_W_KEY
    wire_api: responses
    supports: [responses]
    account: api
    models:
{MODEL_BLOCK}aliases: {{}}
plugins: []
fallback:
{fallback}"#,
    )
}

/// One rig, both mocks held by the test so the demotion can be queued.
struct Rig {
    d: testkit::MockUpstream,
    k: testkit::MockUpstream,
    w: testkit::MockUpstream,
    listen_addr: String,
    serve_task: tokio::task::JoinHandle<i32>,
}

async fn rig(tag: &str, swap_fallback: bool) -> Rig {
    let d = testkit::MockUpstream::start().await.unwrap();
    let k = testkit::MockUpstream::start().await.unwrap();
    let w = testkit::MockUpstream::start().await.unwrap();
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let dir = testkit::tempdir(tag);
    std::fs::write(
        dir.join("config.yaml"),
        config_yaml(
            d.addr.port(),
            k.addr.port(),
            w.addr.port(),
            listen_port,
            swap_fallback,
        ),
    )
    .unwrap();
    std::env::set_var("CONF65_D_KEY", "sk-d");
    std::env::set_var("CONF65_W_KEY", "sk-w");
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    Rig {
        d,
        k,
        w,
        listen_addr,
        serve_task,
    }
}

impl Rig {
    async fn stop(self) {
        self.serve_task.abort();
        let _ = self.serve_task.await;
    }
}

/// One request, parsed: (status, json body).
async fn post(addr: &str, session: &str, stream: bool) -> (u16, serde_json::Value) {
    let s = if stream { "true" } else { "false" };
    let body = format!(
        r#"{{"model":"d/m","messages":[{{"role":"user","content":"conf65 {session}"}}],"prompt_cache_key":"{session}","stream":{s}}}"#
    );
    let (status, bytes, _h) =
        testkit::http_post(addr, "/v1/chat/completions", body.as_bytes(), &[]);
    let v: serde_json::Value = serde_json::from_slice(&bytes).expect("refusal json");
    (status, v)
}

/// The (route, reason) pairs of `details.skipped[]`, in array order.
fn skipped_pairs(v: &serde_json::Value) -> Vec<(String, String)> {
    v["error"]["details"]["skipped"]
        .as_array()
        .expect("skipped[] present")
        .iter()
        .map(|e| {
            (
                e["route"].as_str().expect("route").to_string(),
                e["reason"].as_str().expect("reason").to_string(),
            )
        })
        .collect()
}

fn pairs<const N: usize>(rows: [(&'static str, &'static str); N]) -> Vec<(String, String)> {
    rows.into_iter()
        .map(|(r, s)| (r.to_string(), s.to_string()))
        .collect()
}

/// The produced case (ADR-024 ruling 2): on the chain a cooldown creates —
/// head skipped in-walk, tail skipped at construction — both media's
/// `skipped[]` is the chain's own order, element-for-element identical, and
/// the whole client body differs by `"stream": true` alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_65_skipped_is_the_chain_order_on_both_media() {
    let rig = rig("conf65-order", false).await;
    // The real demotion: 403 quota_exhausted + 60s retry-after on `d/m`.
    rig.d.queue(quota_403(60));
    let (st_d0, v_d0) = post(&rig.listen_addr, "D0", false).await;
    assert_eq!(st_d0, 502, "the demotion request exhausts its chain");
    assert_eq!(
        v_d0["error"]["details"]["stage"],
        serde_json::Value::Null,
        "condition E: an upstream was contacted"
    );
    assert_eq!(rig.d.requests().len(), 1, "the 403 was served by d's mock");

    // The produced case: the same chain, both media. The bodies differ by
    // \"stream\": true alone, so the skipped arrays must be identical.
    let (st_b, v_b) = post(&rig.listen_addr, "B1", false).await;
    let (st_s, v_s) = post(&rig.listen_addr, "S1", true).await;
    assert_eq!(st_b, 502);
    assert_eq!(st_s, 502);
    let want = pairs([
        ("d/m", "demoted"),
        ("k/m", "keyless"),
        ("w/m", "wire_mismatch"),
    ]);
    for (medium, v) in [("buffered", &v_b), ("streaming", &v_s)] {
        assert_eq!(v["error"]["message"], NO_ROUTE_SENTENCE, "{medium}");
        assert_eq!(
            v["error"]["details"]["stage"], "no_available_route",
            "{medium}"
        );
        assert_eq!(
            skipped_pairs(v),
            want,
            "{medium}: skipped[] is the chain's own order, \
             route+reason element for element (ADR-024 ruling 2)"
        );
    }
    // Element-for-element equality of the two media's arrays — the
    // relation itself, not two coincidences.
    assert_eq!(
        skipped_pairs(&v_b),
        skipped_pairs(&v_s),
        "the two media's skipped[] are equal element for element"
    );
    // The only permitted difference between the two bodies.
    assert_eq!(v_b["error"]["details"]["stream"], serde_json::Value::Null);
    assert_eq!(v_s["error"]["details"]["stream"], true);

    // Only the demotion request reached a mock; the refusals charged nothing.
    assert_eq!(rig.d.requests().len(), 1);
    assert_eq!(rig.k.requests().len(), 0);
    assert_eq!(rig.w.requests().len(), 0);

    rig.stop().await;
}

/// The control (ADR-024's rejected "order by anything other than the
/// chain"): a rig whose fallback order is swapped reports the swapped order
/// — the array carries the chain as configured, not a sorted or
/// reason-grouped vocabulary, on both media.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_65_control_swapped_fallback_reports_the_swapped_chain() {
    let rig = rig("conf65-swap", true).await;
    rig.d.queue(quota_403(60));
    let (st_d0, _v) = post(&rig.listen_addr, "D0", false).await;
    assert_eq!(st_d0, 502);

    let (st_b, v_b) = post(&rig.listen_addr, "B1", false).await;
    let (st_s, v_s) = post(&rig.listen_addr, "S1", true).await;
    assert_eq!(st_b, 502);
    assert_eq!(st_s, 502);
    let want = pairs([
        ("d/m", "demoted"),
        ("w/m", "wire_mismatch"),
        ("k/m", "keyless"),
    ]);
    for (medium, v) in [("buffered", &v_b), ("streaming", &v_s)] {
        assert_eq!(
            skipped_pairs(v),
            want,
            "{medium}: the array reports the chain as configured, not sorted"
        );
    }
    assert_eq!(skipped_pairs(&v_b), skipped_pairs(&v_s));

    rig.stop().await;
}
