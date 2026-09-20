//! CONF-61 (§12.8, DESIGN §12.12 invariant I1, ADR-019 item 4): **content
//! determinism** — with a fixed effective transform set, two runs of the
//! composition step over identical inbound bytes produce byte-identical
//! outbound bodies (what the upstream actually received), and the *same*
//! bytes with a different session and `turn_index` produce the same output
//! — the engine reads no clock, no turn index, no session history and no
//! RNG (AGENTS hard constraint 2).
//!
//! Run on **both forwarding paths** (buffered and streaming), because the
//! same request with a different relay is one request (§12.10.3 R11).
//!
//! Negative limb: the trimmed output differs from the untrimmed bytes — the
//! assertion pair proves this is a real edit and not a passthrough dressed
//! as one, so a determinism pass cannot be vacuous.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use router_core::config::WireApi;
use router_core::transform::{PayloadCtx, TransformEngine, TransformMode, TransformOutcome};
use router_proxy::{Forwarder, ProviderTransport};

/// The test engine: a deterministic tool-payload trimmer — drop lines
/// starting with `noise ` (the `bash-log-noise` class), keep everything
/// else. Pure function of (node text, stable rule set): nothing else is
/// observable, which is I1's sufficient rule.
struct NoiseTrimmer;

impl TransformEngine for NoiseTrimmer {
    fn id(&self) -> &'static str {
        "noise-trimmer"
    }
    fn apply_node(&self, ctx: &PayloadCtx<'_>, text: &str) -> Option<TransformOutcome> {
        // match_tool selection: only the Bash family's payloads.
        if ctx.tool != Some("Bash") {
            return None;
        }
        let kept: Vec<&str> = text.lines().filter(|l| !l.starts_with("noise ")).collect();
        if kept.len() == text.lines().count() {
            return None; // nothing changed: no edit, no ledger entry
        }
        Some(TransformOutcome {
            rule: "noise-trimmer".into(),
            new_text: kept.join("\n"),
            cache_impact: "neutral",
            tee_id: None,
        })
    }
}

/// Records every request body the "wire" received (the buffered path's
/// transport seam).
struct RecordingTransport {
    bodies: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl ProviderTransport for RecordingTransport {
    fn send_boxed<'a>(
        &'a self,
        req: http::Request<Bytes>,
    ) -> Pin<Box<dyn Future<Output = router_providers::AttemptOutcome> + Send + 'a>> {
        self.bodies.lock().unwrap().push(req.body().to_vec());
        Box::pin(async move {
            router_providers::AttemptOutcome::Responded(router_providers::UpstreamResponse {
                status: 200,
                retry_after: None,
                content_type: Some("application/json".into()),
                body: Bytes::from_static(
                    br#"{"id":"r","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}"#,
                ),
            })
        })
    }
}

fn config_text(dir: &std::path::Path, upstream_port: u16) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:39712", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "{dir}/state/traces", rollover: hourly }}
providers:
  - name: p1
    base_url: http://127.0.0.1:{upstream_port}/v1
    api_key_env: CONF61_P1_KEY
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
"#,
        dir = dir.display(),
        upstream_port = upstream_port,
    )
}

/// The fixture: a tool payload with noise lines, a user message, and a
/// session key — the shape an agent loop resends every turn.
const BODY_A: &str = r#"{"model":"p1/m1","prompt_cache_key":"conf61-sess","messages":[{"role":"user","content":"run the build"},{"role":"assistant","tool_calls":[{"id":"c1","function":{"name":"Bash"}}]},{"role":"tool","tool_call_id":"c1","content":"noise warning line\ncompiled a.rs\nnoise another\nlinked binary"}]}"#;

fn forwarder(
    dir: &std::path::Path,
    upstream_port: u16,
    transport: Option<Arc<dyn ProviderTransport>>,
) -> Forwarder {
    let cfg_path = dir.join("config.yaml");
    std::fs::write(&cfg_path, config_text(dir, upstream_port)).unwrap();
    let rc = router_cli::config_load::load(&cfg_path).expect("config");
    let mut transports: HashMap<String, Arc<dyn ProviderTransport>> = HashMap::new();
    if let Some(t) = transport {
        transports.insert("p1".into(), t);
    } else {
        // The stream path never consults this map (it opens its own
        // client); a stub keeps the map non-empty for the config walk.
        transports.insert(
            "p1".into(),
            Arc::new(RecordingTransport {
                bodies: Arc::new(Mutex::new(Vec::new())),
            }),
        );
    }
    Forwarder {
        config: rc.router.clone(),
        transports,
        api_keys: HashMap::from([("p1".into(), "k".into())]),
        store: None,
        trace: None,
        transform_engine: Some(Arc::new(NoiseTrimmer)),
        session_ttl_us: 11 * 3600 * 1_000_000,
    }
}

