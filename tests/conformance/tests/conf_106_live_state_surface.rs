//! CONF-106 (spec §4.18 + §9.2's live-gateway fallback + §12.10.8's
//! bounded-use rule, ADR-054, DESIGN §12.24): **the live event-log
//! figure — the one a second process cannot read while `serve` holds
//! the store — answered by the writer itself, and the fallback that
//! reaches it.** Five limbs, through the real `serve` assembly over
//! loopback HTTP:
//!
//! - (a) **the endpoint exists and counts independently** — `GET
//!   /state/events?window=15m` with the guard token answers `200` with
//!   exactly the two keys of spec §4.18's body, stable across calls.
//! - (b) **the status set is `{200, 400, 401}`** — no token ⇒ the
//!   guard's `401` (§8's body, `X-Vadis-Request-Id`, exactly one
//!   pre-pipeline record with `protocol_in: "state"`); an unusable
//!   `window` ⇒ `400` with §8's body (`error.type = invalid_request`,
//!   `details.param = "window"`) and **no trace record** — and no store
//!   read, a property this real assembly cannot observe directly (a
//!   live `serve` always holds a store) and which is therefore
//!   witnessed by `state_read.rs`'s own unit test
//!   `an_unusable_window_never_touches_the_store`: an assembly with NO
//!   store at all still answers the `400` arm (`Err("window")` before
//!   `Err("no store")`). Conformance proves the observable half: the
//!   `400`s arrive and write nothing. (An unreadable store ⇒ an honest
//!   `500`, outside the healthy-store status set by design.)
//! - (c) **the window bound `1ms..=24h`** — both bounds admitted;
//!   `0ms`, `24h1ms`, `abc` and an absent parameter all refused `400`.
//! - (d) **the stats fallback** — a request whose bytes the upstream
//!   swallowed (a listener that accepts and never answers) leaves an
//!   `upstream.submitted` with no `upstream.responded`; while `serve`
//!   still holds the store, `vadis stats`'s local read-only open is
//!   refused, the live-gateway fallback fires, and the figure it
//!   prints is the endpoint's own answer for the same window — and,
//!   after `serve` exits, the direct store read's own count. One
//!   figure, three readers, zero disagreement.
//! - (e) **the read is windowed, never a log scan** — the endpoint's
//!   `window.from` is exactly `window.to − window` and `window.to` is
//!   the call's own clock read, proving the window bounded the read.
//!   The never-`Query::AllEvents` claim is enforced structurally by
//!   `state_read.rs`'s unit tests (`RowsStore` panics on `AllEvents`;
//!   see its `panic_on_all_events` flag) — a store double cannot be
//!   injected into the real assembly, so conformance proves the real
//!   assembly answers and the unit tests prove which query it used.
//!
//! Offline: a swallowing loopback listener and canned mock upstreams
//! only — no provider is dialled, no credential is read.

#![forbid(unsafe_code)]

use vadis_conformance::testkit;

// ---------------------------------------------------------------------------
// The rig: conf_87/conf_46's shape — hand-written config, the real
// `vadis_cli::serve` in-process, hand-written TCP.
// ---------------------------------------------------------------------------

/// The guarded config. `request_timeout` is per-rig: the swallow limb
/// needs a short one so the unanswered attempt terminates inside the
/// test's budget; the trafficless limbs keep the fixture default.
fn config_yaml(
    upstream_port: u16,
    listen_port: u16,
    request_timeout: &str,
    token_env: &str,
) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 1s, request_timeout: {request_timeout}, auth_token_env: {token_env} }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: mock
    urls:
      chat: http://127.0.0.1:{upstream_port}/v1/chat/completions
    api_key_env: CONF106_MOCK_KEY
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
plugins: []
fallback: []
"#
    )
}

/// One rig: tempdir + config written + the real serve spawned. Returns
/// (dir, listen_addr, serve_task). The caller sets the token env first.
async fn rig(
    tag: &str,
    upstream_port: u16,
    request_timeout: &str,
    token_env: &str,
    token: &str,
) -> (std::path::PathBuf, String, tokio::task::JoinHandle<i32>) {
    let dir = testkit::tempdir(tag);
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    std::fs::write(
        dir.join("config.yaml"),
        config_yaml(upstream_port, listen_port, request_timeout, token_env),
    )
    .unwrap();
    std::env::set_var("CONF106_MOCK_KEY", "sk-conf106");
    std::env::set_var(token_env, token);
    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    (dir, listen_addr, serve_task)
}

