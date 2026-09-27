//! CONF-89 (DESIGN §12.8, spec §4.17/§6/§7, ADR-042 §12.3): **a hit is the
//! recorded bytes** — after one miss, a byte-identical repeat inside the
//! same session returns the recorded response verbatim with the upstream
//! seeing **one** request; the hit's record carries the `cache` group and
//! refuses to be counted; the ledger reconciles; the key separates what it
//! must; and the store is fail-closed.
//!
//! Limbs (ADR-042 §12.3), one test per arm:
//! - (a) buffered: byte equality + the record's shape + the window's
//!   reconciliation (limbs 1–4);
//! - (b) streaming: the same over SSE (28 of the 34 frozen-corpus items
//!   stream — the capability is not buffered-only, ADR-042 §13);
//! - (c) the key separates session / transform mode / one body byte
//!   (limb 5; the `config_digest` component is separated at the owner
//!   level, with the store's bounds);
//! - (d) a non-`2xx` response is never stored (limb 6);
//! - (e) an incomplete stream is never stored (limb 6);
//! - (f) a request with no session is neither looked up nor stored
//!   (limb 6 — no `session: null` shared bucket);
//! - (g) the owner level: the frozen bounds and the `config_digest` key
//!   component, asserted against the one owner directly.
//!
//! **Red at the round's base** (no store exists, so no hit can happen):
//! the rigs below mount `builtin/response_cache` with `enabled: true`,
//! which the base's config validation refuses — the recorded red run is
//! `autowork/harness/r51-1/conf-89-red-at-base.log`.
//!
//! No network egress: a loopback mock upstream only.

#![forbid(unsafe_code)]

use router_conformance::testkit::{self, CannedResponse, SseChunk};
use serde_json::Value;

/// The canned buffered 200: a chat completion with usage (100 in / 20
/// cached / 5 out), so the miss is an ordinary measured record and the
/// window's Σ usage has a known value.
const CANNED: &[u8] = br#"{"id":"ok","choices":[{"index":0,"message":{"role":"assistant","content":"cached-answer"},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":5,"total_tokens":105,"prompt_tokens_details":{"cached_tokens":20}}}"#;

/// The buffered request, posted byte-identically inside one session.
const BODY_BUFFERED: &[u8] = br#"{"model":"mock/glm","messages":[{"role":"user","content":"repeat me"}],"stream":false,"prompt_cache_key":"sess-89"}"#;

/// The streamed twin (same content, the streaming medium + the usage ask).
const BODY_STREAMED: &[u8] = br#"{"model":"mock/glm","messages":[{"role":"user","content":"repeat me"}],"stream":true,"stream_options":{"include_usage":true},"prompt_cache_key":"sess-89"}"#;

/// One body byte different from `BODY_BUFFERED` (the content's last
/// letter): limb 5's "changing one body byte is a miss".
const BODY_ONE_BYTE_OFF: &[u8] = br#"{"model":"mock/glm","messages":[{"role":"user","content":"repeat mf"}],"stream":false,"prompt_cache_key":"sess-89"}"#;

/// The SSE byte sequence the upstream streams (and the client must
/// receive, verbatim, on both the miss and the hit) — usage included.
const SSE_EVENTS: &[&[u8]] = &[
    b"data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\n",
    b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":5,\"total_tokens\":105,\"prompt_tokens_details\":{\"cached_tokens\":80}}}\n\n",
    b"data: [DONE]\n\n",
];

fn sse_with_usage() -> Vec<SseChunk> {
    SSE_EVENTS.iter().map(|e| SseChunk::event(e)).collect()
}

fn sse_bytes() -> Vec<u8> {
    SSE_EVENTS.concat()
}

fn config_yaml(upstream_port: u16, listen_port: u16) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: mock
    urls:
      chat: http://127.0.0.1:{upstream_port}/v1/chat/completions
    api_key_env: CONF89_MOCK_KEY
    wire_api: chat
    supports: [chat]
    models:
      - id: glm
        context: 128k
        price:
          input_miss: 0.001
          input_hit: 0.0001
          cache_write: 0.0
          output: 0.002
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "mock upstream (no price; test fixture)"

aliases: {{}}
plugins:
  - id: response-cache
    kind: builtin/response_cache
    config: {{ enabled: true }}
fallback: []
"#,
    )
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

