//! CONF-104 (ADR-051 §2.3 + **ADR-022 decision 3**, spec §8 / §4.2): **the
//! frozen `no_available_route` refusal still holds under the new
//! discriminant** — and the discriminant is the only thing that moved.
//!
//! The case is ADR-022 decision 3's shape, re-pointed at `supports ∋
//! proto_in`, on both media. A chat request whose chain offers **four**
//! candidates none of which can be attempted:
//!
//! * `keyless-a/m` — the **resolved** route: `supports: [chat]`, its key named
//!   but never set ⇒ `keyless`;
//! * `keyless-a/m2` — the same provider's second model, offered by the chain ⇒
//!   `keyless` again (ADR-023 decision 3: the provider-level exclusion may not
//!   swallow it);
//! * `deaf/m` — `supports: [chat, responses]`, `wire_api: responses`, its key
//!   unset ⇒ **`keyless`**, which is the case's discriminating arm: this entry
//!   *does* declare `chat`, so under the new rule the wire test passes and the
//!   key test is what refuses it — the base tree reports `wire_mismatch` here
//!   (its discriminant is `wire_api`, and `responses != chat`);
//! * `nocell/m` — `supports: [responses]`, **keyed** ⇒ `wire_mismatch`, because
//!   the entry declares no `chat` cell at all. Nothing is attempted.
//!
//! Assertions, both media (buffered and `"stream": true`), the streaming arm
//! differing by its pre-existing `"stream": true` member alone:
//! `502`, `error.type == "upstream_error"`, the frozen sentence verbatim,
//! `details.stage == "no_available_route"`, `details.skipped[]` holding **one
//! entry per candidate the chain offered** (four) in the chain's own order,
//! each with a reason from `{unknown_provider, keyless, wire_mismatch,
//! demoted}`; `upstream_status` / `error_class` both `null`; **no**
//! `upstream.submitted` row; `usage_missing: true` and nothing charged.
//!
//! Red at the base on exactly the `deaf/m` entry's reason (and only there) —
//! which is what makes this the case that proves ADR-022 decision 3 itself was
//! not reopened.
//!
//! Depends on: the eligibility predicate on both forwarding paths only; the
//! refusal body is ADR-022 decision 3's, byte-identical.

#![forbid(unsafe_code)]

use vadis_conformance::testkit;

const CLIENT_BODY: &str =
    r#"{"model":"keyless-a/m","messages":[{"role":"user","content":"x"}],"stream":false}"#;
const CLIENT_BODY_STREAM: &str =
    r#"{"model":"keyless-a/m","messages":[{"role":"user","content":"x"}],"stream":true}"#;

struct Rig {
    deaf: testkit::MockUpstream,
    nocell: testkit::MockUpstream,
    keyless_a: testkit::MockUpstream,
    listen_addr: String,
    dir: std::path::PathBuf,
}

async fn rig(tag: &str) -> Rig {
    let keyless_a = testkit::MockUpstream::start().await.unwrap();
    let deaf = testkit::MockUpstream::start().await.unwrap();
    let nocell = testkit::MockUpstream::start().await.unwrap();
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let dir = testkit::tempdir(tag);

    // `nocell` is the only keyed provider; the other two name keys this
    // process never sets.
    std::env::set_var("CONF104_NOCELL_KEY", "sk-nocell");

    let config = format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: keyless-a
    urls:
      chat: http://127.0.0.1:{keyless_port}/v1/chat/completions
    api_key_env: CONF104_KEYLESS_A_KEY_UNSET
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
      - id: m2
        context: 128k
        price:
          input_miss: 0.001
          input_hit: 0.0001
          cache_write: 0.0
          output: 0.002
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "mock upstream (no price; test fixture)"
  - name: deaf
    urls:
      chat:      http://127.0.0.1:{deaf_port}/v1/chat/completions
      responses: http://127.0.0.1:{deaf_port}/v1/responses
    api_key_env: CONF104_DEAF_KEY_UNSET
    wire_api: responses
    supports: [chat, responses]
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
  - name: nocell
    urls:
      responses: http://127.0.0.1:{nocell_port}/v1/responses
    api_key_env: CONF104_NOCELL_KEY
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
  - keyless-a/m2
  - deaf/m
  - nocell/m
"#,
        keyless_port = keyless_a.addr.port(),
        deaf_port = deaf.addr.port(),
        nocell_port = nocell.addr.port(),
    );
    std::fs::write(dir.join("config.yaml"), config).unwrap();
    Rig {
        deaf,
        nocell,
        keyless_a,
        listen_addr,
        dir,
    }
}

