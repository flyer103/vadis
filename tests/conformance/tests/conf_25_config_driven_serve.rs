//! CONF-25 (§12.8): **config-driven `serve`** — the listen address, the
//! plugin set and the roster come from the config file; a config naming
//! another address and another plugin set is what the process actually uses
//! (`/health` reports the configured set, and the configured address is
//! where it listens), with no hardcoded default surviving in the serving
//! path.
//!
//! This case drives `vadis_cli::serve` — the same assembly `vadis serve`
//! runs — binds two distinct addresses with two distinct configs, and talks
//! real HTTP over the sockets. No network egress: only loopback.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::Duration;

/// A config that differs from every hardcoded default on every axis that
/// matters: another listen port, another plugin set (incl. a disabled
/// tier-B entry), another roster.
const CONFIG: &str = r#"
server:   { addr: "127.0.0.1:39711", upstream_attempt_timeout: 45s, request_timeout: 9m }
session:  { key_sources: ["prompt_cache_key", "header:session-id"], ttl: 11h }
cache:    { sticky: true, breakeven: { enabled: true, min_remaining_turns: 2, safety_factor: 1.1 } }
trace:    { dir: "./state/traces", rollover: hourly }

providers:
  - name: only-provider
    urls:
      chat: https://only.example/v1/chat/completions
    api_key_env: CONF25_ONLY_KEY
    wire_api: chat
    supports: [chat]
    models:
      - id: only-model
        context: 128k
        price:
          input_miss: 0.001
          input_hit: 0.0001
          cache_write: 0.0
          output: 0.002
          peak: { multiplier: 1.0, windows: [] }
        source: "https://only.example/pricing @2026-09-19"

aliases:
  only-alias: only-provider/only-model

plugins:
  - id: cache-guard
    kind: builtin/cache_guard
    config: { strict_prefix: true }
  - id: shadow-judge
    kind: process
    url: unix:///tmp/conf25-judge.sock
    inject: [cache_ledger]
    intercept: { sample: 0.05, shadow: true }
    disabled: true

fallback: [only-provider/only-model]
"#;

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "conf25-{}-{}-{tag}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn http_get(addr: &str, path: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::Interrupted =>
            {
                // Headers may not have arrived yet; retry.
                continue;
            }
            Err(_) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                // Stop once the declared body length has arrived.
                let text = String::from_utf8_lossy(&buf);
                if let Some(headers) = text.split("\r\n\r\n").next() {
                    let len: usize = headers
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse().ok())?
                        })
                        .unwrap_or(0);
                    if text
                        .split("\r\n\r\n")
                        .nth(1)
                        .is_some_and(|b| b.len() >= len)
                        && len > 0
                    {
                        break;
                    }
                }
            }
        }
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    let status: u16 = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("status line");
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}

// Multi-thread runtime: the case does blocking socket reads on the test
// thread while `serve` must keep making progress on another.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_25_config_driven_serve() {
    let dir = tempdir("main");
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, CONFIG).unwrap();

    // The provider's api key env is deliberately NOT set: it must surface
    // as api_key_present=false on /health, not as a startup failure.
    std::env::remove_var("CONF25_ONLY_KEY");

    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });

    // Wait for the configured address to accept connections (max ~10s).
    let addr = "127.0.0.1:39711";
    let mut up = false;
    for _ in 0..200 {
        if TcpStream::connect_timeout(&addr.parse().unwrap(), std::time::Duration::from_millis(50))
            .is_ok()
        {
            up = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(up, "serve never came up on the configured address {addr}");

    // (a) the configured address is where it listens, and /health answers.
    let (status, body) = http_get(addr, "/health");
    assert_eq!(status, 200, "health status: {status}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("health json");

    // (b) /health reports the configured set, not a default:
    //     - the configured addr, not 127.0.0.1:8790
    assert_eq!(v["addr"], "127.0.0.1:39711");
    //     - the configured plugin list: cache-guard active, the tier-B
    //       entry disabled
    let plugins = v["plugins"].as_array().expect("plugins array");
    assert_eq!(plugins.len(), 2, "plugin set must come from the config");
    let ids: Vec<&str> = plugins.iter().map(|p| p["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["cache-guard", "shadow-judge"]);
    assert!(
        plugins[1]["disabled"].as_bool().unwrap(),
        "a disabled plugin is shown as disabled, not missing"
    );
    assert!(plugins[0].get("disabled").is_none());

    //     - the roster: exactly the configured provider, with its key
    //       presence (env unset → false) reported
    let providers = v["providers"].as_array().expect("providers array");
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0]["name"], "only-provider");
    assert_eq!(providers[0]["api_key_env"], "CONF25_ONLY_KEY");
    assert_eq!(providers[0]["api_key_present"], false);
    assert_eq!(providers[0]["available"], false);

    //     - the resolved paths, anchored at the config file's directory
    //     (spec §4.1 / §4.5), with the store open (the store is a
    //     startup prerequisite — a running process opened it, CONF-23)
    let trace_dir = v["trace_dir"].as_str().unwrap();
    assert!(
        trace_dir.replace('\\', "/").ends_with("state/traces"),
        "trace_dir resolved against the config dir: {trace_dir}"
    );
    let state_db = v["state_db"].as_str().unwrap();
    assert!(
        state_db.replace('\\', "/").ends_with("state/vadis.db"),
        "state_db fixed at <config dir>/state/vadis.db: {state_db}"
    );
    assert_eq!(v["store"], "open");

    // (c) no hardcoded default survived: the old stub default 8790 must be
    //     dead. (If serve still hardcoded it, the connect above to 39711
    //     would have failed and we would not be here.)
    let default_addr = "127.0.0.1:8790";
    assert!(
        TcpStream::connect_timeout(&default_addr.parse().unwrap(), Duration::from_millis(300))
            .is_err()
            || *default_addr == *addr,
        "the old hardcoded default listen address must not be serving"
    );

    serve_task.abort();
    let _ = serve_task.await;
}