struct Rig {
    upstream: testkit::MockUpstream,
    listen_addr: String,
    dir: std::path::PathBuf,
    serve_task: tokio::task::JoinHandle<i32>,
}

impl Rig {
    /// A cache-ON rig (`config.enabled: true`) with the canned response
    /// already queued.
    async fn start(tag: &str, canned: CannedResponse) -> Rig {
        let dir = testkit::tempdir(tag);
        let upstream = testkit::MockUpstream::start().await.unwrap();
        upstream.queue(canned);
        let listen_port = testkit::free_port();
        let listen_addr = format!("127.0.0.1:{listen_port}");
        let config_path = dir.join("config.yaml");
        std::fs::write(&config_path, config_yaml(upstream.addr.port(), listen_port)).unwrap();
        std::env::set_var("CONF89_MOCK_KEY", "sk-conf89");
        let cfg = config_path.to_string_lossy().into_owned();
        let serve_task = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
        testkit::wait_listening(&listen_addr);
        Rig {
            upstream,
            listen_addr,
            dir,
            serve_task,
        }
    }

    fn post(&self, body: &[u8], extra_headers: &[(&str, &str)]) -> (u16, Vec<u8>) {
        let (status, body, _) = testkit::http_post(
            &self.listen_addr,
            "/v1/chat/completions",
            body,
            extra_headers,
        );
        (status, body)
    }

    /// Aborts serve, then reads the window's records.
    fn records(self) -> Vec<Value> {
        self.serve_task.abort();
        read_records(&self.dir.join("state/traces"))
    }
}

/// Assert the whole of a hit record's shape (ADR-042 §12.3 limbs 2–3,
/// spec §6): the `cache` group naming its source, and the
/// refuses-to-be-counted fields.
fn assert_hit_record(hit: &Value, miss: &Value, replayed_len: usize) {
    // The group is present and names the record the bytes came from.
    let cache = &hit["cache"];
    assert!(cache.is_object(), "a hit carries the cache group: {hit}");
    assert_eq!(
        cache["verdict"], "inferred",
        "the label is inferred, always"
    );
    assert_eq!(
        cache["replayed"]["request_id"], miss["identity"]["request_id"],
        "the reference is the miss's own id"
    );
    assert_eq!(
        cache["replayed"]["session"], hit["identity"]["session"],
        "the reference is inside the session"
    );
    assert_eq!(
        cache["replayed"]["turn_index"], miss["identity"]["turn_index"],
        "the reference carries the source record's own turn index"
    );
    assert_eq!(
        cache["replayed_bytes"], replayed_len as u64,
        "a count of bytes returned, never a price and never a saving"
    );
    let key_digest = cache["key_digest"]
        .as_str()
        .expect("key_digest is a string");
    assert_eq!(key_digest.len(), 64, "a sha256 hex digest: {key_digest}");
    assert!(
        key_digest
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "lowercase hex: {key_digest}"
    );
    // The record refuses to be counted (spec §6's three properties).
    assert_eq!(hit["usage_missing"], true, "no usage was measured");
    assert_eq!(hit["usage"]["input_total"], 0);
    assert_eq!(hit["usage"]["output"], 0);
    assert_eq!(hit["cost"]["total"], 0);
    assert_eq!(hit["cost"]["input_miss"], 0);
    assert!(
        hit["result"]["upstream_status"].is_null(),
        "an absent measurement is absent, never 0"
    );
    assert!(hit["result"]["upstream_ms"].is_null());
    assert!(
        hit["protocol"]["protocol_out"].is_null(),
        "no bytes left the process (spec §6's third class)"
    );
    assert_eq!(
        hit["errors"],
        serde_json::json!([]),
        "a hit declares no failure"
    );
    assert_eq!(
        hit["schema_version"], 2,
        "the group is additive: the version stays 2"
    );
    // The rest is the ordinary record of the request: the route it
    // resolved to and did NOT call.
    assert_eq!(hit["decision"]["provider"], "mock");
    assert_eq!(hit["decision"]["model"], "glm");
}