async fn serve(dir: &std::path::Path, listen_addr: &str) -> tokio::task::JoinHandle<i32> {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_104_no_available_route_still_holds() {
    let r = rig("conf104").await;
    let task = serve(&r.dir, &r.listen_addr).await;

    let (s_buf, b_buf, _h) = testkit::http_post(
        &r.listen_addr,
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[],
    );
    let (s_str, b_str, _h) = testkit::http_post(
        &r.listen_addr,
        "/v1/chat/completions",
        CLIENT_BODY_STREAM.as_bytes(),
        &[],
    );
    assert_eq!(s_buf, 502, "buffered: nothing may serve");
    assert_eq!(s_str, 502, "streaming: nothing may serve");

    let v_buf: serde_json::Value = serde_json::from_slice(&b_buf).expect("buffered refusal json");
    let v_str: serde_json::Value = serde_json::from_slice(&b_str).expect("streaming refusal json");

    let expected = [
        ("keyless-a/m", "keyless"),
        ("keyless-a/m2", "keyless"),
        ("deaf/m", "keyless"),
        ("nocell/m", "wire_mismatch"),
    ];
    for (medium, v) in [("buffered", &v_buf), ("streaming", &v_str)] {
        assert_eq!(v["error"]["type"], "upstream_error", "{medium}");
        assert_eq!(
            v["error"]["message"],
            "no available route: every candidate provider is demoted, keyless or unavailable",
            "{medium}: the frozen sentence, verbatim"
        );
        let details = &v["error"]["details"];
        assert_eq!(details["stage"], "no_available_route", "{medium}");
        assert_eq!(
            details["upstream_status"],
            serde_json::Value::Null,
            "{medium}: nothing was attempted"
        );
        assert_eq!(details["error_class"], serde_json::Value::Null, "{medium}");
        let skipped = details["skipped"].as_array().expect("skipped[]");
        assert_eq!(
            skipped.len(),
            expected.len(),
            "{medium}: one entry per candidate the chain offered"
        );
        for (i, (route, reason)) in expected.iter().enumerate() {
            assert_eq!(skipped[i]["route"], *route, "{medium}: chain order");
            assert_eq!(
                skipped[i]["reason"], *reason,
                "{medium}: {route}'s reason under the declared-cell rule"
            );
        }
    }
    // The two media differ by the streaming arm's pre-existing member alone.
    let mut buf_details = v_buf["error"]["details"].as_object().unwrap().clone();
    let mut str_details = v_str["error"]["details"].as_object().unwrap().clone();
    assert_eq!(
        str_details.remove("stream"),
        Some(serde_json::Value::Bool(true))
    );
    assert_eq!(buf_details.remove("stream"), None);
    assert_eq!(buf_details, str_details, "one chain, one array, two media");

    task.abort();
    assert_eq!(r.deaf.requests().len(), 0);
    assert_eq!(r.nocell.requests().len(), 0);
    assert_eq!(r.keyless_a.requests().len(), 0);
    let evs = events(&r.dir);
    assert_eq!(
        evs.iter()
            .filter(|(k, _)| k == "upstream.submitted")
            .count(),
        0,
        "no intent row: nothing was attempted"
    );
    let records = trace_records(&r.dir);
    for rec in records.iter().filter(|x| x["result"]["status"] == 502) {
        assert_eq!(rec["usage_missing"], true, "the terminal failure record");
    }
}