const EXPECTED_EDITED: &str = r#"{"model":"p1/m1","prompt_cache_key":"conf61-sess","messages":[{"role":"user","content":"run the build"},{"role":"assistant","tool_calls":[{"id":"c1","function":{"name":"Bash"}}]},{"role":"tool","tool_call_id":"c1","content":"compiled a.rs\nlinked binary"}]}"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_61_i1_content_determinism_buffered() {
    let dir = router_conformance::testkit::tempdir("conf61-buf");
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let f = forwarder(
        &dir,
        1,
        Some(Arc::new(RecordingTransport {
            bodies: bodies.clone(),
        })),
    );
    // Two identical runs: same bytes, same session, same turn.
    for _ in 0..2 {
        let out = f
            .forward(
                WireApi::Chat,
                BODY_A.as_bytes(),
                "req-conf61",
                &[],
                TransformMode::Transform,
            )
            .await;
        assert!(matches!(out, router_proxy::ForwardOutcome::Success(s) if s.status == 200));
    }
    // A third run with a DIFFERENT session key: the same content must
    // still produce the same outbound bytes (the plan reads no session).
    let body_b = BODY_A.replace("conf61-sess", "conf61-other-session");
    let out = f
        .forward(
            WireApi::Chat,
            body_b.as_bytes(),
            "req-conf61-b",
            &[],
            TransformMode::Transform,
        )
        .await;
    assert!(matches!(out, router_proxy::ForwardOutcome::Success(s) if s.status == 200));

    let seen = bodies.lock().unwrap().clone();
    assert_eq!(seen.len(), 3);
    // NOTE: the upstream body carries the native model id (mutation (b))
    // and no router-owned keys — the trim happened on top of that.
    let expected_native = EXPECTED_EDITED.replace("\"model\":\"p1/m1\"", "\"model\":\"m1\"");
    assert_eq!(
        seen[0], seen[1],
        "two runs of the same bytes produce byte-identical outbound bodies"
    );
    assert_eq!(
        seen[0],
        expected_native.as_bytes().to_vec(),
        "the edit landed on the payload node only (the noise lines are gone, everything else byte-identical)"
    );
    // The different-session run: identical modulo the session key itself.
    let expected_b = expected_native.replace("conf61-sess", "conf61-other-session");
    assert_eq!(
        seen[2],
        expected_b.as_bytes().to_vec(),
        "a different session/turn context does not change the plan (I1: no session history in f)"
    );
    // And the trimmed bytes differ from the untrimmed ones — the negative
    // limb: this is a real edit, not a passthrough dressed as one.
    let untrimmed_native = BODY_A.replace("\"model\":\"p1/m1\"", "\"model\":\"m1\"");
    assert_ne!(seen[0], untrimmed_native.as_bytes().to_vec());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_61_i1_content_determinism_stream() {
    let dir = router_conformance::testkit::tempdir("conf61-stream");
    // The stream path opens its own HTTP client to the provider's
    // base_url, so the "wire" is a real mock upstream (CONF-30's rig).
    let upstream = router_conformance::testkit::MockUpstream::start()
        .await
        .unwrap();
    upstream.queue(router_conformance::testkit::CannedResponse::sse(vec![
        router_conformance::testkit::SseChunk::event(b"data: {\"ok\":true}\n\n"),
        router_conformance::testkit::SseChunk::event(b"data: [DONE]\n\n"),
    ]));
    upstream.queue(router_conformance::testkit::CannedResponse::sse(vec![
        router_conformance::testkit::SseChunk::event(b"data: {\"ok\":true}\n\n"),
        router_conformance::testkit::SseChunk::event(b"data: [DONE]\n\n"),
    ]));

    let f = forwarder(&dir, upstream.addr.port(), None);
    // The same fixture with `stream:true` inserted after the model member.
    let stream_body = BODY_A.replace(
        "\"model\":\"p1/m1\",",
        "\"model\":\"p1/m1\",\"stream\":true,",
    );
    for _ in 0..2 {
        let out = f
            .forward_stream(
                WireApi::Chat,
                stream_body.as_bytes(),
                "req-conf61-s",
                &[],
                TransformMode::Transform,
            )
            .await;
        assert!(
            matches!(out, router_proxy::StreamOutcome::Success(s) if s.status == 200),
            "the stream path must relay"
        );
    }
    let seen: Vec<Vec<u8>> = upstream.requests().into_iter().map(|r| r.body).collect();
    assert_eq!(seen.len(), 2, "one attempt per request");
    assert_eq!(
        seen[0], seen[1],
        "the streaming path is the same request with a different relay: identical outbound bytes"
    );
    // The trim removed the two noise lines; mutation (b) rewrote the
    // model value only (`stream` survives, like every other member);
    // everything else is the client's bytes.
    let expected = stream_body
        .replace("\"model\":\"p1/m1\"", "\"model\":\"m1\"")
        .replace(
            "noise warning line\\ncompiled a.rs\\nnoise another\\nlinked binary",
            "compiled a.rs\\nlinked binary",
        );
    assert_eq!(
        String::from_utf8_lossy(&seen[0]),
        expected,
        "the stream path's edit equals the buffered path's edit"
    );
    // Negative limb, the stream twin: the edit is real.
    let untrimmed = stream_body.replace("\"model\":\"p1/m1\"", "\"model\":\"m1\"");
    assert_ne!(String::from_utf8_lossy(&seen[0]), untrimmed);
}