/// A GET over raw TCP with arbitrary headers — conf_46's reader, with
/// the path parameterized for the query-string limbs.
fn http_get(
    addr: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> (u16, Vec<u8>, Vec<(String, String)>) {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(addr).expect("connect");
    let mut req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
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

fn bearer(token: &str) -> Vec<(&'static str, String)> {
    vec![("authorization", format!("Bearer {token}"))]
}

/// `GET /state/events?window=<w>` with the token, parsed:
/// (status, json, response headers). The headers ride along so an
/// assertion about §8's formatter reads the response of THIS call —
/// never a neighbouring call's binding.
fn state_events(
    addr: &str,
    window: &str,
    token: &str,
) -> (u16, serde_json::Value, Vec<(String, String)>) {
    let headers = bearer(token)
        .into_iter()
        .map(|(k, v)| (k, Box::leak(v.into_boxed_str()) as &'static str))
        .collect::<Vec<_>>();
    let (status, body, h) = http_get(addr, &format!("/state/events?window={window}"), &headers);
    let v = serde_json::from_slice(&body).expect("the body is JSON on every arm");
    (status, v, h)
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

/// RFC3339 UTC with millis (`2026-10-07T10:21:09.574Z`) → epoch ms,
/// test-side over vadis-core's own calendar helper (conf_87's method).
fn parse_rfc3339_ms(s: &str) -> i64 {
    let (date, rest) = s.split_once('T').expect("RFC3339 T");
    let mut d = date.split('-');
    let y: i64 = d.next().unwrap().parse().unwrap();
    let mo: u32 = d.next().unwrap().parse().unwrap();
    let day: u32 = d.next().unwrap().parse().unwrap();
    let core = rest.strip_suffix('Z').unwrap_or(rest);
    let (time, frac) = core.split_once('.').unwrap_or((core, "0"));
    let mut t = time.split(':');
    let h: i64 = t.next().unwrap().parse().unwrap();
    let mi: i64 = t.next().unwrap().parse().unwrap();
    let sec: i64 = t.next().unwrap().parse().unwrap();
    let milli: i64 = frac.parse().unwrap_or(0);
    vadis_core::peak::utc_midnight_epoch(y, mo, day) as i64 * 1_000
        + (h * 3_600 + mi * 60 + sec) * 1_000
        + milli
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// (a): the endpoint exists, is guarded-admitted, and its body is
// exactly spec §4.18's two keys — stable across calls.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_106_a_the_endpoint_answers_the_two_key_body() {
    // Upstream port 9 (discard): this limb drives no traffic.
    let (_dir, addr, serve_task) =
        rig("conf106-l1", 9, "30s", "CONF106_L1_TOKEN", "tok-conf106-l1").await;

    // The liveness control (CONF-46's): the same run answers /health
    // with no token — a refusal below is the guard's, not a dead
    // socket's.
    let (status, _b, _h) = http_get(&addr, "/health", &[]);
    assert_eq!(status, 200, "control: the assembly is up");

    // The admitted GET, read raw for the header limbs below.
    let hdrs = bearer("tok-conf106-l1")
        .into_iter()
        .map(|(k, v)| (k, Box::leak(v.into_boxed_str()) as &'static str))
        .collect::<Vec<_>>();
    let (status, raw, headers) = http_get(&addr, "/state/events?window=15m", &hdrs);
    assert_eq!(status, 200, "the admitted GET answers 200, got {status}");
    assert_eq!(
        header(&headers, "content-type"),
        Some("application/json"),
        "spec §4.18's success body is JSON"
    );
    assert!(
        header(&headers, "x-vadis-request-id").is_none(),
        "the admitted arm never went through §8's error formatter"
    );
    let body: serde_json::Value = serde_json::from_slice(&raw).expect("the body is JSON");

    // Exactly the two keys — anything else is a contract change, not an
    // extension (spec §4.18's export boundary). Compared as a set: the
    // wire's key order is the serializer's, not the contract's.
    let obj = body.as_object().expect("an object");
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["unknown_outcome_requests", "window"],
        "the body is exactly the two keys, got {obj:?}"
    );
    let w = body["window"].as_object().expect("window object");
    let mut wkeys: Vec<&str> = w.keys().map(String::as_str).collect();
    wkeys.sort_unstable();
    assert_eq!(wkeys, vec!["from", "to"], "window is exactly {{from, to}}");
    assert!(body["window"]["from"].is_string());
    assert!(body["window"]["to"].is_string());
    assert!(
        body["unknown_outcome_requests"].is_u64(),
        "the figure is a number, got {}",
        body["unknown_outcome_requests"]
    );

    // Stable across calls over an unchanged log (spec §4.18's
    // determinism clause): the figure is the same number.
    let (status2, body2, _h) = state_events(&addr, "15m", "tok-conf106-l1");
    assert_eq!(status2, 200);
    assert_eq!(
        body2["unknown_outcome_requests"], body["unknown_outcome_requests"],
        "two calls over an unchanged log agree on the figure"
    );

    serve_task.abort();
    let _ = serve_task.await;
}

// ---------------------------------------------------------------------------
// (b): the status set — the guard's 401 (with its one `protocol_in:
// "state"` record) and the 400 arm (§8's body, no record written, no
// store read — the read half witnessed by state_read.rs's own unit
// test, cited in the module doc).
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_106_b_status_set_guard_401_and_window_400() {
    let (dir, addr, serve_task) =
        rig("conf106-l2", 9, "30s", "CONF106_L2_TOKEN", "tok-conf106-l2").await;
    let trace_dir = dir.join("state/traces");

    // -- the guard's two refusal arms --------------------------------
    let (status, body, headers) = http_get(&addr, "/state/events?window=15m", &[]);
    assert_eq!(status, 401, "no token must be refused, got {status}");
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["error"]["type"], "unauthorized");
    assert_eq!(v["error"]["details"]["header"], serde_json::Value::Null);
    assert!(
        header(&headers, "x-vadis-request-id").is_some(),
        "a refusal went through §8's formatter"
    );

    let (status, body, _h) = http_get(
        &addr,
        "/state/events?window=15m",
        &[("authorization", "Bearer tok-conf106-l2-wrong")],
    );
    assert_eq!(status, 401, "a wrong token must be refused, got {status}");
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["error"]["type"], "unauthorized");
    assert_eq!(v["error"]["details"]["header"], "authorization");

    // Each refused arm wrote exactly one record — the guard's boundary
    // class, with THIS surface's own protocol word (DESIGN §12.24).
    let records = trace_records(&trace_dir);
    assert_eq!(records.len(), 2, "one record per refused call");
    for rec in &records {
        assert_eq!(rec["protocol"]["protocol_in"], "state");
        assert_eq!(rec["errors"][0]["kind"], "unauthorized");
        assert_eq!(rec["result"]["status"], 401);
    }

    // -- the 400 arm --------------------------------------------------
    // The admitted token, three unusable windows: out of bounds above,
    // garbage, and the grammar's missing unit. All §8 bodies naming the
    // parameter; none writes a record; none reads the store (the
    // discrimination is state_read.rs's `an_unusable_window_...` unit
    // test — a live serve always holds a store, so the observable here
    // is the answer and the silence).
    for bad in ["25h", "abc", "15"] {
        let (status, body, h) = state_events(&addr, bad, "tok-conf106-l2");
        assert_eq!(
            status, 400,
            "window={bad} must be refused 400, got {status}"
        );
        assert_eq!(body["error"]["type"], "invalid_request", "window={bad}");
        assert_eq!(body["error"]["details"]["param"], "window", "window={bad}");
        assert!(
            header(&h, "x-vadis-request-id").is_some(),
            "the 400 arm went through §8's formatter (window={bad})"
        );
    }

    // Absent parameter: same 400, same shape.
    let headers = bearer("tok-conf106-l2")
        .into_iter()
        .map(|(k, v)| (k, Box::leak(v.into_boxed_str()) as &'static str))
        .collect::<Vec<_>>();
    let (status, body, _h) = http_get(&addr, "/state/events", &headers);
    assert_eq!(
        status, 400,
        "an absent window must be refused 400, got {status}"
    );
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["error"]["type"], "invalid_request");
    assert_eq!(v["error"]["details"]["param"], "window");

    // The 400 arm wrote nothing: still exactly the two refusal records.
    assert_eq!(
        trace_records(&trace_dir).len(),
        2,
        "the 400 arm adds no record"
    );

    // And the surface still answers 200 afterwards — the 400s poisoned
    // nothing.
    let (status, _b, _h) = state_events(&addr, "15m", "tok-conf106-l2");
    assert_eq!(status, 200);

    serve_task.abort();
    let _ = serve_task.await;
}

// ---------------------------------------------------------------------------
// (c): the window bound — `1ms` and `24h` admitted, everything outside
// (or unparsable, or absent — limb (b) above) refused.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_106_c_window_bounds_1ms_to_24h() {
    let (_dir, addr, serve_task) =
        rig("conf106-l3", 9, "30s", "CONF106_L3_TOKEN", "tok-conf106-l3").await;

    for good in ["1ms", "24h", "15m", "1h30m"] {
        let (status, body, _h) = state_events(&addr, good, "tok-conf106-l3");
        assert_eq!(status, 200, "window={good} is in bound, got {status}");
        assert!(
            body["unknown_outcome_requests"].is_u64(),
            "window={good}: the figure is a number"
        );
    }
    for bad in ["0ms", "24h1ms", "1d", ""] {
        let (status, body, _h) = state_events(&addr, bad, "tok-conf106-l3");
        assert_eq!(
            status, 400,
            "window={bad:?} is out of bound or unparsable, got {status}"
        );
        assert_eq!(body["error"]["type"], "invalid_request", "window={bad:?}");
    }

    serve_task.abort();
    let _ = serve_task.await;
}

// ---------------------------------------------------------------------------
// (d): the round's point — the swallowed request leaves an unanswered
// intent; while serve holds the store, `vadis stats` gets the figure
// from the gateway and it equals both the endpoint's answer and, after
// exit, the direct store read.
// ---------------------------------------------------------------------------

/// A loopback listener that accepts connections, swallows the bytes and
/// never answers — the upstream that creates exactly one
/// `upstream.submitted` with no `upstream.responded`. The sockets are
/// held open until the task is dropped, so the upstream never closes
/// first (a close would be an EOF answer, not a swallow).
async fn swallow_upstream() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held: Vec<tokio::net::TcpStream> = Vec::new();
        loop {
            match listener.accept().await {
                Ok((stream, _)) => held.push(stream),
                Err(_) => return,
            }
        }
    });
    addr
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_106_d_stats_fallback_equals_the_endpoint_and_the_store() {
    let upstream = swallow_upstream().await;
    // request_timeout 2s: the swallowed attempt terminates the request
    // inside the test's budget, as a transport timeout — no response
    // ever arrives, so no `upstream.responded` row is ever written.
    let (dir, addr, serve_task) = rig(
        "conf106-l4",
        upstream.port(),
        "2s",
        "CONF106_L4_TOKEN",
        "tok-conf106-l4",
    )
    .await;

    // Drive one request into the swallow. The guard covers the protocol
    // routes too, so the POST carries the token.
    let body = r#"{"model":"mock/glm","messages":[{"role":"user","content":"swallow me"}],"prompt_cache_key":"conf106-l4","stream":false}"#;
    let t0 = std::time::Instant::now();
    let (status, _b, _h) = testkit::http_post(
        &addr,
        "/v1/chat/completions",
        body.as_bytes(),
        &[("authorization", "Bearer tok-conf106-l4")],
    );
    let elapsed = t0.elapsed();
    assert!(
        (500..=504).contains(&status),
        "the swallowed attempt surfaces as a 5xx, got {status}"
    );
    assert!(
        elapsed >= std::time::Duration::from_millis(900),
        "the failure is the attempt timeout, not an instant refusal ({elapsed:?})"
    );

    // The endpoint's own answer: at least one unanswered intent.
    let (status, endpoint_body, _h) = state_events(&addr, "15m", "tok-conf106-l4");
    assert_eq!(status, 200);
    let endpoint_figure = endpoint_body["unknown_outcome_requests"].as_u64().unwrap();
    assert!(
        endpoint_figure >= 1,
        "the swallowed request is an unanswered intent, got {endpoint_figure}"
    );

    // `vadis stats`, in-process, while serve STILL holds the store: the
    // local read-only open is refused, the fallback fires.
    let cfg_path = dir.join("config.yaml");
    let rep = vadis_cli::stats::report(&cfg_path.to_string_lossy(), "15m").expect("report");
    assert!(
        rep.events.log_was_read,
        "the figure is present — the fallback served it, not the local open"
    );
    assert_eq!(rep.events.source, "gateway", "the source is in-band");
    assert_eq!(
        rep.events.unknown_outcome_requests, endpoint_figure,
        "the gateway figure IS the endpoint's answer for the same window"
    );

    // The json shape: the figure and its source ride beside each other
    // (spec §9.2). The text mode's `(gateway)` word is witnessed by
    // stats.rs's own unit tests (`text_names_the_gateway_source_in_band`).
    let rc = vadis_cli::config_load::load(&cfg_path).expect("config reloads");
    let json = vadis_cli::stats::report_json(&rc, "15m", &rep, &None);
    assert_eq!(json["events"]["source"], "gateway");
    assert_eq!(
        json["unknown_outcome_requests"].as_u64().unwrap(),
        endpoint_figure
    );

    // The red control: with the token variable removed from the
    // environment the same call cannot reach the gateway, and the
    // figure is honestly omitted — proving the presence above was the
    // fallback's doing, never a lucky local read.
    std::env::remove_var("CONF106_L4_TOKEN");
    let rep2 = vadis_cli::stats::report(&cfg_path.to_string_lossy(), "15m").expect("report");
    assert!(
        !rep2.events.log_was_read,
        "no token ⇒ no fallback ⇒ the figure is omitted, never guessed"
    );
    assert_ne!(rep2.events.source, "gateway");

    // Stop serve, then read the store directly — the third reader. The
    // figure must be the same number: one derivation, three readers.
    serve_task.abort();
    let _ = serve_task.await;
    let store = vadis_store::SqliteStore::open_read_only(&dir.join("state/vadis.db"))
        .expect("after exit the store opens read-only");
    use vadis_core::store::{Query, QueryRow, Store as _};
    let QueryRow::Events(rows) = store.query(Query::AllEvents).expect("the log reads") else {
        panic!("AllEvents answers events");
    };
    let submitted: Vec<&str> = rows
        .iter()
        .filter(|e| e.kind_raw == "upstream.submitted")
        .filter_map(|e| e.request_id.as_deref())
        .collect();
    assert!(!submitted.is_empty(), "the swallow wrote its intent row");
    let answered: std::collections::HashSet<&str> = rows
        .iter()
        .filter(|e| e.kind_raw == "upstream.responded")
        .filter_map(|e| e.request_id.as_deref())
        .collect();
    let direct_figure = submitted
        .iter()
        .filter(|rid| !answered.contains(*rid))
        .count() as u64;
    assert_eq!(
        direct_figure, endpoint_figure,
        "the direct store count equals the endpoint's and the gateway's"
    );
}

// ---------------------------------------------------------------------------
// (e): the window bounded the read — `window.from` is exactly
// `window.to − window`, `to` is the call's own clock read, and the
// never-AllEvents claim is state_read.rs's unit tests' (RowsStore
// panics on AllEvents); this limb proves the real assembly's own
// bounds move with the parameter.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_106_e_the_window_bounds_the_read() {
    let (_dir, addr, serve_task) =
        rig("conf106-l5", 9, "30s", "CONF106_L5_TOKEN", "tok-conf106-l5").await;

    for (window, window_ms) in [("15m", 900_000_i64), ("1h", 3_600_000), ("24h", 86_400_000)] {
        let before = now_ms();
        let (status, body, _h) = state_events(&addr, window, "tok-conf106-l5");
        let after = now_ms();
        assert_eq!(status, 200, "window={window}");
        let to_ms = parse_rfc3339_ms(body["window"]["to"].as_str().unwrap());
        let from_ms = parse_rfc3339_ms(body["window"]["from"].as_str().unwrap());
        assert!(
            before <= to_ms && to_ms <= after,
            "window={window}: `to` is the call's own clock read ({before}..={after}, got {to_ms})"
        );
        assert_eq!(
            from_ms,
            to_ms - window_ms,
            "window={window}: `from` is exactly `to` − window — the parameter bounded the read"
        );
    }

    serve_task.abort();
    let _ = serve_task.await;
}
