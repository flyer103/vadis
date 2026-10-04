//! CONF-57 (spec §2 / §4.2 / §8, ADR-022, DESIGN §12.10.9): **a candidate
//! may only be served on its own wire.** The failover walk never crosses the
//! 3×3 matrix: a chat request whose resolved route is keyless walks a chain
//! whose first entry speaks responses and whose second is chat-native — the
//! foreign mock's request log must stay empty while the chat-native
//! candidate serves, and when no native candidate exists anywhere both media
//! get one frozen exhausted shape. All assertions are relations over the
//! rig's own construction (which mock received what, one entry per offered
//! candidate, one reason each) — no snapshot numbers.
//!
//! Rig shape (the shipped roster's own defect shape, R11-F1): three
//! providers — `keyless-chat` (chat wire, no key in env), `foreign`
//! (responses wire, keyed) and `native` (chat wire, keyed) — with
//! `fallback: [foreign/model, native/model]`. A chat request naming the
//! keyless entry's model walks `keyless-chat` → `foreign` → `native`.

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, CannedResponse};

/// A 200 chat completion body the byte assertions can compare against.
const CHAT_OK: &str = r#"{"id":"ok","choices":[{"index":0,"message":{"role":"assistant","content":"served"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}"#;

/// The client's chat bytes, deliberately odd enough that a reserialization
/// would show: nested braces, unicode, a float, a vadis-owned key.
const CLIENT_BODY: &str = r#"{"model":"keyless-chat/m","messages":[{"role":"user","content":"héllo 😀 {b}"}],"temperature":1e-9,"vadis_meta":{"echo":true},"stream":false}"#;
/// The same bytes after exactly the two permitted mutations: `vadis_meta`
/// removed and the `model` value rewritten to the native route's id.
const EXPECTED_UPSTREAM_BODY: &str = r#"{"model":"m","messages":[{"role":"user","content":"héllo 😀 {b}"}],"temperature":1e-9,"stream":false}"#;
/// The streaming form of the same request (the body-level `stream` flag is
/// the path split's only input).
const CLIENT_BODY_STREAM: &str = r#"{"model":"keyless-chat/m","messages":[{"role":"user","content":"héllo 😀 {b}"}],"temperature":1e-9,"vadis_meta":{"echo":true},"stream":true}"#;