/// (a) A buffered hit is the recorded bytes; the record says so; the
/// window reconciles (limbs 1–4).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_89a_a_buffered_hit_is_the_recorded_bytes() {
    let rig = Rig::start("a-buffered", CannedResponse::json(200, "OK", CANNED)).await;

    // One miss, then two byte-identical repeats inside the same session.
    let (s1, b1) = rig.post(BODY_BUFFERED, &[]);
    let (s2, b2) = rig.post(BODY_BUFFERED, &[]);
    let (s3, b3) = rig.post(BODY_BUFFERED, &[]);
    assert_eq!((s1, s2, s3), (200, 200, 200), "statuses: {s1}/{s2}/{s3}");

    // Limb 1: the upstream saw ONE request, and the repeats returned the
    // recorded bytes — byte for byte, the full body.
    let seen = rig.upstream.requests();
    assert_eq!(seen.len(), 1, "the repeats made no upstream call");
    assert_eq!(b1, CANNED, "the miss returned the upstream's body");
    assert_eq!(b2, b1, "hit 1 returned the recorded bytes verbatim");
    assert_eq!(b3, b1, "hit 2 returned the recorded bytes verbatim");

    // Limbs 2–3: the records say so.
    let records = rig.records();
    assert_eq!(records.len(), 3, "one record per client request");
    let (miss, hit1, hit2) = (&records[0], &records[1], &records[2]);
    assert!(
        miss.get("cache").is_none(),
        "the miss carries no cache group"
    );
    assert_eq!(miss["usage_missing"], false, "the miss measured usage");
    assert_hit_record(hit1, miss, b2.len());
    assert_hit_record(hit2, miss, b3.len());
    assert_eq!(
        hit1["cache"]["replayed"]["request_id"], hit2["cache"]["replayed"]["request_id"],
        "both hits name the same source record"
    );

    // Limb 4: the ledger reconciles — Σ usage over the window is unmoved
    // by the repeats, and hits are never counted as calls.
    let sum_input: u64 = records
        .iter()
        .map(|r| r["usage"]["input_total"].as_u64().unwrap())
        .sum();
    assert_eq!(
        sum_input,
        miss["usage"]["input_total"].as_u64().unwrap(),
        "Σ usage over the window is unmoved by the hits"
    );
    let calls = records
        .iter()
        .filter(|r| !r["result"]["upstream_status"].is_null())
        .count();
    assert_eq!(calls, 1, "exactly the miss was an upstream call");
}

/// (b) A streaming hit is the recorded bytes (the SSE byte sequence,
/// terminator included).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_89b_a_streaming_hit_is_the_recorded_bytes() {
    let rig = Rig::start("b-streaming", CannedResponse::sse(sse_with_usage())).await;

    let (s1, raw1) = rig.post(BODY_STREAMED, &[]);
    let (s2, raw2) = rig.post(BODY_STREAMED, &[]);
    assert_eq!((s1, s2), (200, 200), "statuses: {s1}/{s2}");

    // The upstream saw one request; the replayed stream is the recorded
    // SSE byte sequence, verbatim — chunk framing is transport, the
    // event bytes (with the [DONE] terminator) are the contract.
    let seen = rig.upstream.requests();
    assert_eq!(seen.len(), 1, "the repeat made no upstream call");
    let (b1, b2) = (testkit::dechunk(&raw1), testkit::dechunk(&raw2));
    assert_eq!(b1, sse_bytes(), "the miss relayed the upstream's events");
    assert_eq!(b2, b1, "the hit replayed the recorded events verbatim");

    let records = rig.records();
    assert_eq!(records.len(), 2, "one record per client request");
    let (miss, hit) = (&records[0], &records[1]);
    assert!(
        miss.get("cache").is_none(),
        "the miss carries no cache group"
    );
    assert_eq!(
        miss["usage_missing"], false,
        "the streamed miss measured usage"
    );
    assert_hit_record(hit, miss, b2.len());
    let sum_input: u64 = records
        .iter()
        .map(|r| r["usage"]["input_total"].as_u64().unwrap())
        .sum();
    assert_eq!(
        sum_input,
        miss["usage"]["input_total"].as_u64().unwrap(),
        "Σ usage over the window is unmoved by the hit"
    );
}

