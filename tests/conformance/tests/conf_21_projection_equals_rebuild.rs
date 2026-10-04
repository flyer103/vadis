//! CONF-21 (§12.8): **projection == rebuild** — running the same request set
//! twice, once projecting incrementally and once rebuilding from `events`,
//! yields row-by-row identical `sessions` / `cache_ledger` / `quota_counters`
//! / `provider_cooldown` contents.
//!
//! Two fresh stores, one identical event sequence each (one request with a
//! session binding, a prefix-block set, a quota charge and a demotion), then
//! a four-projection comparison through the public surface. `expires_at_us`
//! is compared within one store (rebuild of the incremental store must be a
//! no-op), because it anchors on each store's own event timestamps.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use vadis_core::store::{EventKind, NewEvent, Projection, ProjectionWrite, Store};
use serde_json::json;

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "conf21-{}-{}-{tag}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn open(tag: &str) -> vadis_store::SqliteStore {
    vadis_store::SqliteStore::open(&tempdir(tag).join("state/router.db")).unwrap()
}

/// The scenario's event sequence — identical for both stores. The intent
/// carries `prefix_blocks` because the encoder computed them from exactly
/// the outbound bytes (§12.10.6), so the log alone determines the ledger.
fn scenario(
    s: &vadis_store::SqliteStore,
) -> (
    vadis_core::EventId,
    vadis_core::EventId,
    vadis_core::EventId,
    vadis_core::EventId,
) {
    s.append(NewEvent {
        kind: EventKind::RequestReceived,
        request_id: Some("req-1"),
        session: Some("sess-1"),
        body_hash: Some("aaaaaaaaaaaaaaaa"),
        trace_ref: None,
        payload: json!({"protocol_in": "chat", "turn_index": 1}),
    })
    .unwrap();
    let e2 = s
        .append(NewEvent {
            kind: EventKind::SessionBound,
            request_id: Some("req-1"),
            session: Some("sess-1"),
            body_hash: None,
            trace_ref: None,
            payload: json!({"session_key": "sess-1", "provider": "zai", "model": "glm-5.3", "ttl_us": 43_200_000_000_i64}),
        })
        .unwrap();
    let e3 = s
        .append(NewEvent {
            kind: EventKind::UpstreamSubmitted,
            request_id: Some("req-1"),
            session: Some("sess-1"),
            body_hash: Some("0123456789abcdef"),
            trace_ref: None,
            payload: json!({
                "route": "zai/glm-5.3", "attempt_index": 0, "attempt_id": "att-1",
                "prefix_blocks": [
                    {"index": 0, "kind": "system",  "tokens": 120, "hash": "1111111111111111"},
                    {"index": 1, "kind": "message", "tokens": 800, "hash": "2222222222222222"},
                    {"index": 2, "kind": "tool",    "tokens": 300, "hash": "3333333333333333"}
                ]
            }),
        })
        .unwrap();
    let e5 = s
        .append(NewEvent {
            kind: EventKind::QuotaCharged,
            request_id: Some("req-1"),
            session: Some("sess-1"),
            body_hash: None,
            trace_ref: Some("2026-09-19T12.jsonl:42"),
            payload: json!({"provider": "zai", "plan_idx": 0, "window_start_us": 1_767_225_600_000_000_i64, "tokens": 14520}),
        })
        .unwrap();
    let e6 = s
        .append(NewEvent {
            kind: EventKind::ErrorClassified,
            request_id: Some("req-2"),
            session: None,
            body_hash: None,
            trace_ref: None,
            payload: json!({"status": 429, "reason": "rate_limit",
                            "demotion": {"scope": "provider", "provider": "moonshot", "model": "", "until_us": 1_789_830_000_000_000_i64, "reason": "rate_limit"}}),
        })
        .unwrap();
    (e2, e3, e5, e6)
}

#[test]
fn conf_21_projection_equals_rebuild() {
    // Store A: incremental projection on top of the events.
    let a = open("inc");
    let (e2, e3, e5, e6) = scenario(&a);
    let blocks: Vec<(u32, &str, u64, &str)> = vec![
        (0, "system", 120, "1111111111111111"),
        (1, "message", 800, "2222222222222222"),
        (2, "tool", 300, "3333333333333333"),
    ];
    a.project(ProjectionWrite::SessionBound {
        session_key: "sess-1",
        provider: "zai",
        model: "glm-5.3",
        ttl_us: 43_200_000_000,
        last_event: e2,
    })
    .unwrap();
    a.project(ProjectionWrite::CacheLedgerPut {
        session_key: "sess-1",
        blocks: &blocks,
        last_event: e3,
    })
    .unwrap();
    a.project(ProjectionWrite::QuotaCharged {
        provider: "zai",
        plan_idx: 0,
        window_start_us: 1_767_225_600_000_000,
        tokens: 14_520,
        last_event: e5,
    })
    .unwrap();
    a.project(ProjectionWrite::ProviderCooldown {
        scope: "provider",
        provider: "moonshot",
        model: "",
        until_us: 1_789_830_000_000_000,
        reason: "rate_limit",
        last_event: e6,
    })
    .unwrap();

    // Store B: same events, no incremental writes; rebuild from the log.
    let b = open("reb");
    let _ = scenario(&b);
    let stats = b.rebuild(Projection::All).unwrap();
    assert_eq!(stats.events_scanned, 5);

    for p in [
        Projection::Sessions,
        Projection::CacheLedger,
        Projection::QuotaCounters,
        Projection::ProviderCooldown,
    ] {
        // expires_at_us (field 4 of sessions rows) anchors on each store's
        // own event timestamps; it is excluded cross-store and asserted
        // within-store below.
        let strip = |rows: Vec<String>| {
            rows.into_iter()
                .map(|r| {
                    let mut f: Vec<&str> = r.split('|').collect();
                    if f.len() == 6 {
                        f.remove(4);
                    }
                    f.join("|")
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            strip(a.projection_rows(p).unwrap()),
            strip(b.projection_rows(p).unwrap()),
            "projection {p:?}: incremental != rebuild"
        );
    }

    // The strongest form: rebuilding A's own projections is a no-op, row for
    // row, including expires_at_us.
    let before = a.projection_rows(Projection::All).unwrap();
    a.rebuild(Projection::All).unwrap();
    assert_eq!(a.projection_rows(Projection::All).unwrap(), before);
}
