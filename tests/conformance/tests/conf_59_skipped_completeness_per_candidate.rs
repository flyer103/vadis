//! CONF-59 (spec §8, ADR-023 Decision 3·`skipped[]` carries one entry per
//! candidate the chain offered, DESIGN §12.8/§12.10.9): in a
//! `no_available_route` refusal, `|skipped[]|` equals the rig's own
//! offered-candidate count — the relation, not a snapshot.
//!
//! (a) a chain of **two models on one keyless provider** plus a keyed
//! responses-wire entry, nothing attempted: both media's refusal lists
//! **three** entries in chain order — both models of the keyless provider
//! with reason `keyless`, the wire-ineligible one with `wire_mismatch`;
//! (b) the control without the duplicate model lists two, i.e. adding the
//! second model of the same provider changed the list by exactly its own
//! entry;
//! (c) neither mock receives a request, no `upstream.submitted` row
//! exists, `usage_missing: true` and nothing is charged.

#![forbid(unsafe_code)]

use vadis_conformance::testkit;

/// The frozen condition-N sentence, verbatim (spec §8).
const NO_ROUTE_SENTENCE: &str =
    "no available route: every candidate provider is demoted, keyless or unavailable";

/// The rig: `kl` (chat wire, no key in this process) carrying `m1` — and
/// `m2` too when `with_second_model` — plus `foreign` (responses wire,
/// keyed). The client names `kl/m1`; the fallback chain is
/// `[kl/m2 (when present), foreign/m]`. The offered-candidate count is
/// therefore 3 or 2.
async fn rig(
    tag: &str,
    with_second_model: bool,
) -> (
    testkit::MockUpstream,
    testkit::MockUpstream,
    String,
    std::path::PathBuf,
) {
    let kl = testkit::MockUpstream::start().await.unwrap();
    let foreign = testkit::MockUpstream::start().await.unwrap();
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let dir = testkit::tempdir(tag);

    let foreign_key_env = format!("CONF59_FOREIGN_KEY_{tag}");
    std::env::set_var(&foreign_key_env, "sk-foreign");

    let second_model = if with_second_model {
        r#"
      - id: m2
        context: 128k
        price:
          input_miss: 0.001
          input_hit: 0.0001
          cache_write: 0.0
          output: 0.002
          peak: { multiplier: 1.0, windows: [] }
        source: "mock upstream (no price; test fixture)"
"#
    } else {
        ""
    };
    let fallback_yaml = if with_second_model {
        "  - kl/m2\n  - foreign/m\n"
    } else {
        "  - foreign/m\n"
    };
    let config = format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: kl
    urls:
      chat: http://127.0.0.1:{kl_port}/v1/chat/completions
    api_key_env: CONF59_KL_KEY_UNSET
    wire_api: chat
    supports: [chat]
    account: api
    models:
      - id: m1
        context: 128k
        price:
          input_miss: 0.001
          input_hit: 0.0001
          cache_write: 0.0
          output: 0.002
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "mock upstream (no price; test fixture)"{second_model}
  - name: foreign
    urls:
      responses: http://127.0.0.1:{foreign_port}/v1/responses
    api_key_env: {foreign_key_env}
    wire_api: responses
    supports: [responses]
    account: api
    models:
      - id: m
        context: 128k
        price:
          input_miss: 0.001
          input_hit: 0.0001
          cache_write: 0.0
          output: 0.002
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "mock upstream (no price; test fixture)"
aliases: {{}}
plugins: []
fallback:
{fallback_yaml}"#,
        kl_port = kl.addr.port(),
        foreign_port = foreign.addr.port(),
    );
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config).unwrap();
    (kl, foreign, listen_addr, dir)
}

async fn serve(dir: &std::path::PathBuf, listen_addr: &str) -> tokio::task::JoinHandle<i32> {
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let addr = listen_addr.to_string();
    let task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&addr);
    task
}

