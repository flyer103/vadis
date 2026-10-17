//! CONF-107 (spec §4.19, ADR-055, DESIGN §12.8): **the model list surface —
//! the configured alias names, and nothing else, answered by the running
//! process with no record of its own.** Seven limbs, through the real
//! `serve` assembly over loopback HTTP:
//!
//! - (a) **the alias namespace, in order, twice** — `GET /v1/models` with the
//!   guard token answers `200 application/json`, `object: "list"`, exactly
//!   three entries each holding **exactly** `{id, object, created, owned_by}`,
//!   ids in ascending byte order (`chat-fast`, `coding-bulk`, `coding-fast`),
//!   `created == 0` and `owned_by == "vadis"` (the two constants), no
//!   `X-Vadis-Request-Id` on the admitted arm, and a second call
//!   **byte-identical** to the first (the body is a pure function of the
//!   roster: no clock, no RNG, no per-process state).
//! - (b) **the route is never published** — the raw body contains neither the
//!   provider name nor either model id, and no `id` holds a `/`. Asserted with
//!   the roster's own literal strings, so the negative cannot pass vacuously.
//! - (c) **the guard's two refusal arms** — no token ⇒ `401` with §8's body
//!   (`details.header: null`, `X-Vadis-Request-Id` present); a wrong token ⇒
//!   the same with `details.header: "authorization"`. Each refusal leaves
//!   **exactly one** pre-pipeline record whose `protocol.protocol_in` is
//!   `"models"`, `errors[].kind` is `"unauthorized"`, `result.status` is `401`.
//! - (d) **the empty roster** — `aliases: {}` answers `200` with exactly
//!   `{object, data}` and `data: []`: never `404`, never `500`.
//! - (e) **the admission's stage set is empty** — over two admitted calls the
//!   trace directory gains no line, and after `serve` stops the store's event
//!   log holds no pipeline-kind row at all (one `config.applied` for the boot,
//!   which is not this endpoint's): no `request.received`, no `upstream.*`, no
//!   cost, no session, no prefix. A stage-set assertion, not "the body is
//!   fine".
//! - (f) **the method face, both directions** — `POST` **with** the token ⇒
//!   `405` with an `allow` header naming `GET`, no §8 body and **no** record;
//!   `POST` **without** a token ⇒ `401` and exactly one record. The second arm
//!   is the contract's stated precedence (auth before method dispatch, the
//!   route-layer wiring `guarded_metrics_route`/`guarded_state_route` already
//!   use), asserted so a later wiring change that flips the order is caught.
//! - (g) **the reload** — a rewritten `aliases:` block is accepted **without a
//!   restart** (the switch observed through `/health`'s `config_digest`, the
//!   technique CONF-96 uses), and the next call lists the new set on the same
//!   process, the same port and the same trace directory.
//!
//! Offline: one discard upstream address in the roster and no dialled provider
//! at all — this surface never forwards anything.

#![forbid(unsafe_code)]

use std::io::{Read, Write};

use vadis_conformance::testkit;

/// The roster's provider and its two models — the literal strings limb (b)
/// forbids in the body.
const PROVIDER: &str = "zai-cn-plan";
const MODEL: &str = "glm-5.3";
const MODEL_FLASH: &str = "glm-5.3-flash";

/// The three aliases limb (a) expects, in the order the `BTreeMap` yields them.
const THREE_ALIASES: &str = "aliases:\n  chat-fast: zai-cn-plan/glm-5.3\n  coding-bulk: zai-cn-plan/glm-5.3-flash\n  coding-fast: zai-cn-plan/glm-5.3";

/// One alias, for the reload limb (g).
const ONE_ALIAS: &str = "aliases:\n  solo: zai-cn-plan/glm-5.3";

// ---------------------------------------------------------------------------
// The rig: conf_106/conf_87's shape — hand-written config, the real
// `vadis_cli::serve` in-process, hand-written TCP.
// ---------------------------------------------------------------------------