/// (c) The key separates what it must (limb 5): a changed session, a
/// changed mode word, or one changed body byte is a miss; identical
/// everything is the only hit.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_89c_the_key_separates_session_mode_and_body_bytes() {
    let rig = Rig::start("c-key", CannedResponse::json(200, "OK", CANNED)).await;

    // The original request (session sess-89, passthrough) — recorded.
    let (s0, _) = rig.post(BODY_BUFFERED, &[]);
    // Same bytes, another session: a MISS (the session is a key component).
    let body_other_session = String::from_utf8(BODY_BUFFERED.to_vec())
        .unwrap()
        .replace("sess-89", "sess-other");
    let (s1, _) = rig.post(body_other_session.as_bytes(), &[]);
    // Same bytes, same session, the transform mode word: a MISS (the mode
    // is a key component; with no rule set mounted it is asked-not-applied).
    let (s2, _) = rig.post(BODY_BUFFERED, &[("x-router-transform", "transform")]);
    // Same session, one body byte changed: a MISS (the content component).
    let (s3, _) = rig.post(BODY_ONE_BYTE_OFF, &[]);
    assert_eq!(
        (s0, s1, s2, s3),
        (200, 200, 200, 200),
        "statuses: {s0}/{s1}/{s2}/{s3}"
    );
    let seen = rig.upstream.requests();
    assert_eq!(
        seen.len(),
        4,
        "every separated variant was forwarded upstream"
    );

    // Identical everything is the only hit.
    let (s4, b4) = rig.post(BODY_BUFFERED, &[]);
    assert_eq!(s4, 200);
    let seen = rig.upstream.requests();
    assert_eq!(seen.len(), 4, "the exact repeat made no upstream call");
    assert_eq!(b4, CANNED, "the hit returned the recorded bytes");

    let records = rig.records();
    assert_eq!(records.len(), 5);
    for (i, rec) in records[..4].iter().enumerate() {
        assert!(
            rec.get("cache").is_none(),
            "separated variant {i} is an ordinary miss: no cache group"
        );
    }
    assert_hit_record(&records[4], &records[0], b4.len());
}

/// (d) A non-`2xx` response is never stored: the repeat is a second
/// upstream call (fail-closed; a 400 classifies `format_error` — no
/// demotion, so the repeat really does reach the upstream again).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_89d_a_non_2xx_response_is_never_stored() {
    let rig = Rig::start(
        "d-non-2xx",
        CannedResponse::json(
            400,
            "Bad Request",
            br#"{"error":{"message":"bad request","type":"invalid_request"}}"#,
        ),
    )
    .await;

    let (s1, _) = rig.post(BODY_BUFFERED, &[]);
    let (s2, _) = rig.post(BODY_BUFFERED, &[]);
    assert_eq!(s1, s2, "the repeat got the same refusal shape: {s1}/{s2}");
    let seen = rig.upstream.requests();
    assert_eq!(
        seen.len(),
        2,
        "a non-2xx response is never stored: the repeat is a second call"
    );
    let records = rig.records();
    assert_eq!(records.len(), 2);
    for (i, rec) in records.iter().enumerate() {
        assert!(
            rec.get("cache").is_none(),
            "record {i} of a refused pair carries no cache group"
        );
    }
}

/// (e) An incomplete stream is never stored: an upstream that dies
/// mid-stream leaves nothing replayable, so the repeat is a second call
/// (and the truncation is declared in `errors[]`, never replayed as a
/// complete answer).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_89e_an_incomplete_stream_is_never_stored() {
    // The FIRST answer dies mid-stream; the repeat's (queued behind it —
    // the rig's queue is FIFO) is a complete stream. Only a stored entry
    // could suppress the second call, and nothing may be stored.
    let rig = Rig::start(
        "e-incomplete",
        CannedResponse::sse(vec![
            SseChunk::event(SSE_EVENTS[0]),
            SseChunk::abort_after(10),
        ]),
    )
    .await;
    rig.upstream.queue(CannedResponse::sse(sse_with_usage()));

    let (s1, _) = rig.post(BODY_STREAMED, &[]);
    let (s2, _) = rig.post(BODY_STREAMED, &[]);
    assert_eq!((s1, s2), (200, 200), "statuses: {s1}/{s2}");
    let seen = rig.upstream.requests();
    assert_eq!(
        seen.len(),
        2,
        "an incomplete body is never stored: the repeat is a second call"
    );
    let records = rig.records();
    assert_eq!(records.len(), 2);
    for (i, rec) in records.iter().enumerate() {
        assert!(
            rec.get("cache").is_none(),
            "record {i} carries no cache group"
        );
    }
    assert!(
        !records[0]["errors"].as_array().unwrap().is_empty(),
        "the truncated stream declared its truncation"
    );
}

