//! CONF-58 (spec §4.2 / §6 / §8, ADR-023 Decision 1·the walk's refusal is
//! decided by **whether anything was attempted**, DESIGN §12.8/§12.10.9):
//! the two refusal conditions, on both media, red first.
//!
//! (a)+(b) a keyed chat head whose mock answers 500 with a fallback whose
//! only entry is responses-wire (keyed) — an attempt was classified and
//! nothing served after it, so **both** media get the attempt-exhausted
//! body: 502, `error.type == "upstream_error"`, the class-based sentence,
//! `details.upstream_status == 500`, `details.error_class` == the head's
//! own class, and **neither** `details.stage` **nor** `details.skipped[]`;
//! the streaming arm differs by nothing but its pre-existing
//! `"stream": true`. The records carry `result.failover_from: null`,
//! exactly one `upstream.submitted` row per request and **no**
//! `failover.triggered` row, with `errors[0].details` equal to what the
//! client saw.
//! (c) the control with a native keyed entry appended: the walk serves
//! 200, `failover_from` names the failed head and `failover.triggered.to`
//! names the native entry — never the wire-ineligible one (the narration
//! predicate).
//! (d) the other condition in the same case's rig: the same shape with an
//! all-ineligible chain (the head keyless) still returns the frozen
//! `no_available_route` body with `upstream_status` / `error_class`
//! `null` on both media — the discriminant itself, asserted.
//!
//! All assertions are relations over the rig's own construction — no
//! snapshot numbers beyond the rig's own canned 500.

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, CannedResponse};

/// The class a bare 500 classifies as (vadis-core's `classify_status`):
/// the head's own class the sentence must name.
const HEAD_CLASS: &str = "server_error";

/// The attempt-exhausted sentence for the head's own class (spec §8's
/// first limb, the class-based variant).
const EXHAUSTED_SENTENCE: &str =
    "upstream error (server_error) and the fallback chain is exhausted";
/// The frozen condition-N sentence, verbatim (spec §8).
const NO_ROUTE_SENTENCE: &str =
    "no available route: every candidate provider is demoted, keyless or unavailable";

const CHAT_OK: &str = r#"{"id":"ok","choices":[{"index":0,"message":{"role":"assistant","content":"served"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}"#;

/// One SSE body the relay must carry verbatim (two framed events).
const SSE_EVENTS: [&[u8]; 2] = [
    b"data: {\"id\":\"s\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}]}\n\n",
    b"data: [DONE]\n\n",
];

/// The rig's one shape knob: which tail the chain carries and whether the
/// head is keyed. Unique env-var names per tag (the serve assembly runs
/// in-process, so tests sharing a binary share the environment).
enum Shape {
    /// (a)/(b): keyed head, wire-ineligible keyed tail, no native.
    MixedTail,
    /// (c): MixedTail + a keyed chat-native entry appended to the chain.
    NativeTail,
    /// (d): keyless head, wire-ineligible keyed tail.
    KeylessHead,
}