fn config_yaml(listen_port: u16, token_env: &str, aliases: &str) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 2s, request_timeout: 30s, auth_token_env: {token_env} }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: {PROVIDER}
    urls:
      chat: http://127.0.0.1:9/v1/chat/completions
    api_key_env: CONF107_KEY
    wire_api: chat
    supports: [chat]
    models:
      - id: {MODEL}
        context: 128k
        price:
          input_miss: 0.001
          input_hit: 0.0001
          cache_write: 0.0
          output: 0.002
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "mock upstream (no price; test fixture)"
      - id: {MODEL_FLASH}
        context: 128k
        price:
          input_miss: 0.001
          input_hit: 0.0001
          cache_write: 0.0
          output: 0.002
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "mock upstream (no price; test fixture)"

{aliases}
plugins: []
fallback: []
"#
    )
}

/// One rig: tempdir + config written + the real serve spawned. Returns
/// (dir, listen_addr, serve_task). The caller sets the token env first.
async fn rig(
    tag: &str,
    token_env: &str,
    token: &str,
    aliases: &str,
) -> (std::path::PathBuf, String, tokio::task::JoinHandle<i32>) {
    let dir = testkit::tempdir(tag);
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    std::fs::write(
        dir.join("config.yaml"),
        config_yaml(listen_port, token_env, aliases),
    )
    .unwrap();
    std::env::set_var("CONF107_KEY", "sk-conf107");
    std::env::set_var(token_env, token);
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    (dir, listen_addr, serve_task)
}

/// One HTTP exchange over raw TCP, method-parameterized (conf_106's reader,
/// widened for the method face): (status, body, headers).
fn http_req(
    addr: &str,
    method: &str,
    path: &str,
    headers: &[(String, String)],
) -> (u16, Vec<u8>, Vec<(String, String)>) {
    let mut stream = std::net::TcpStream::connect(addr).expect("connect");
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if method.eq_ignore_ascii_case("POST") {
        req.push_str("content-length: 0\r\n");
    }
    req.push_str("Connection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    let split = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("head/body split");
    let head = String::from_utf8_lossy(&buf[..split]).into_owned();
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("status line");
    let headers = head
        .lines()
        .skip(1)
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    (status, buf[split + 4..].to_vec(), headers)
}

fn header<'h>(headers: &'h [(String, String)], name: &str) -> Option<&'h str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn bearer(token: &str) -> Vec<(String, String)> {
    vec![("authorization".to_string(), format!("Bearer {token}"))]
}

/// `GET /v1/models` raw — the byte-identity limb needs the exact bytes, so the
/// body is returned unparsed.
fn models_raw(addr: &str, token: &str) -> (u16, Vec<u8>, Vec<(String, String)>) {
    http_req(addr, "GET", "/v1/models", &bearer(token))
}

fn trace_records(trace_dir: &std::path::Path) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(trace_dir) else {
        return out;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        for line in std::fs::read_to_string(path).unwrap().lines() {
            if !line.trim().is_empty() {
                out.push(serde_json::from_str(line).expect("record json"));
            }
        }
    }
    out
}

/// `/health`'s `config.config_digest` — the reload's observation (CONF-96's).
fn health_digest(addr: &str) -> Option<String> {
    let (status, body, _h) = http_req(addr, "GET", "/health", &[]);
    assert_eq!(status, 200, "the liveness control: /health answers");
    let v: serde_json::Value = serde_json::from_slice(&body).expect("health json");
    v["config"]["config_digest"].as_str().map(str::to_string)
}