/// The rig: three mock upstreams (one per provider) and a config whose
/// fallback chain is `[foreign/m, native/m]`. `with_native: false` drops
/// the native (chat, keyed) entry from the roster and the chain — the
/// "no candidate may serve" shape.
async fn rig(
    tag: &str,
    with_native: bool,
) -> (
    testkit::MockUpstream,
    testkit::MockUpstream,
    testkit::MockUpstream,
    String,
    std::path::PathBuf,
) {
    let keyless = testkit::MockUpstream::start().await.unwrap();
    let foreign = testkit::MockUpstream::start().await.unwrap();
    let native = testkit::MockUpstream::start().await.unwrap();
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let dir = testkit::tempdir(tag);

    // Keys: the foreign and native providers are keyed; `keyless-chat`
    // names an env var this process never sets, so it gets no transport
    // (the keyless skip's precondition).
    std::env::set_var("CONF57_FOREIGN_KEY", "sk-foreign");
    if with_native {
        std::env::set_var("CONF57_NATIVE_KEY", "sk-native");
    }

    let native_port = native.addr.port();
    let native_entry = if with_native {
        format!(
            r#"
  - name: native
    urls:
      chat: http://127.0.0.1:{native_port}/v1/chat/completions
    api_key_env: CONF57_NATIVE_KEY
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
        r#"{head}server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: keyless-chat
    urls:
      chat: http://127.0.0.1:{keyless_port}/v1/chat/completions
    api_key_env: CONF57_KEYLESS_KEY_UNSET
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
    api_key_env: CONF57_FOREIGN_KEY
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
        head = "",
        keyless_port = keyless.addr.port(),
        foreign_port = foreign.addr.port(),
    );
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config).unwrap();
    (keyless, foreign, native, listen_addr, dir)
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
    let store = vadis_store::SqliteStore::open(&dir.join("state/router.db")).unwrap();
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

/// (a) The wire gate: the foreign mock receives **zero** requests while the
/// chat-native candidate serves the request with `protocol_out == chat`,
/// `translated == false`, and upstream bytes equal to the client's modulo
/// the two permitted mutations.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_57_wire_mismatch_is_skipped_native_candidate_serves() {
    let (keyless, foreign, native, listen_addr, dir) = rig("conf57-gate", true).await;
    // The native mock answers; the foreign mock gets nothing queued (an
    // unexpected request would answer 500 "no canned response queued" and
    // the empty-log assertion below would fail loudly).
    native.queue(CannedResponse::json(200, "OK", CHAT_OK.as_bytes()));
    let serve_task = serve(&dir, &listen_addr).await;

    let (status, body, _h) = testkit::http_post(
        &listen_addr,
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[],
    );

    assert_eq!(status, 200, "the chat-native candidate serves the request");
    assert_eq!(
        body,
        CHAT_OK.as_bytes(),
        "the native candidate's response bytes relayed verbatim"
    );
    assert_eq!(
        foreign.requests().len(),
        0,
        "no byte of the client's chat body crosses the matrix: the \
         responses-wire mock received nothing"
    );
    assert_eq!(keyless.requests().len(), 0, "the keyless route was skipped");
    assert_eq!(
        native.requests().len(),
        1,
        "the chat-native candidate answered"
    );

    // Byte fidelity on the serving attempt: the client's own bytes modulo
    // mutations (a) and (b) — the relation CONF-01 asserts for a direct
    // route, here over a walk that skipped two candidates first.
    let seen = &native.requests()[0];
    assert_eq!(seen.path, "/v1/chat/completions");
    assert_eq!(
        seen.body,
        EXPECTED_UPSTREAM_BODY.as_bytes(),
        "the upstream-visible body is the client's bytes minus vadis_meta, \
         with the model value rewritten to the native id"
    );

    // The skip is not an event: no intent row, no classified row, no
    // failover trigger naming the skipped candidates.
    let dir = stop(serve_task, dir).await;
    let evs = events(&dir);
    assert_eq!(
        evs.iter()
            .filter(|(k, _)| k == "upstream.submitted")
            .count(),
        1,
        "exactly one intent row: the candidate that served"
    );
    for (kind, payload) in &evs {
        let text = payload.to_string();
        assert!(
            !(kind == "error.classified" && text.contains("foreign")),
            "the wire skip classifies nothing: {kind} {text}"
        );
        assert!(
            !(kind == "failover.triggered" && text.contains("foreign")),
            "the wire skip triggers no failover: {kind} {text}"
        );
    }

    // The served record tells the wire truth: protocol_out is the inbound
    // protocol, translated is false (no mapper exists in v0.1), and no
    // displacement is narrated for a skip.
    let rec = trace_records(&dir)
        .into_iter()
        .find(|r| r["result"]["status"] == 200)
        .expect("the served request's record");
    assert_eq!(rec["protocol"]["protocol_in"], "chat");
    assert_eq!(
        rec["protocol"]["protocol_out"], "chat",
        "protocol_out names the wire that served — never a foreign wire"
    );
    assert_eq!(
        rec["protocol"]["translated"], false,
        "no mapper ran; translated is the mapper event, not a word comparison"
    );
    assert_eq!(
        rec["result"]["failover_from"],
        serde_json::Value::Null,
        "the keyless/wire skips are not displacements: failover_from stays clear"
    );
    assert_eq!(rec["decision"]["provider"], "native");
    assert_eq!(rec["decision"]["requested_model"], "keyless-chat/m");
}