/// (f) A request with no session identity is neither looked up nor
/// stored — two identical sessionless requests are two upstream calls,
/// and no `session: null` bucket can form.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_89f_a_sessionless_request_is_neither_looked_up_nor_stored() {
    let rig = Rig::start("f-sessionless", CannedResponse::json(200, "OK", CANNED)).await;
    let body_sessionless =
        br#"{"model":"mock/glm","messages":[{"role":"user","content":"repeat me"}],"stream":false}"#;

    let (s1, _) = rig.post(body_sessionless, &[]);
    let (s2, _) = rig.post(body_sessionless, &[]);
    assert_eq!((s1, s2), (200, 200), "statuses: {s1}/{s2}");
    let seen = rig.upstream.requests();
    assert_eq!(
        seen.len(),
        2,
        "no session ⇒ no key ⇒ neither looked up nor stored: two calls"
    );
    let records = rig.records();
    assert_eq!(records.len(), 2);
    for (i, rec) in records.iter().enumerate() {
        assert!(
            rec["identity"]["session"].is_null(),
            "record {i} is sessionless"
        );
        assert!(
            rec.get("cache").is_none(),
            "record {i} carries no cache group"
        );
    }
}

/// (g) The owner level: the store's frozen bounds and the
/// `config_digest` key component, asserted against the one owner
/// directly (the serving-path limbs above exercise the rest through the
/// rig). A test here keeps the ledger rule: it lives in THIS case file,
/// not in a `#[cfg(test)]` block inside the owner's module.
#[test]
fn conf_89g_owner_level_bounds_and_config_digest_separation() {
    use router_core::response_cache::{
        RecordedResponse, RequestFacts, ResponseKey, ResponseStore, SourceRef, MAX_ENTRIES,
        MAX_STORED_BYTES,
    };
    use router_core::transform::TransformMode;

    fn facts<'a>(digest: &'a str, body: &'a [u8]) -> RequestFacts<'a> {
        RequestFacts {
            protocol_in: "chat",
            config_digest: digest,
            session: "sess",
            transform_mode: TransformMode::Passthrough,
            body,
        }
    }
    let resp = |body: &[u8]| RecordedResponse {
        status: 200,
        content_type: None,
        body: body.to_vec(),
        source: SourceRef {
            request_id: "r".into(),
            session: "sess".into(),
            turn_index: 0,
        },
    };

    // The config revision is a key component (ADR-042 §3.1): a repeat
    // under another revision of the config is a miss, structurally.
    let k_a = ResponseKey::for_request(&facts("rev-a", b"body"));
    let k_b = ResponseKey::for_request(&facts("rev-b", b"body"));
    assert_ne!(k_a, k_b, "config_digest is a key component");
    let mut store = ResponseStore::new();
    store.record(k_a.clone(), resp(b"answer-a"));
    assert!(store.lookup(&k_b).is_none(), "another revision is a miss");
    assert_eq!(store.lookup(&k_a).unwrap().body, b"answer-a");

    // Fail-closed at the store itself (§3.3/§3.4): whatever the caller
    // knows, a non-2xx status and an oversized body are refused.
    let mut store = ResponseStore::new();
    store.record(
        k_a.clone(),
        RecordedResponse {
            status: 500,
            ..resp(b"err")
        },
    );
    assert!(store.is_empty(), "a non-2xx response is never stored");
    let oversized = vec![0u8; MAX_STORED_BYTES as usize + 1];
    store.record(k_a.clone(), resp(&oversized));
    assert!(
        store.is_empty(),
        "an entry past the byte bound is never stored"
    );

    // The entry bound: MAX_ENTRIES + 1 inserts leave exactly MAX_ENTRIES,
    // and the FIRST inserted key was evicted (FIFO by insertion).
    let mut store = ResponseStore::new();
    let first = ResponseKey::for_request(&facts("rev", b"first"));
    store.record(first.clone(), resp(b"x"));
    for i in 0..MAX_ENTRIES {
        let k = ResponseKey::for_request(&facts("rev", i.to_string().as_bytes()));
        store.record(k, resp(b"x"));
    }
    assert_eq!(store.len(), MAX_ENTRIES, "the entry bound holds");
    assert!(
        store.lookup(&first).is_none(),
        "the oldest entry was evicted first (FIFO by insertion)"
    );
    assert!(
        store.stored_bytes() <= MAX_STORED_BYTES,
        "the byte bound holds"
    );
}
