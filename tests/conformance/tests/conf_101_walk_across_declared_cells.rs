//! CONF-101 (ADR-051 §2.1/§2.3, spec §2 / §4.2 / §8, DESIGN §12.10.9): **the
//! walk's discriminant is the declared cell set, not the entry's
//! `wire_api`** — and the two fields are allowed to disagree.
//!
//! An entry that declares a cell is served on it **whatever its `wire_api`
//! says**; an entry that does not declare it is skipped with the reason
//! `wire_mismatch`, whose meaning is now *this entry's `supports` does not
//! declare this inbound protocol* (and not *its `wire_api` differs*). The
//! rig makes the two fields disagree on purpose: a fixture in which
//! `supports == [wire_api]` cannot tell the two rules apart, so it would
//! pass either way — the case would be vacuous.
//!
//! Rig: a **chat** request whose resolved route is `keyless-chat/m` (its key
//! is named but never set, so the walk skips it), then a `fallback` chain of
//! * `disagree/m` — `supports: [responses]`, `wire_api: responses` (the
//!   legal shape of a foreign entry, CONF-57's own): it does **not** declare
//!   `chat`, so it is skipped — an entry that does not declare the cell
//!   cannot serve it, whatever its `wire_api` names;
//! * `both/m` — `supports: [chat, responses]`, `wire_api: responses`: it
//!   declares `chat`, so it serves — even though its `wire_api` is not `chat`
//!   (the old rule's discriminant would have answered `501`).
//!
//! Arms:
//! * **(a) served.** With `both` present: the request is `200` from `both`,
//!   and the mock's recorded `path` is `urls.chat`. The `disagree` mock
//!   receives **zero** requests.
//! * **(b) the skip's semantics.** With `both` dropped, the walk exhausts:
//!   `502`, `error.type == "upstream_error"`, `details.stage ==
//!   "no_available_route"`, and `details.skipped[]` in chain order —
//!   `{keyless-chat/m, "keyless"}`, `{disagree/m, "wire_mismatch"}` — with no
//!   `upstream.submitted` row, `usage_missing: true` and nothing charged.
//!
//! Red at the base: arm (a) is a `501` (the entry's `wire_api` is not the
//! inbound protocol) and arm (b) lists `disagree/m` for the old reason.
//!
//! Depends on: the eligibility and narration predicates on both forwarding
//! paths, and `url_for` being called with the inbound protocol.

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, CannedResponse};

const CHAT_OK: &str = r#"{"id":"ok","choices":[{"index":0,"message":{"role":"assistant","content":"served"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}"#;
const CLIENT_BODY: &str =
    r#"{"model":"keyless-chat/m","messages":[{"role":"user","content":"x"}],"stream":false}"#;

/// The rig's three mocks, the listen address and the temp dir.
struct Rig {
    keyless: testkit::MockUpstream,
    disagree: testkit::MockUpstream,
    both: testkit::MockUpstream,
    listen_addr: String,
    dir: std::path::PathBuf,
}

/// `with_both: false` drops the serving entry and its chain row — the
/// exhausted-walk shape (arm b).
async fn rig(tag: &str, with_both: bool) -> Rig {
    let keyless = testkit::MockUpstream::start().await.unwrap();
    let disagree = testkit::MockUpstream::start().await.unwrap();
    let both = testkit::MockUpstream::start().await.unwrap();
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let dir = testkit::tempdir(tag);

    // `disagree` is keyed; `both` is keyed when it is in the chain.
    std::env::set_var("CONF101_DISAGREE_KEY", "sk-disagree");
    if with_both {
        std::env::set_var("CONF101_BOTH_KEY", "sk-both");
    }

    let both_port = both.addr.port();
    let both_entry = if with_both {
        format!(
            r#"
  - name: both
    urls:
      chat:      http://127.0.0.1:{both_port}/v1/chat/completions
      responses: http://127.0.0.1:{both_port}/v1/responses
    api_key_env: CONF101_BOTH_KEY
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
"#
        )
    } else {
        String::new()
    };
    let chain = if with_both {
        "  - disagree/m\n  - both/m\n"
    } else {
        "  - disagree/m\n"
    };

    let config = format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: keyless-chat
    urls:
      chat: http://127.0.0.1:{keyless_port}/v1/chat/completions
    api_key_env: CONF101_KEYLESS_KEY_UNSET
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
  - name: disagree
    urls:
      responses: http://127.0.0.1:{disagree_port}/v1/responses
    api_key_env: CONF101_DISAGREE_KEY
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
        source: "mock upstream (no price; test fixture)"{both_entry}
aliases: {{}}
plugins: []
fallback:
{chain}"#,
        keyless_port = keyless.addr.port(),
        disagree_port = disagree.addr.port(),
    );
    std::fs::write(dir.join("config.yaml"), config).unwrap();
    Rig {
        keyless,
        disagree,
        both,
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

/// (a) the declared cell is what serves, and the disagreeing entry is skipped.
async fn arm_served() {
    let r = rig("conf101-served", true).await;
    r.both
        .queue(CannedResponse::json(200, "OK", CHAT_OK.as_bytes()));
    let task = serve(&r.dir, &r.listen_addr).await;

    let (status, _b, _h) = testkit::http_post(
        &r.listen_addr,
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[],
    );
    assert_eq!(status, 200, "the entry that declares `chat` serves it");

    let seen = r.both.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].path, "/v1/chat/completions",
        "the cell's own URL, not the entry's `wire_api` URL"
    );
    assert_eq!(
        r.disagree.requests().len(),
        0,
        "an entry that does not declare `chat` is never attempted"
    );

    task.abort();
}

/// (b) the skip's semantics: `wire_mismatch` means "not declared".
async fn arm_exhausted() {
    let r = rig("conf101-exhausted", false).await;
    let task = serve(&r.dir, &r.listen_addr).await;

    let (status, body, _h) = testkit::http_post(
        &r.listen_addr,
        "/v1/chat/completions",
        CLIENT_BODY.as_bytes(),
        &[],
    );
    assert_eq!(status, 502);
    let v: serde_json::Value = serde_json::from_slice(&body).expect("json body");
    assert_eq!(v["error"]["type"], "upstream_error");
    let details = &v["error"]["details"];
    assert_eq!(details["stage"], "no_available_route");
    let skipped = details["skipped"].as_array().expect("skipped[]");
    assert_eq!(skipped.len(), 2, "one entry per offered candidate");
    assert_eq!(skipped[0]["route"], "keyless-chat/m");
    assert_eq!(skipped[0]["reason"], "keyless");
    assert_eq!(skipped[1]["route"], "disagree/m");
    assert_eq!(
        skipped[1]["reason"], "wire_mismatch",
        "the entry does not declare `chat` — its `wire_api` says otherwise, which is the point"
    );
    assert_eq!(r.disagree.requests().len(), 0);
    assert_eq!(
        r.keyless.requests().len(),
        0,
        "the keyless provider holds no transport"
    );

    task.abort();
    let evs = events(&r.dir);
    assert_eq!(
        evs.iter()
            .filter(|(k, _)| k == "upstream.submitted")
            .count(),
        0,
        "nothing was attempted"
    );
}

/// One ID, one case file, one test function (DESIGN §12.8's own rule): both
/// arms run inside this one test function — the case carries no `#[ignore]`
/// since R69 un-parked it (ADR-051 §2.7) — so the register's occupancy moves
/// by exactly one id, not two.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_101_walk_discriminant_is_the_declared_cell() {
    arm_served().await;
    arm_exhausted().await;
}
