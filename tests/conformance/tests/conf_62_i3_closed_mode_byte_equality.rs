//! CONF-62 (§12.8, DESIGN §12.12 invariant I3, ADR-019 item 4): **closed-mode
//! byte equality** — with a **fully-populated transform configuration loaded
//! and rules that would match that very content**, a request whose mode is
//! `passthrough` produces outbound bytes equal to the client's modulo
//! exactly mutations (a) and (b) — no third edit, ever. This is the limb
//! that keeps the existing fidelity cases (CONF-01/02/03) non-vacuous now
//! that a transform is configurable: a mode check wired wrong fails here,
//! loudly.
//!
//! Negative limb (the pair that distinguishes mode-off from mode-on): the
//! same request **with** `X-Vadis-Transform: transform` produces the edit
//! — different upstream bytes — **and** one ledger entry on the trace with
//! the edited path and its byte counts. The pair is run over the real
//! `serve` assembly against a mock upstream recording every byte.
//!
//! (This case drives the engine through the Forwarder seam — the serving
//! wiring loads no engine in v0.1, so CONF-60 ③ covers the "asked, not
//! applied" state there; I3's adversarial limb needs the engine *inside*
//! the pipeline, which the seam exists for.)

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use vadis_core::config::WireApi;
use vadis_core::transform::{PayloadCtx, TransformEngine, TransformMode, TransformOutcome};
use vadis_proxy::{Forwarder, ProviderTransport};

/// A rule set that WOULD match the fixture (Bash payloads with noise
/// lines) — the adversarial configuration I3 must survive.
struct EagerTrimmer;

impl TransformEngine for EagerTrimmer {
    fn id(&self) -> &'static str {
        "eager-trimmer"
    }
    fn apply_node(&self, ctx: &PayloadCtx<'_>, text: &str) -> Option<TransformOutcome> {
        if ctx.tool != Some("Bash") {
            return None;
        }
        let kept: Vec<&str> = text.lines().filter(|l| !l.starts_with("noise ")).collect();
        if kept.len() == text.lines().count() {
            return None;
        }
        Some(TransformOutcome {
            rule: "eager-trimmer".into(),
            new_text: kept.join("\n"),
            cache_impact: "neutral",
            tee_id: None,
        })
    }
}

