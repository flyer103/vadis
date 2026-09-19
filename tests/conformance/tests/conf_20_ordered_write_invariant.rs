//! CONF-20 (§12.8): **ordered write invariant** — on one request run through
//! the pipeline with a recording store and a fake provider, the event row
//! with the highest `event_id` at the instant the attempt's request bytes are
//! handed to the wire is that attempt's `upstream.submitted` intent; nothing
//! is written between the intent commit and the attempt.
//!
//! The forwarding pipeline lands in R2-2d; this file drives the invariant at
//! the seam it is enforced on: `write_intent_then` (router-core) over the
//! real SQLite store (router-store), with the "fake provider" as the effect
//! closure that inspects the store at hand-off time. The pipeline case's
//! `#[ignore]`d twin is kept below until R2-2d wires the real proxy.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use router_core::store::{write_intent_then, EventKind, NewEvent, Query, QueryRow, Store};
use serde_json::json;

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "conf20-{}-{}-{tag}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn conf_20_ordered_write_invariant() {
    let store = router_store::SqliteStore::open(&tempdir("main").join("state/router.db")).unwrap();

    // The pipeline prefix, exactly DESIGN §12.10.5's rows 1–4.
    store
        .append(NewEvent {
            kind: EventKind::RequestReceived,
            request_id: Some("req-1"),
            session: Some("sess-1"),
            body_hash: Some("aaaaaaaaaaaaaaaa"),
            trace_ref: None,
            payload: json!({"protocol_in": "chat", "turn_index": 1}),
        })
        .unwrap();
    store
        .append(NewEvent {
            kind: EventKind::DecisionMade,
            request_id: Some("req-1"),
            session: Some("sess-1"),
            body_hash: None,
            trace_ref: None,
            payload: json!({"provider": "zai", "model": "glm-5.3", "selection_source": "explicit"}),
        })
        .unwrap();
    let intent_id = store
        .append(NewEvent {
            kind: EventKind::SessionBound,
            request_id: Some("req-1"),
            session: Some("sess-1"),
            body_hash: None,
            trace_ref: None,
            payload: json!({"session_key": "sess-1", "provider": "zai", "model": "glm-5.3", "ttl_us": 43_200_000_000_i64}),
        })
        .unwrap();

    // The fake provider: the effect closure is "the request bytes are handed
    // to the wire". At that instant the log's highest event_id MUST be the
    // intent row this effect is authorized by.
    let (intent_event, (observed_max, ())) = write_intent_then(
        &store,
        NewEvent {
            kind: EventKind::UpstreamSubmitted,
            request_id: Some("req-1"),
            session: Some("sess-1"),
            body_hash: Some("0123456789abcdef"),
            trace_ref: None,
            payload: json!({"route": "zai/glm-5.3", "attempt_index": 0, "attempt_id": "att-1"}),
        },
        |_| {
            let QueryRow::Events(events) = store.query(Query::AllEvents).unwrap() else {
                panic!("expected events");
            };
            let max = events.last().unwrap();
            assert_eq!(
                max.kind_raw,
                EventKind::UpstreamSubmitted.as_str(),
                "the last row at wire hand-off must be the intent, nothing after it"
            );
            (max.event_id, ())
        },
    )
    .unwrap();

    assert_eq!(observed_max, intent_event);
}

/// The full-pipeline twin: the same invariant through the real Forwarder
/// over the real SQLite store. The fake transport inspects the store at
/// the instant its `send` is entered — the wire hand-off — and records
/// the log's highest event_id; nothing may be written between the intent
/// commit and that instant, so the last row MUST be the intent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_20_pipeline_ordered_write() {
    use std::collections::HashMap;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    use bytes::Bytes;
    use router_core::config::WireApi;
    use router_core::store::{EventKind, Query, QueryRow, Store as _};
    use router_proxy::{ForwardOutcome, Forwarder, ProviderTransport};

    let dir = tempdir("pipeline");
    let store = Arc::new(router_store::SqliteStore::open(&dir.join("state/router.db")).unwrap());

    /// The fake provider: at wire hand-off, the last event row must be
    /// this attempt's `upstream.submitted` intent.
    struct InspectingTransport {
        store: Arc<router_store::SqliteStore>,
        observed: Arc<Mutex<Vec<(i64, String)>>>,
    }
    impl ProviderTransport for InspectingTransport {
        fn send_boxed<'a>(
            &'a self,
            _req: http::Request<Bytes>,
        ) -> Pin<Box<dyn Future<Output = router_providers::AttemptOutcome> + Send + 'a>> {
            // The instant the bytes would hit the wire: inspect now.
            let QueryRow::Events(events) = self.store.query(Query::AllEvents).expect("events")
            else {
                panic!("events");
            };
            let last = events.last().expect("non-empty log").clone();
            self.observed
                .lock()
                .unwrap()
                .push((last.event_id.0, last.kind_raw.clone()));
            Box::pin(async move {
                router_providers::AttemptOutcome::Responded(
                    router_providers::UpstreamResponse {
                        status: 200,
                        retry_after: None,
                        content_type: Some("application/json".into()),
                        body: Bytes::from_static(
                            br#"{"id":"r","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}"#,
                        ),
                    },
                )
            })
        }
    }

    let observed = Arc::new(Mutex::new(Vec::new()));
    let transport = InspectingTransport {
        store: Arc::clone(&store),
        observed: observed.clone(),
    };
    // A minimal config with one native chat route (built by hand — the
    // config file round trip is CONF-25's object, not this one's).
    let cfg_text = format!(
        r#"server:   {{ addr: "127.0.0.1:39712", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}
providers:
  - name: p1
    base_url: http://127.0.0.1:1/v1
    api_key_env: CONF20_P1_KEY
    wire_api: chat
    supports: [chat]
    models:
      - id: m1
        context: 128k
        price: {{ input_miss: 0.001, input_hit: 0.0001, cache_write: 0.0, output: 0.002, peak: {{ multiplier: 1.0, windows: [] }} }}
        source: "fixture (no price; test double)"
aliases: {{}}
plugins: []
fallback: []
"#
    );
    let cfg_path = dir.join("config.yaml");
    std::fs::write(&cfg_path, cfg_text).unwrap();
    let rc = router_cli::config_load::load(&cfg_path).expect("config");

    let mut transports: HashMap<String, Arc<dyn ProviderTransport>> = HashMap::new();
    transports.insert("p1".into(), Arc::new(transport));
    let forwarder = Forwarder {
        config: rc.router.clone(),
        transports,
        api_keys: HashMap::from([("p1".into(), "k".into())]),
        store: Some(store as Arc<dyn router_core::store::Store>),
        trace: None,
        session_ttl_us: 11 * 3600 * 1_000_000,
    };

    let body = br#"{"model":"p1/m1","messages":[{"role":"user","content":"hi"}],"prompt_cache_key":"sess-conf20"}"#;
    let outcome = forwarder
        .forward(WireApi::Chat, body, "req-conf20", &[])
        .await;
    assert!(
        matches!(outcome, ForwardOutcome::Success(s) if s.status == 200),
        "the pipeline case must succeed"
    );

    // The invariant: at hand-off, the log's last row was the intent.
    let obs = observed.lock().unwrap().clone();
    assert_eq!(obs.len(), 1, "one attempt");
    assert_eq!(
        obs[0].1,
        EventKind::UpstreamSubmitted.as_str(),
        "the last row at wire hand-off is the intent (event_id {})",
        obs[0].0
    );
}