// ---------------------------------------------------------------------------
// The case: seven limbs, one rig per group — each with its own tempdir, port
// and token-variable name, so no two limbs share state.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_107_v1_models_lists_aliases() {
    // -----------------------------------------------------------------------
    // (a): the alias namespace — three entries, the four keys, the stated
    // order, the two constants, and byte-identical repeated calls.
    // -----------------------------------------------------------------------
    let (dir_a, addr_a, serve_a) = rig(
        "conf107-l1",
        "CONF107_L1_TOKEN",
        "tok-conf107-l1",
        THREE_ALIASES,
    )
    .await;

    let (status, raw, headers) = models_raw(&addr_a, "tok-conf107-l1");
    assert_eq!(status, 200, "the admitted GET answers 200, got {status}");
    assert_eq!(
        header(&headers, "content-type"),
        Some("application/json"),
        "spec §4.19's success body is JSON"
    );
    assert!(
        header(&headers, "x-vadis-request-id").is_none(),
        "the admitted arm allocates no request, so it carries no request id"
    );

    let body: serde_json::Value = serde_json::from_slice(&raw).expect("the body is JSON");
    assert_eq!(body["object"], "list", "the OpenAI list shape's own word");
    let data = body["data"].as_array().expect("data[] is an array");
    assert_eq!(data.len(), 3, "one entry per alias, got {data:?}");

    let ids: Vec<&str> = data
        .iter()
        .map(|e| e["id"].as_str().expect("every entry has a string id"))
        .collect();
    assert_eq!(
        ids,
        vec!["chat-fast", "coding-bulk", "coding-fast"],
        "data[] is ascending by id (the BTreeMap's own order), asserted on the wire"
    );

    for entry in data {
        let obj = entry.as_object().expect("an entry is an object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["created", "id", "object", "owned_by"],
            "an entry carries exactly the four keys of the shape, got {obj:?}"
        );
        assert_eq!(entry["object"], "model", "the entry's own object word");
        assert_eq!(
            entry["created"].as_u64(),
            Some(0),
            "created is the constant 0 — never a clock read (got {})",
            entry["created"]
        );
        assert_eq!(
            entry["owned_by"], "vadis",
            "owned_by is the constant literal \"vadis\""
        );
    }

    // Byte stability: one revision, two calls, the same bytes.
    let (status2, raw2, _h) = models_raw(&addr_a, "tok-conf107-l1");
    assert_eq!(status2, 200);
    assert_eq!(
        raw2, raw,
        "two calls over one revision are byte-identical (no clock, no RNG)"
    );

    // D8's query-ignored rule, given its assertion landing: one call
    // with a query string answers the same 200 and the same bytes.
    let (status_q, raw_q, _h) = http_req(
        &addr_a,
        "GET",
        "/v1/models?limit=1",
        &bearer("tok-conf107-l1"),
    );
    assert_eq!(
        status_q, 200,
        "a query string is ignored, not refused, got {status_q}"
    );
    assert_eq!(
        raw_q, raw,
        "the ignored query string changes nothing in the body's bytes"
    );

    // -----------------------------------------------------------------------
    // (b): the route is never published — the export boundary, asserted with
    // the roster's own literal strings (the same rig: the roster under test is
    // the one whose strings must not appear).
    // -----------------------------------------------------------------------
    let text = String::from_utf8(raw.clone()).expect("the body is UTF-8");
    assert!(
        !text.contains(PROVIDER),
        "the provider name must not appear in the body: {text}"
    );
    assert!(
        !text.contains(MODEL),
        "the resolved model id (and its -flash twin) must not appear: {text}"
    );
    assert!(
        !text.contains(MODEL_FLASH),
        "the second model id must not appear either: {text}"
    );
    for entry in body["data"].as_array().expect("data[]") {
        let id = entry["id"].as_str().expect("a string id");
        assert!(
            !id.contains('/'),
            "an id is an alias name, never a provider/model route: {id}"
        );
    }
    // The positive control: the surface answered the roster, not an empty body.
    assert!(text.contains("chat-fast"));
    assert!(text.contains("coding-bulk"));
    assert!(text.contains("coding-fast"));

    // -----------------------------------------------------------------------
    // (e): the admission's stage set is empty — no trace line, no event row.
    // (Same rig as (a)/(b): the two admitted calls above are the ones whose
    // absence this limb proves; the store is read after `serve` stops.)
    // -----------------------------------------------------------------------
    assert_eq!(
        trace_records(&dir_a.join("state/traces")).len(),
        0,
        "the admitted arm writes no trace record at all"
    );
    serve_a.abort();
    let _ = serve_a.await;

    let store = vadis_store::SqliteStore::open_read_only(&dir_a.join("state/vadis.db"))
        .expect("after exit the store opens read-only");
    use vadis_core::store::{Query, QueryRow, Store as _};
    let QueryRow::Events(rows) = store.query(Query::AllEvents).expect("the log reads") else {
        panic!("AllEvents answers events");
    };
    let pipeline = [
        "request.received",
        "decision.made",
        "transform.applied",
        "session.bound",
        "upstream.submitted",
        "upstream.responded",
        "error.classified",
        "failover.triggered",
        "cost.computed",
        "quota.charged",
    ];
    for row in &rows {
        assert!(
            !pipeline.contains(&row.kind_raw.as_str()),
            "the endpoint is not on the pipeline; it wrote a `{}` row",
            row.kind_raw
        );
    }
    let boot_rows = rows
        .iter()
        .filter(|r| r.kind_raw == "config.applied")
        .count();
    assert_eq!(
        boot_rows, 1,
        "the boot's one config.applied row, and no more"
    );
    assert_eq!(
        rows.len(),
        boot_rows,
        "the event log holds nothing but the boot row: {rows:?}"
    );

    // -----------------------------------------------------------------------
    // (c): the guard's two refusal arms — §8's body, the header, and exactly
    // one pre-pipeline record per refusal with this surface's own word.
    // -----------------------------------------------------------------------
    let (dir_c, addr_c, serve_c) = rig(
        "conf107-l3",
        "CONF107_L3_TOKEN",
        "tok-conf107-l3",
        THREE_ALIASES,
    )
    .await;
    let trace_c = dir_c.join("state/traces");

    // No credential at all.
    let (status, body, headers) = http_req(&addr_c, "GET", "/v1/models", &[]);
    assert_eq!(status, 401, "no token must be refused, got {status}");
    let v: serde_json::Value = serde_json::from_slice(&body).expect("§8's body is JSON");
    assert_eq!(v["error"]["type"], "unauthorized");
    assert_eq!(v["error"]["details"]["header"], serde_json::Value::Null);
    assert!(
        header(&headers, "x-vadis-request-id").is_some(),
        "§8's \"always\" has no exception on a refusal"
    );

    // A credential that does not match.
    let (status, body, _h) = http_req(
        &addr_c,
        "GET",
        "/v1/models",
        &[(
            "authorization".to_string(),
            "Bearer tok-conf107-l3-wrong".to_string(),
        )],
    );
    assert_eq!(status, 401, "a wrong token must be refused, got {status}");
    let v: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(v["error"]["type"], "unauthorized");
    assert_eq!(v["error"]["details"]["header"], "authorization");

    let records = trace_records(&trace_c);
    assert_eq!(records.len(), 2, "one record per refused call");
    for rec in &records {
        assert_eq!(
            rec["protocol"]["protocol_in"], "models",
            "the refusal carries the endpoint's own word (spec §4.7)"
        );
        assert_eq!(rec["errors"][0]["kind"], "unauthorized");
        assert_eq!(rec["result"]["status"], 401);
    }

    // The admitted arm still answers afterwards — the refusals poisoned nothing.
    let (status, _b, _h) = models_raw(&addr_c, "tok-conf107-l3");
    assert_eq!(status, 200);
    serve_c.abort();
    let _ = serve_c.await;

    // -----------------------------------------------------------------------
    // (d): the empty roster — `200` with `data: []`, never 404 and never 500.
    // -----------------------------------------------------------------------
    let (_dir_d, addr_d, serve_d) = rig(
        "conf107-l4",
        "CONF107_L4_TOKEN",
        "tok-conf107-l4",
        "aliases: {}",
    )
    .await;

    let (status, raw, _h) = models_raw(&addr_d, "tok-conf107-l4");
    assert_eq!(
        status, 200,
        "an empty roster is a legal configuration, got {status}"
    );
    let body: serde_json::Value = serde_json::from_slice(&raw).expect("json");
    let obj = body.as_object().expect("an object");
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, vec!["data", "object"], "exactly the two keys");
    assert_eq!(body["object"], "list");
    assert_eq!(
        body["data"],
        serde_json::json!([]),
        "data is an empty array, not null and not absent"
    );
    serve_d.abort();
    let _ = serve_d.await;

    // -----------------------------------------------------------------------
    // (f): the method face — `405` admitted with no record; `401`
    // unauthenticated (auth precedes method dispatch, stated not implied).
    // -----------------------------------------------------------------------
    let (dir_f, addr_f, serve_f) = rig(
        "conf107-l6",
        "CONF107_L6_TOKEN",
        "tok-conf107-l6",
        THREE_ALIASES,
    )
    .await;
    let trace_f = dir_f.join("state/traces");

    // Admitted POST: the method router answers, nothing is recorded.
    let (status, body, headers) =
        http_req(&addr_f, "POST", "/v1/models", &bearer("tok-conf107-l6"));
    assert_eq!(
        status, 405,
        "GET-only, so an admitted POST is 405, got {status}"
    );
    let allow = header(&headers, "allow").unwrap_or("");
    assert!(
        allow
            .split(',')
            .any(|m| m.trim().eq_ignore_ascii_case("GET")),
        "the 405 names the method it serves (allow: {allow:?})"
    );
    assert!(
        serde_json::from_slice::<serde_json::Value>(&body).is_err(),
        "the method face is not §8's error body"
    );
    assert_eq!(
        trace_records(&trace_f).len(),
        0,
        "an admitted non-GET writes no record"
    );

    // Unauthenticated POST: the guard decides first, as the contract states.
    let (status, body, _h) = http_req(&addr_f, "POST", "/v1/models", &[]);
    assert_eq!(
        status, 401,
        "auth precedes method dispatch on this route, got {status}"
    );
    let v: serde_json::Value = serde_json::from_slice(&body).expect("§8's body");
    assert_eq!(v["error"]["type"], "unauthorized");
    let records = trace_records(&trace_f);
    assert_eq!(records.len(), 1, "exactly the refusal's one record");
    assert_eq!(records[0]["protocol"]["protocol_in"], "models");
    serve_f.abort();
    let _ = serve_f.await;

    // -----------------------------------------------------------------------
    // (g): the reload — the list follows the new revision, with no restart.
    // -----------------------------------------------------------------------
    let (dir_g, addr_g, serve_g) = rig(
        "conf107-l7",
        "CONF107_L7_TOKEN",
        "tok-conf107-l7",
        THREE_ALIASES,
    )
    .await;

    let before = health_digest(&addr_g).expect("the digest is in-band");
    let (status, raw, _h) = models_raw(&addr_g, "tok-conf107-l7");
    assert_eq!(status, 200);
    let body: serde_json::Value = serde_json::from_slice(&raw).expect("json");
    assert_eq!(body["data"].as_array().map(Vec::len), Some(3));

    // Rewrite the aliases block: one alias. Same process, same port.
    let listen_port: u16 = addr_g
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .expect("the rig's own port");
    std::fs::write(
        dir_g.join("config.yaml"),
        config_yaml(listen_port, "CONF107_L7_TOKEN", ONE_ALIAS),
    )
    .unwrap();

    // Wait for the accepted revision (the digest moved) — CONF-96's
    // technique, with a window that survives a loaded machine: 60s at
    // 250ms polling (the architect measured an 18s reload under load;
    // 60s is ~3x that worst case, and still bounded).
    let mut moved = false;
    for _ in 0..240 {
        match health_digest(&addr_g) {
            Some(now) if now != before => {
                moved = true;
                break;
            }
            _ => std::thread::sleep(std::time::Duration::from_millis(250)),
        }
    }
    assert!(moved, "the reload was accepted on the running process");

    let (status, raw, _h) = models_raw(&addr_g, "tok-conf107-l7");
    assert_eq!(status, 200, "the same process answers the new revision");
    let body: serde_json::Value = serde_json::from_slice(&raw).expect("json");
    let ids: Vec<&str> = body["data"]
        .as_array()
        .expect("data[]")
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        vec!["solo"],
        "the list follows the reloaded aliases — no restart, no stale map"
    );
    serve_g.abort();
    let _ = serve_g.await;
}