async fn rig(
    tag: &str,
    shape: Shape,
) -> (
    testkit::MockUpstream,
    testkit::MockUpstream,
    testkit::MockUpstream,
    String,
    std::path::PathBuf,
) {
    let head = testkit::MockUpstream::start().await.unwrap();
    let foreign = testkit::MockUpstream::start().await.unwrap();
    let native = testkit::MockUpstream::start().await.unwrap();
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let dir = testkit::tempdir(tag);

    // Tag-unique env names: tests in one binary run concurrently in one
    // process, so (d)'s never-set head key must not collide with (a)'s
    // set one.
    let head_key_env = format!("CONF58_HEAD_KEY_{tag}");
    let foreign_key_env = format!("CONF58_FOREIGN_KEY_{tag}");
    let native_key_env = format!("CONF58_NATIVE_KEY_{tag}");
    if !matches!(shape, Shape::KeylessHead) {
        std::env::set_var(&head_key_env, "sk-head");
    }
    std::env::set_var(&foreign_key_env, "sk-foreign");
    if matches!(shape, Shape::NativeTail) {
        std::env::set_var(&native_key_env, "sk-native");
    }

    let with_native = matches!(shape, Shape::NativeTail);
    let native_port = native.addr.port();
    let native_entry = if with_native {
        format!(
            r#"
  - name: native
    urls:
      chat: http://127.0.0.1:{native_port}/v1/chat/completions
    api_key_env: {native_key_env}
    wire_api: chat
    supports: [chat]
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
"#
        )
    } else {
        String::new()
    };
    let fallback_yaml = if with_native {
        "  - foreign/m\n  - native/m\n"
    } else {
        "  - foreign/m\n"
    };
    let config = format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: head
    urls:
      chat: http://127.0.0.1:{head_port}/v1/chat/completions
    api_key_env: {head_key_env}
    wire_api: chat
    supports: [chat]
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
        source: "mock upstream (no price; test fixture)"{native_entry}
aliases: {{}}
plugins: []
fallback:
{fallback_yaml}"#,
        head_port = head.addr.port(),
        foreign_port = foreign.addr.port(),
    );
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config).unwrap();
    (head, foreign, native, listen_addr, dir)
}

/// Spawns the real serve assembly on the rig's config and waits for it.
async fn serve(dir: &std::path::PathBuf, listen_addr: &str) -> tokio::task::JoinHandle<i32> {
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let addr = listen_addr.to_string();
    let task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&addr);
    task
}

/// Every stored event as (kind_raw, payload), read after the server stopped.
fn events(dir: &std::path::Path) -> Vec<(String, serde_json::Value)> {
    let store = vadis_store::SqliteStore::open(&dir.join("state/vadis.db")).unwrap();
    use vadis_core::store::{Query, QueryRow, Store as _};
    let QueryRow::Events(rows) = store.query(Query::AllEvents).unwrap() else {
        panic!("events query");
    };
    rows.into_iter().map(|r| (r.kind_raw, r.payload)).collect()
}

/// Every trace record, in write order.
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

/// Aborts serve; the state directory survives for the reads.
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
        r#"{{"model":"head/m","messages":[{{"role":"user","content":"conf58 {s}"}}],"stream":{s}}}"#,
        s = if stream { "true" } else { "false" }
    )
}

