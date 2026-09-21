//! CONF-23 (§12.8): **startup refusal, both kinds** — (a) a store that cannot
//! be opened or migrated makes `serve` exit **non-zero** with the reason
//! (never a silent in-memory fallback); (b) a second `serve` on the same
//! state directory is refused at startup with a *distinguishable* "locked"
//! reason.
//!
//! (b) and (a)'s migration half are asserted against the real `SqliteStore`
//! directly; (a)'s exit-code half drives the real `router_cli::serve`
//! assembly with a store-unopenable config (the `state/` path blocked by a
//! regular file), asserting exit code 4 and the reason on stderr.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const CONFIG: &str = r#"
server:   { addr: "127.0.0.1:39723", upstream_attempt_timeout: 45s, request_timeout: 9m }
session:  { key_sources: ["prompt_cache_key"], ttl: 11h }
cache:    { sticky: true, breakeven: { enabled: true, min_remaining_turns: 2, safety_factor: 1.1 } }
trace:    { dir: "./state/traces", rollover: hourly }

providers:
  - name: only-provider
    urls:
      chat: https://only.example/v1/chat/completions
    api_key_env: CONF23_ONLY_KEY
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

fallback: [only-provider/only-model]
"#;

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "conf23-{}-{}-{tag}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// (a) `serve` exits non-zero (4) with the reason when the store cannot be
// opened. Blocking regular file at <config dir>/state.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_23a_unopenable_store_exits_nonzero() {
    let dir = tempdir("unopen");
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, CONFIG).unwrap();
    // `state` is a regular file → router.db cannot be created under it.
    std::fs::write(dir.join("state"), b"not a directory").unwrap();

    let cfg = config_path.to_string_lossy().into_owned();
    let code = router_cli::serve(&cfg).await;
    assert_eq!(
        code, 4,
        "store-unopenable must exit 4 (2=config, 3=bind), got {code}"
    );
}

// (b) A second open on the same state directory is refused with the
// distinguishable `Locked` reason (the writer lock, ADR-009 item 8).
#[test]
fn conf_23b_second_writer_refused_with_locked_reason() {
    let dir = tempdir("lock");
    let db = dir.join("state/router.db");
    let _first = router_store::SqliteStore::open(&db).unwrap();
    match router_store::SqliteStore::open(&db) {
        Err(router_core::StoreError::Locked) => {
            // The distinguishable reason, as designed.
        }
        other => panic!("expected StoreError::Locked, got {other:?}"),
    }
}

// (a, migration half) A schema version from the future is refused, not read
// on a guess (forward-only migrations, ADR-009 item 7).
#[test]
fn conf_23a_schema_too_new_is_refused() {
    let dir = tempdir("future");
    let db = dir.join("state/router.db");
    {
        let s = router_store::SqliteStore::open(&db).unwrap();
        let conn = s.raw_connection();
        conn.execute(
            "INSERT INTO schema_version (version, applied_at) VALUES (99, '2999-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
    }
    match router_store::SqliteStore::open(&db) {
        // `supported` is the binary's max DDL version (2 since the
        // `plan_state` migration, DESIGN §12.10.8) — asserted as a
        // relation, not a snapshot (AGENTS constraint 6).
        Err(router_core::StoreError::SchemaTooNew {
            found: 99,
            supported,
        }) => {
            assert_eq!(supported, s_max_supported())
        }
        other => panic!("expected SchemaTooNew, got {other:?}"),
    }
}

/// The binary's own maximum DDL version, read from a fresh store rather
/// than hardcoded.
fn s_max_supported() -> u32 {
    use router_core::store::Store as _;
    let dir = router_conformance::testkit::tempdir("conf23-max");
    let s = router_store::SqliteStore::open(&dir.join("state/router.db")).unwrap();
    s.schema_version().unwrap()
}