/// (b) The exhausted walk: with **no** chat-native candidate anywhere in
/// the chain, the client gets the frozen shape on **both** media — 502,
/// `error.type == "upstream_error"`, the frozen sentence verbatim,
/// `details.stage == "no_available_route"`, and `details.skipped[]` holding
/// exactly the candidates the walk refused without attempting, in chain
/// order, each with its own reason — the streaming arm differing by nothing
/// but its pre-existing `"stream": true`. No `upstream.submitted` row,
/// `usage_missing: true`, nothing charged, `failover_from: null`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_57_no_native_candidate_is_the_frozen_exhausted_shape() {
    let (keyless, foreign, _native, listen_addr, dir) = rig("conf57-exhausted", false).await;
    let serve_task = serve(&dir, &listen_addr).await;

    // Both media, same request, same config: the two refusal bodies may
    // differ only in the streaming arm's pre-existing "stream": true.
    let (s_buf, b_buf, _h) = testkit::http_post(
        &listen_addr,
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[],
    );
    let (s_str, b_str, _h) = testkit::http_post(
        &listen_addr,
        "/v1/chat/completions",
        CLIENT_BODY_STREAM.as_bytes(),
        &[],
    );
    assert_eq!(s_buf, 502, "buffered: the walk exhausted");
    assert_eq!(s_str, 502, "streaming: the walk exhausted");

    let v_buf: serde_json::Value = serde_json::from_slice(&b_buf).expect("buffered refusal json");
    let v_str: serde_json::Value = serde_json::from_slice(&b_str).expect("streaming refusal json");

    for (medium, v) in [("buffered", &v_buf), ("streaming", &v_str)] {
        assert_eq!(
            v["error"]["type"], "upstream_error",
            "{medium}: the exhausted walk's error type"
        );
        assert_eq!(
            v["error"]["message"],
            "no available route: every candidate provider is demoted, keyless or unavailable",
            "{medium}: the frozen sentence, verbatim"
        );
        assert_eq!(
            v["error"]["details"]["stage"], "no_available_route",
            "{medium}: the stage"
        );
        // The relation, not a snapshot: skipped[] holds exactly the
        // candidates the rig offered (the resolved route plus the chain),
        // in chain order, one reason each from the frozen set.
        let skipped = v["error"]["details"]["skipped"]
            .as_array()
            .expect("{medium}: skipped[] present");
        assert_eq!(
            skipped.len(),
            2,
            "{medium}: one entry per offered candidate (resolved + fallback)"
        );
        assert_eq!(skipped[0]["route"], "keyless-chat/m");
        assert_eq!(
            skipped[0]["reason"], "keyless",
            "{medium}: the resolved route is keyless in this process"
        );
        assert_eq!(skipped[1]["route"], "foreign/m");
        assert_eq!(
            skipped[1]["reason"], "wire_mismatch",
            "{medium}: the responses-wire entry cannot serve a chat request"
        );
        for entry in skipped {
            let reason = entry["reason"].as_str().expect("reason string");
            assert!(
                ["unknown_provider", "keyless", "wire_mismatch", "demoted"].contains(&reason),
                "{medium}: reason '{reason}' is from the frozen set"
            );
        }
        assert_eq!(
            v["error"]["details"]["upstream_status"],
            serde_json::Value::Null,
            "{medium}: nothing was attempted, so no upstream status exists"
        );
        assert_eq!(
            v["error"]["details"]["error_class"],
            serde_json::Value::Null,
            "{medium}: nothing was classified"
        );
    }
    // The only permitted difference between the two media.
    assert_eq!(v_buf["error"]["details"]["stream"], serde_json::Value::Null);
    assert_eq!(v_str["error"]["details"]["stream"], true);

    // The foreign mock stayed untouched on both media.
    assert_eq!(foreign.requests().len(), 0, "no byte crossed the matrix");
    assert_eq!(keyless.requests().len(), 0, "the keyless route was skipped");

    let dir = stop(serve_task, dir).await;
    let evs = events(&dir);
    assert_eq!(
        evs.iter()
            .filter(|(k, _)| k == "upstream.submitted")
            .count(),
        0,
        "the exhausted walk attempted nothing: no intent row exists"
    );
    for rec in trace_records(&dir) {
        if rec["result"]["status"] != 502 {
            continue;
        }
        assert_eq!(rec["usage_missing"], true, "the terminal failure record");
        assert_eq!(
            rec["cost"]["total"], 0,
            "nothing was charged for a request nothing served"
        );
        assert_eq!(
            rec["result"]["failover_from"],
            serde_json::Value::Null,
            "a skip is not a displacement"
        );
        assert_eq!(
            rec["errors"][0]["kind"], "upstream_error",
            "kind_for_code's mapping of the 502-class code"
        );
        assert_eq!(
            rec["errors"][0]["details"]["stage"], "no_available_route",
            "the record carries the same details object the client saw"
        );
        // Trace truth: protocol_out never names a foreign wire even on the
        // exhausted walk, and translated is the mapper event (no mapper
        // exists in v0.1 → false).
        assert_eq!(rec["protocol"]["protocol_in"], "chat");
        assert_eq!(
            rec["protocol"]["protocol_out"], "chat",
            "the record names the resolved route's provider wire, which the \
             pre-flight guarantees equals the inbound protocol"
        );
        assert_eq!(rec["protocol"]["translated"], false);
        let skipped = rec["errors"][0]["details"]["skipped"]
            .as_array()
            .expect("the record's skipped[]");
        assert_eq!(skipped.len(), 2, "both media's records carry the skips");
    }
}

/// (c) The control: a chain whose every entry is native and keyed serves
/// normally — the rig's own head — so (a)/(b) cannot pass vacuously.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_57_all_native_keyed_chain_serves() {
    // Reuse the full rig but name the native route directly: the resolved
    // route is chat-native and keyed, so the walk never needs a skip.
    let (keyless, foreign, native, listen_addr, dir) = rig("conf57-control", true).await;
    native.queue(CannedResponse::json(200, "OK", CHAT_OK.as_bytes()));
    let serve_task = serve(&dir, &listen_addr).await;

    let body =
        r#"{"model":"native/m","messages":[{"role":"user","content":"direct"}],"stream":false}"#;
    let (status, body_out, _h) =
        testkit::http_post(&listen_addr, "/v1/chat/completions", body.as_bytes(), &[]);
    assert_eq!(
        status, 200,
        "the control serves: a keyed native chain works"
    );
    assert_eq!(body_out, CHAT_OK.as_bytes());
    assert_eq!(native.requests().len(), 1);
    assert_eq!(foreign.requests().len(), 0);
    assert_eq!(keyless.requests().len(), 0);
    let _ = stop(serve_task, dir).await;
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