/// (a)+(b) The mixed tail: an attempt classified (the head's 500) and
/// nothing served after it — the attempt-exhausted body on **both** media.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_58_attempted_head_ineligible_tail_is_attempt_exhausted_on_both_media() {
    let (head, foreign, native, listen_addr, dir) = rig("conf58-mixed", Shape::MixedTail).await;
    // The head mock answers 500 (the last canned response repeats, so both
    // media's requests get it); nothing is queued on the other mocks, so a
    // stray attempt would answer a loud 500 and fail the log assertions.
    head.queue(CannedResponse::json(
        500,
        "Internal Server Error",
        b"{\"error\":{\"message\":\"boom\"}}",
    ));
    let serve_task = serve(&dir, &listen_addr).await;

    let (st_buf, b_buf, _h) = testkit::http_post(
        &listen_addr,
        "/v1/chat/completions",
        client_body(false).as_bytes(),
        &[],
    );
    let (st_str, b_str, _h) = testkit::http_post(
        &listen_addr,
        "/v1/chat/completions",
        client_body(true).as_bytes(),
        &[],
    );
    assert_eq!(st_buf, 502, "buffered: the walk exhausted");
    assert_eq!(st_str, 502, "streaming: the walk exhausted");

    let v_buf: serde_json::Value = serde_json::from_slice(&b_buf).expect("buffered refusal json");
    let v_str: serde_json::Value = serde_json::from_slice(&b_str).expect("streaming refusal json");

    for (medium, v) in [("buffered", &v_buf), ("streaming", &v_str)] {
        assert_eq!(
            v["error"]["type"], "upstream_error",
            "{medium}: the exhausted walk's error type"
        );
        assert_eq!(
            v["error"]["message"], EXHAUSTED_SENTENCE,
            "{medium}: the class-based sentence for the head's own class"
        );
        assert_eq!(
            v["error"]["details"]["upstream_status"], 500,
            "{medium}: the mock's status"
        );
        assert_eq!(
            v["error"]["details"]["error_class"], HEAD_CLASS,
            "{medium}: the head's own class"
        );
        // The members that would claim no upstream was contacted.
        assert_eq!(
            v["error"]["details"]["stage"],
            serde_json::Value::Null,
            "{medium}: condition E carries no stage"
        );
        assert_eq!(
            v["error"]["details"]["skipped"],
            serde_json::Value::Null,
            "{medium}: condition E carries no skipped[]"
        );
    }
    // The only permitted difference between the two media.
    assert_eq!(v_buf["error"]["details"]["stream"], serde_json::Value::Null);
    assert_eq!(v_str["error"]["details"]["stream"], true);

    // The wire-ineligible tail was never contacted, on either medium.
    assert_eq!(foreign.requests().len(), 0, "no byte crossed the matrix");
    assert_eq!(native.requests().len(), 0);
    assert_eq!(head.requests().len(), 2, "one attempt per medium");

    // (b) The records: no displacement narrated, one intent per request,
    // no failover row, and errors[0].details equals what the client saw.
    let dir = stop(serve_task, dir).await;
    let evs = events(&dir);
    assert_eq!(
        evs.iter()
            .filter(|(k, _)| k == "upstream.submitted")
            .count(),
        2,
        "exactly one intent row per request: the head, and only the head"
    );
    assert_eq!(
        evs.iter()
            .filter(|(k, _)| k == "failover.triggered")
            .count(),
        0,
        "no destination existed, so no displacement is narrated"
    );
    let recs = trace_records(&dir);
    assert_eq!(recs.len(), 2, "one record per request");
    for rec in &recs {
        assert_eq!(rec["result"]["status"], 502);
        assert_eq!(
            rec["result"]["failover_from"],
            serde_json::Value::Null,
            "failover_from names a destination that never existed: null"
        );
        assert_eq!(rec["usage_missing"], true, "the terminal failure record");
        assert_eq!(rec["cost"]["total"], 0, "nothing charged");
        assert_eq!(rec["errors"][0]["kind"], "upstream_error");
        // errors[0].details is the same details object the client saw —
        // whichever medium carried the request, stage/skipped stay absent.
        assert_eq!(
            rec["errors"][0]["details"]["stage"],
            serde_json::Value::Null
        );
        assert_eq!(
            rec["errors"][0]["details"]["skipped"],
            serde_json::Value::Null
        );
        assert_eq!(
            rec["errors"][0]["details"]["error_class"], HEAD_CLASS,
            "the record carries the same details the client received"
        );
    }
}