fn events(dir: &std::path::Path) -> Vec<(String, serde_json::Value)> {
    let store = vadis_store::SqliteStore::open(&dir.join("state/vadis.db")).unwrap();
    use vadis_core::store::{Query, QueryRow, Store as _};
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

async fn stop(
    serve_task: tokio::task::JoinHandle<i32>,
    dir: std::path::PathBuf,
) -> std::path::PathBuf {
    serve_task.abort();
    let _ = serve_task.await;
    dir
}

fn client_body(stream: bool) -> String {
    format!(
        r#"{{"model":"kl/m1","messages":[{{"role":"user","content":"conf59 {s}"}}],"stream":{s}}}"#,
        s = if stream { "true" } else { "false" }
    )
}

/// Both media's refusal for this rig, as (medium, body) pairs.
async fn both_media_refusals(
    listen_addr: &str,
) -> ((u16, serde_json::Value), (u16, serde_json::Value)) {
    let (st_buf, b_buf, _h) = testkit::http_post(
        listen_addr,
        "/v1/chat/completions",
        client_body(false).as_bytes(),
        &[],
    );
    let (st_str, b_str, _h) = testkit::http_post(
        listen_addr,
        "/v1/chat/completions",
        client_body(true).as_bytes(),
        &[],
    );
    let v_buf: serde_json::Value = serde_json::from_slice(&b_buf).expect("buffered refusal json");
    let v_str: serde_json::Value = serde_json::from_slice(&b_str).expect("streaming refusal json");
    ((st_buf, v_buf), (st_str, v_str))
}

/// (a) Three offered candidates → three entries, both media, chain order.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_59_second_model_of_keyless_provider_is_listed() {
    let (kl, foreign, listen_addr, dir) = rig("conf59-three", true).await;
    let serve_task = serve(&dir, &listen_addr).await;
    let ((st_buf, v_buf), (st_str, v_str)) = both_media_refusals(&listen_addr).await;
    assert_eq!(st_buf, 502);
    assert_eq!(st_str, 502);

    for (medium, v) in [("buffered", &v_buf), ("streaming", &v_str)] {
        assert_eq!(v["error"]["type"], "upstream_error", "{medium}");
        assert_eq!(v["error"]["message"], NO_ROUTE_SENTENCE, "{medium}");
        assert_eq!(
            v["error"]["details"]["stage"], "no_available_route",
            "{medium}"
        );
        assert_eq!(
            v["error"]["details"]["upstream_status"],
            serde_json::Value::Null,
            "{medium}: nothing was attempted"
        );
        assert_eq!(
            v["error"]["details"]["error_class"],
            serde_json::Value::Null,
            "{medium}: nothing was classified"
        );
        let skipped = v["error"]["details"]["skipped"]
            .as_array()
            .expect("{medium}: skipped[] present");
        // The relation: |skipped[]| equals the rig's own offered-candidate
        // count (resolved route kl/m1 + fallback kl/m2 + fallback foreign/m).
        assert_eq!(
            skipped.len(),
            3,
            "{medium}: one entry per candidate the chain offered"
        );
        // Chain order, per-candidate reasons: both models of the keyless
        // provider with `keyless`, the foreign-wire entry `wire_mismatch`.
        assert_eq!(skipped[0]["route"], "kl/m1", "{medium}: chain order");
        assert_eq!(skipped[0]["reason"], "keyless", "{medium}");
        assert_eq!(
            skipped[1]["route"], "kl/m2",
            "{medium}: the second model of the keyless provider is listed"
        );
        assert_eq!(
            skipped[1]["reason"], "keyless",
            "{medium}: same reason as the first — the provider-level \
             exclusion may not swallow it"
        );
        assert_eq!(skipped[2]["route"], "foreign/m", "{medium}: chain order");
        assert_eq!(skipped[2]["reason"], "wire_mismatch", "{medium}");
    }
    // The only permitted difference between the two media.
    assert_eq!(v_buf["error"]["details"]["stream"], serde_json::Value::Null);
    assert_eq!(v_str["error"]["details"]["stream"], true);

    // (c) Neither mock received a request; the records are terminal
    // failures with nothing charged and no intent row.
    assert_eq!(kl.requests().len(), 0);
    assert_eq!(foreign.requests().len(), 0);

    let dir = stop(serve_task, dir).await;
    let evs = events(&dir);
    assert_eq!(
        evs.iter()
            .filter(|(k, _)| k == "upstream.submitted")
            .count(),
        0,
        "no upstream was contacted"
    );
    let recs = trace_records(&dir);
    assert_eq!(recs.len(), 2, "one record per request");
    for rec in &recs {
        assert_eq!(rec["result"]["status"], 502);
        assert_eq!(rec["usage_missing"], true);
        assert_eq!(rec["cost"]["total"], 0, "nothing was charged");
        assert_eq!(
            rec["errors"][0]["details"]["skipped"]
                .as_array()
                .expect("the record's skipped[]")
                .len(),
            3,
            "errors[0].details carries the same details the client saw"
        );
    }
}

/// (b) The control without the duplicate model: two offered candidates →
/// two entries — adding the second model of the same provider changed the
/// list by exactly its own entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_59_control_without_the_duplicate_model_lists_two() {
    let (kl, foreign, listen_addr, dir) = rig("conf59-two", false).await;
    let serve_task = serve(&dir, &listen_addr).await;
    let ((st_buf, v_buf), (st_str, v_str)) = both_media_refusals(&listen_addr).await;
    assert_eq!(st_buf, 502);
    assert_eq!(st_str, 502);

    for (medium, v) in [("buffered", &v_buf), ("streaming", &v_str)] {
        assert_eq!(
            v["error"]["details"]["stage"], "no_available_route",
            "{medium}"
        );
        let skipped = v["error"]["details"]["skipped"]
            .as_array()
            .expect("{medium}: skipped[] present");
        assert_eq!(
            skipped.len(),
            2,
            "{medium}: two offered candidates (kl/m1 + foreign/m), two entries"
        );
        assert_eq!(skipped[0]["route"], "kl/m1");
        assert_eq!(skipped[0]["reason"], "keyless");
        assert_eq!(skipped[1]["route"], "foreign/m");
        assert_eq!(skipped[1]["reason"], "wire_mismatch");
    }
    assert_eq!(kl.requests().len(), 0);
    assert_eq!(foreign.requests().len(), 0);
    let _ = stop(serve_task, dir).await;
}