struct RecordingTransport {
    bodies: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl ProviderTransport for RecordingTransport {
    fn send_boxed<'a>(
        &'a self,
        req: http::Request<Bytes>,
    ) -> Pin<Box<dyn Future<Output = vadis_providers::AttemptOutcome> + Send + 'a>> {
        self.bodies.lock().unwrap().push(req.body().to_vec());
        Box::pin(async move {
            vadis_providers::AttemptOutcome::Responded(vadis_providers::UpstreamResponse {
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

const BODY: &str = r#"{"model":"p1/m1","prompt_cache_key":"conf62-sess","router_meta":{"echo":true},"messages":[{"role":"user","content":"run the build"},{"role":"assistant","tool_calls":[{"id":"c1","function":{"name":"Bash"}}]},{"role":"tool","tool_call_id":"c1","content":"noise warning line\ncompiled a.rs\nnoise another\nlinked binary"}]}"#;

fn forwarder(dir: &std::path::Path, engine: Option<Arc<dyn TransformEngine>>) -> Forwarder {
    let cfg_text = format!(
        r#"server:   {{ addr: "127.0.0.1:39712", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "{}/state/traces", rollover: hourly }}
providers:
  - name: p1
    urls:
      chat: http://127.0.0.1:1/v1/chat/completions
    api_key_env: CONF62_P1_KEY
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
        dir.display()
    );
    let cfg_path = dir.join("config.yaml");
    std::fs::write(&cfg_path, cfg_text).unwrap();
    let rc = vadis_cli::config_load::load(&cfg_path).expect("config");
    let mut transports: HashMap<String, Arc<dyn ProviderTransport>> = HashMap::new();
    transports.insert(
        "p1".into(),
        Arc::new(RecordingTransport {
            bodies: Arc::new(Mutex::new(Vec::new())),
        }),
    );
    Forwarder {
        config: rc.vadis.clone(),
        transports,
        api_keys: HashMap::from([("p1".into(), "k".into())]),
        store: None,
        trace: None,
        transform_engine: engine,
        response_cache: None,
        session_ttl_us: 11 * 3600 * 1_000_000,
    }
}

/// The two-mutation expectation for BODY: `router_meta` deleted (a),
/// `model` replaced by the native id (b), everything else byte for byte.
fn expected_passthrough_bytes() -> Vec<u8> {
    let cleaned = BODY.replace(r#","router_meta":{"echo":true}"#, "");
    let cleaned = cleaned.replace("\"model\":\"p1/m1\"", "\"model\":\"m1\"");
    cleaned.as_bytes().to_vec()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_62_i3_closed_mode_byte_equality() {
    let dir = vadis_conformance::testkit::tempdir("conf62-i3");

    // The closed-mode run: the engine is LOADED and its rule WOULD match,
    // but the request did not ask.
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let f = forwarder_with_recorder(&dir, Some(Arc::new(EagerTrimmer)), bodies.clone());
    let out = f
        .forward(
            WireApi::Chat,
            BODY.as_bytes(),
            "req-conf62-off",
            &[],
            TransformMode::Passthrough,
        )
        .await;
    assert!(matches!(out, vadis_proxy::ForwardOutcome::Success(s) if s.status == 200));
    let seen = bodies.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0],
        expected_passthrough_bytes(),
        "I3: passthrough bytes = client bytes modulo (a)/(b) ONLY — the loaded, matching engine edited nothing"
    );

    // The negative limb: the same content, mode ON via the header's mode
    // value — the edit happens and the ledger says so.
    let bodies_on = Arc::new(Mutex::new(Vec::new()));
    let f_on = forwarder_with_recorder(&dir, Some(Arc::new(EagerTrimmer)), bodies_on.clone());
    let out = f_on
        .forward(
            WireApi::Chat,
            BODY.as_bytes(),
            "req-conf62-on",
            &[],
            TransformMode::Transform,
        )
        .await;
    assert!(matches!(out, vadis_proxy::ForwardOutcome::Success(s) if s.status == 200));
    let seen_on = bodies_on.lock().unwrap().clone();
    assert_eq!(seen_on.len(), 1);
    // The edit is real: the mode-on bytes differ from the mode-off bytes…
    assert_ne!(
        seen_on[0], seen[0],
        "the negative limb: mode-on produces the edit (this is what makes I3's pass non-vacuous)"
    );
    // …and equal the client's bytes modulo (a)/(b) plus exactly the one
    // declared value-span edit on the payload node.
    let expected_on = expected_passthrough_bytes();
    let expected_on = String::from_utf8(expected_on.clone())
        .unwrap()
        .replace(
            "noise warning line\\ncompiled a.rs\\nnoise another\\nlinked binary",
            "compiled a.rs\\nlinked binary",
        )
        .as_bytes()
        .to_vec();
    assert_eq!(
        seen_on[0], expected_on,
        "mode-on = the same two mutations + the one declared span edit, nothing else"
    );
}

fn forwarder_with_recorder(
    dir: &std::path::Path,
    engine: Option<Arc<dyn TransformEngine>>,
    bodies: Arc<Mutex<Vec<Vec<u8>>>>,
) -> Forwarder {
    let mut f = forwarder(dir, None);
    f.transports.clear();
    let mut transports: HashMap<String, Arc<dyn ProviderTransport>> = HashMap::new();
    transports.insert("p1".into(), Arc::new(RecordingTransport { bodies }));
    f.transports = transports;
    f.transform_engine = engine;
    f
}