/// (c) The control: a native keyed entry appended to the chain serves on
/// both media, and the narration names the failed head and the native
/// entry — never the wire-ineligible one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_58_native_tail_control_serves_and_narrates_the_native_destination() {
    let (head, foreign, native, listen_addr, dir) = rig("conf58-native", Shape::NativeTail).await;
    head.queue(CannedResponse::json(
        500,
        "Internal Server Error",
        b"{\"error\":{\"message\":\"boom\"}}",
    ));
    // Queue order is request order: the buffered arm's attempt gets the
    // JSON 200, the streaming arm's attempt gets the SSE 200 (the last
    // canned response repeats, so the head's 500 serves both of its).
    native.queue(CannedResponse::json(200, "OK", CHAT_OK.as_bytes()));
    native.queue(CannedResponse::sse(vec![
        testkit::SseChunk::event(SSE_EVENTS[0]),
        testkit::SseChunk::event(SSE_EVENTS[1]),
    ]));
    let serve_task = serve(&dir, &listen_addr).await;

    let (st_buf, b_buf, _h) = testkit::http_post(
        &listen_addr,
        "/v1/chat/completions",
        client_body(false).as_bytes(),
        &[],
    );
    assert_eq!(st_buf, 200, "buffered: the native tail serves");
    assert_eq!(b_buf, CHAT_OK.as_bytes(), "the native response relayed");

    let (st_str, b_str, _h) = testkit::http_post(
        &listen_addr,
        "/v1/chat/completions",
        client_body(true).as_bytes(),
        &[],
    );
    assert_eq!(st_str, 200, "streaming: the native tail serves");
    let expected_sse: Vec<u8> = SSE_EVENTS.concat();
    assert_eq!(
        testkit::dechunk(&b_str),
        expected_sse,
        "the SSE events relayed verbatim"
    );

    // The narration predicate: the foreign mock is untouched, the walk
    // went head → native.
    assert_eq!(foreign.requests().len(), 0, "no byte crossed the matrix");
    assert_eq!(head.requests().len(), 2);
    assert_eq!(native.requests().len(), 2);

    let dir = stop(serve_task, dir).await;
    let evs = events(&dir);
    let failovers: Vec<&serde_json::Value> = evs
        .iter()
        .filter(|(k, _)| k == "failover.triggered")
        .map(|(_, p)| p)
        .collect();
    assert_eq!(failovers.len(), 2, "one failover row per request");
    for p in &failovers {
        assert_eq!(p["from"], "head/m", "the failed head is named");
        assert_eq!(
            p["to"], "native/m",
            "the destination is the candidate the walk will attempt"
        );
        assert_ne!(p["to"], "foreign/m", "never the wire-ineligible one");
    }
    for rec in trace_records(&dir) {
        assert_eq!(rec["result"]["status"], 200);
        assert_eq!(
            rec["result"]["failover_from"], "head/m",
            "failover_from names the failed head"
        );
    }
}

/// (d) The discriminant: the same rig with an all-ineligible chain (the
/// head keyless) returns the frozen condition-N body on both media.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_58_keyless_head_still_gets_the_frozen_no_available_route_body() {
    let (head, foreign, _native, listen_addr, dir) =
        rig("conf58-keyless", Shape::KeylessHead).await;
    let serve_task = serve(&dir, &listen_addr).await;

    let (st_buf, b_buf, _h) = testkit::http_post(
        &listen_addr,
        "/v1/chat/completions",
        client_body(false).as_bytes(),
        &[],
    );
    let (st_str, b_str, _h) = testkit::http_post(
        &listen_addr,
        "/v1/chat/completions",
        client_body(true).as_bytes(),
        &[],
    );
    assert_eq!(st_buf, 502);
    assert_eq!(st_str, 502);

    let v_buf: serde_json::Value = serde_json::from_slice(&b_buf).expect("buffered refusal json");
    let v_str: serde_json::Value = serde_json::from_slice(&b_str).expect("streaming refusal json");
    for (medium, v) in [("buffered", &v_buf), ("streaming", &v_str)] {
        assert_eq!(v["error"]["type"], "upstream_error", "{medium}");
        assert_eq!(v["error"]["message"], NO_ROUTE_SENTENCE, "{medium}: frozen");
        assert_eq!(
            v["error"]["details"]["stage"], "no_available_route",
            "{medium}: condition N"
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
        assert_eq!(
            skipped.len(),
            2,
            "{medium}: one entry per offered candidate"
        );
        assert_eq!(skipped[0]["route"], "head/m");
        assert_eq!(skipped[0]["reason"], "keyless");
        assert_eq!(skipped[1]["route"], "foreign/m");
        assert_eq!(skipped[1]["reason"], "wire_mismatch");
    }
    assert_eq!(v_buf["error"]["details"]["stream"], serde_json::Value::Null);
    assert_eq!(v_str["error"]["details"]["stream"], true);
    assert_eq!(head.requests().len(), 0);
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
}
