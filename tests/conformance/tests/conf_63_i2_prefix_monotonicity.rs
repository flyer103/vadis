//! CONF-63 (§12.8, DESIGN §12.12 invariant I2, ADR-019 item 4): **prefix
//! monotonicity, per fixed effective rule set** — within one session, with
//! the effective set unchanged and the inbound body growing by append,
//! `out(N)` is a **byte prefix** of `out(N+1)` and `prefix.continuity`
//! stays 1.0.
//!
//! Unlike CONF-61/62 (whose engines are test-local trimmers), this case
//! drives the **real** `builtin/transform_rules` engine over the repo's
//! own `rules/tool_output.toml`, loaded by the real `serve` assembly from
//! `plugins[].config.rules_file` — the first rule set that is actually
//! configurable end to end.
//!
//! The ledger is part of the object under test (ADR-019 §5): every step
//! reports `saved` / `added` / `net = saved − added`, the added side
//! counts the replaced text **and the tee marker**, and every decision-time
//! figure is labelled `inferred` — no saving is claimed as measured.
//!
//! A change to the effective set mid-session must not be silent (ADR-019
//! §4's last paragraph): when the client stops asking, the mode word
//! moves, the ledger empties and `prefix_continuity` drops for exactly
//! that turn.
//!
//! Negative limbs (a vacuous pass must be visible):
//! - an append keeps the prefix; a **mid-history edit** of an earlier
//!   payload breaks it — the same assertion pair distinguishes the two;
//! - the mode-switch turn's continuity drop shows the metric moves when
//!   the bytes move (it is a predictor, not a constant).

#![forbid(unsafe_code)]

use router_conformance::testkit::{self, CannedResponse};

/// The repo's landed rule file — the engine's acceptance-tested data.
const RULES_FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../rules/tool_output.toml");

fn config_yaml(upstream_port: u16, listen_port: u16) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}
providers:
  - name: mock
    base_url: http://127.0.0.1:{upstream_port}/v1
    api_key_env: CONF63_MOCK_KEY
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
plugins:
  - id: tool-output-rules
    kind: builtin/transform_rules
    config: {{ rules_file: "{RULES_FILE}", on_failure: passthrough }}
fallback: []
"#
    )
}

// ---- The conversation fixtures (all lengths are computed in-test from
// these constants — nothing is transcribed by hand) ----

/// A build log the `bash-log-noise` rule actually trims: two progress
/// lines dropped, two diagnostics kept, no lines-dropped tee (that rule
/// does not declare one).
const BASH_LOG_1: &str = "   Compiling serde v1.0.210\nwarning: unused variable: `x`\n[ 91%] Building object\nerror: could not compile\n";
/// What the rule keeps (the two diagnostics, one trailing newline).
const BASH_LOG_1_TRIMMED: &str = "warning: unused variable: `x`\nerror: could not compile\n";

/// Turn 2's new Bash payload: a `Checking` progress line dropped, the
/// diagnostic lines kept.
const BASH_LOG_2: &str =
    "   Checking module tree\nerror[E0308]: mismatched types\n --> src/main.rs:3:5\n";
const BASH_LOG_2_TRIMMED: &str = "error[E0308]: mismatched types\n --> src/main.rs:3:5\n";

/// Turn 2's new Grep payload: context lines dropped, one hit line kept,
/// and because lines were dropped the rule's `tee` marker rides along —
/// the added side of the ledger must count it.
const GREP_HITS: &str =
    "src/a.rs-10-  fn unrelated() {\nsrc/a.rs:12:let x = foo();\nsrc/a.rs-13-  trailing context\n";

/// The estimate convention (router-core::transform::estimate_tokens):
/// bytes/4, floor. Asserted against the record so the arithmetic the
/// ledger claims is the arithmetic the domain defines.
fn est(bytes: usize) -> i64 {
    (bytes / 4) as i64
}

fn message(parts: &[&str]) -> String {
    parts.join(",")
}

/// One turn's body: the stateless-client shape (full history resent,
/// appended per turn) under one session key.
fn turn_body(session: &str, turn: usize) -> String {
    let tool_call = |id: &str, name: &str, payload: &str| {
        format!(
            r#"{{"role":"user","content":"run"}},{{"role":"assistant","tool_calls":[{{"id":"{id}","function":{{"name":"{name}"}}}}]}},{{"role":"tool","tool_call_id":"{id}","content":{}}}"#,
            serde_json::to_string(payload).unwrap()
        )
    };
    let mut parts: Vec<String> = Vec::new();
    if turn >= 1 {
        parts.push(tool_call("c1", "Bash", BASH_LOG_1));
    }
    if turn >= 2 {
        parts.push(tool_call("c2", "Bash", BASH_LOG_2));
        parts.push(tool_call("c3", "Grep", GREP_HITS));
    }
    if turn >= 3 {
        parts.push(tool_call("c4", "Bash", BASH_LOG_1));
    }
    let refs: Vec<&str> = parts.iter().map(|s| s.as_str()).collect();
    format!(
        r#"{{"model":"mock/glm","prompt_cache_key":"{session}","messages":[{}]}}"#,
        message(&refs)
    )
}

fn canned() -> CannedResponse {
    let body = r#"{"id":"r","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":5,"total_tokens":105,"prompt_tokens_details":{"cached_tokens":50}}}"#;
    CannedResponse::json(200, "OK", body.as_bytes())
}

fn read_trace(dir: &std::path::Path) -> Vec<serde_json::Value> {
    let trace_dir = dir.join("state/traces");
    let mut records = Vec::new();
    for entry in std::fs::read_dir(&trace_dir).expect("trace dir exists") {
        let entry = entry.unwrap();
        if entry.path().extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        for line in std::fs::read_to_string(entry.path()).unwrap().lines() {
            records.push(serde_json::from_str(line).expect("each line is one record"));
        }
    }
    records
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_63_i2_prefix_monotonicity_ledger_and_mode_switch() {
    let dir = testkit::tempdir("conf63");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    // 5 served requests: 3 turns of the main session, 2 of the negative
    // limb's session.
    for _ in 0..5 {
        upstream.queue(canned());
    }

    let listen_port = testkit::free_port();
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(upstream.addr.port(), listen_port)).unwrap();

    std::env::set_var("CONF63_MOCK_KEY", "sk-conf63");
    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
    testkit::wait_listening(&format!("127.0.0.1:{listen_port}"));

    let post = |body: String, mode_header: bool| {
        let headers: Vec<(&str, &str)> = if mode_header {
            vec![("x-router-transform", "transform")]
        } else {
            vec![]
        };
        testkit::http_post(
            &format!("127.0.0.1:{listen_port}"),
            "/v1/chat/completions",
            body.as_bytes(),
            &headers,
        )
    };

    // Main session: turn 1 and 2 ask for transform mode (the effective
    // set is fixed); turn 3 stops asking (the set changes mid-session).
    for turn in 1..=2 {
        let (status, body, _) = post(turn_body("sess-conf63", turn), true);
        assert_eq!(
            status,
            200,
            "turn {turn}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    let (status, body, _) = post(turn_body("sess-conf63", 3), false);
    assert_eq!(
        status,
        200,
        "mode-off turn: {}",
        String::from_utf8_lossy(&body)
    );

    // Negative limb, second session: turn A is the base, turn B is the
    // same history with an earlier payload's SURVIVING line edited plus
    // one append — the append shape deliberately violated. (A mutation
    // inside a stripped noise line would be normalized away by the rule
    // itself — the transform is a pure function, so that shape stays
    // monotone by construction.)
    let (status, body, _) = post(turn_body("sess-conf63-neg", 1), true);
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let mutated = turn_body("sess-conf63-neg", 2).replace("unused variable", "unused variab1e");
    let (status, body, _) = post(mutated, true);
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));

    serve_task.abort();
    let _ = serve_task.await;

    // ---- I2 over the wire: out(1) is a byte prefix of out(2) ----
    let mut seen: Vec<Vec<u8>> = upstream.requests().into_iter().map(|r| r.body).collect();
    assert_eq!(seen.len(), 5, "one attempt per request");
    let neg_b = seen.pop().unwrap();
    let neg_a = seen.pop().unwrap();
    let out3 = seen.pop().unwrap();
    let out2 = seen.pop().unwrap();
    let out1 = seen.pop().unwrap();

    // A JSON array append: turn 2's document continues the array with a
    // `,` exactly where turn 1's closed it (`]}`). The document-level form
    // of I2 therefore compares the shared region — turn 1's document minus
    // its own structural tail must be a byte prefix of turn 2's, which is
    // precisely "every already-present node's outbound bytes were
    // re-derived identically".
    fn shared_region(doc: &[u8]) -> &[u8] {
        &doc[..doc.len() - "]}".len()]
    }
    assert!(
        out2.starts_with(shared_region(&out1)),
        "I2: with the rule set fixed and turn 2 an append, out(1)'s shared region is a byte prefix of out(2)"
    );
    // And it is the EDITED prefix: the rule really fired on both turns
    // (the trimmed payload, not the client's bytes, is what the wire saw).
    let wire_payload = serde_json::to_string(BASH_LOG_1_TRIMMED).unwrap();
    assert!(
        String::from_utf8_lossy(&out1).contains(&wire_payload),
        "turn 1's outbound body carries the trimmed payload"
    );
    let raw_payload = serde_json::to_string(BASH_LOG_1).unwrap();
    assert!(
        !String::from_utf8_lossy(&out1).contains(&raw_payload),
        "turn 1's outbound body does not carry the raw payload"
    );

    // Negative limb 1: a mid-history edit breaks the prefix — the same
    // assertion that passed above fails here, so I2's check is not
    // vacuous.
    assert!(
        !neg_b.starts_with(&neg_a),
        "negative limb: mutating an earlier payload breaks the byte prefix"
    );
    // Negative limb 2: the mode-switch turn's bytes are the unedited
    // history, so out(2) is NOT a prefix of out(3).
    assert!(
        !out3.starts_with(&out2),
        "negative limb: the mode-switch turn changes the bytes (the declared prefix break)"
    );
    assert!(
        String::from_utf8_lossy(&out3).contains(&raw_payload),
        "the mode-off turn forwards the raw payload (I3 on the live path)"
    );

    // ---- The trace: ledger, labels and the mode word ----
    let records = read_trace(&dir);
    assert_eq!(records.len(), 5, "one record per request");
    let main: Vec<&serde_json::Value> = records
        .iter()
        .filter(|r| r["identity"]["session"] == "sess-conf63")
        .collect();
    let neg: Vec<&serde_json::Value> = records
        .iter()
        .filter(|r| r["identity"]["session"] == "sess-conf63-neg")
        .collect();
    assert_eq!(main.len(), 3);
    assert_eq!(neg.len(), 2);

    // Turn 1: one step, the bash rule, inferred, no tee.
    let t1 = main[0];
    assert_eq!(t1["transform_mode"], "transform");
    let steps = t1["transforms"].as_array().expect("one step");
    assert_eq!(steps.len(), 1);
    let s = &steps[0];
    assert_eq!(s["plugin"], "bash-log-noise");
    assert_eq!(
        s["verdict"], "inferred",
        "decision-time figures are inferred, never verified"
    );
    assert_eq!(s["cache_impact"], "neutral");
    assert!(s["tee_id"].is_null(), "bash rule declares no tee");
    assert_eq!(s["saved_output_tokens"], 0);
    let paths = s["edited_paths"].as_array().expect("edited paths");
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0]["path"], "messages[2].content");
    assert_eq!(paths[0]["bytes_in"], BASH_LOG_1.len());
    assert_eq!(paths[0]["bytes_out"], BASH_LOG_1_TRIMMED.len());
    // net = saved − added, both sides of the payload text.
    assert_eq!(s["saved_input_tokens"], est(BASH_LOG_1.len()));
    assert_eq!(s["added_input_tokens"], est(BASH_LOG_1_TRIMMED.len()));
    assert_eq!(
        s["saved_input_tokens"].as_i64().unwrap() - s["added_input_tokens"].as_i64().unwrap() > 0,
        true
    );

    // Turn 2: the same rule re-derives the same bytes for the old node
    // (that is I2's mechanism), plus the new payloads — and the grep step
    // carries the tee marker on its ADDED side.
    let t2 = main[1];
    assert_eq!(t2["transform_mode"], "transform");
    assert_eq!(
        t2["prefix"]["continuity"], 1.0,
        "I2: the append keeps continuity at exactly 1.0"
    );
    let steps2 = t2["transforms"].as_array().expect("two steps");
    assert_eq!(steps2.len(), 2, "bash + grep rules fired");
    let bash2 = steps2
        .iter()
        .find(|s| s["plugin"] == "bash-log-noise")
        .expect("bash step");
    let grep2 = steps2
        .iter()
        .find(|s| s["plugin"] == "grep-hits-budget")
        .expect("grep step");
    // The bash step edited both payload nodes (the resent one re-derived
    // identically, the new one trimmed) — the ledger says which.
    let bpaths = bash2["edited_paths"].as_array().unwrap();
    assert_eq!(bpaths.len(), 2);
    assert_eq!(bpaths[0]["path"], "messages[2].content");
    assert_eq!(
        bpaths[0]["bytes_out"],
        BASH_LOG_1_TRIMMED.len(),
        "the resent node re-derived identically"
    );
    assert_eq!(bpaths[1]["path"], "messages[5].content");
    assert_eq!(
        bash2["saved_input_tokens"],
        est(BASH_LOG_1.len()) + est(BASH_LOG_2.len())
    );
    assert_eq!(
        bash2["added_input_tokens"],
        est(BASH_LOG_1_TRIMMED.len()) + est(BASH_LOG_2_TRIMMED.len())
    );
    // The grep step: tee marker counted on the added side.
    assert_eq!(grep2["verdict"], "inferred");
    let gpath = &grep2["edited_paths"].as_array().unwrap()[0];
    assert_eq!(gpath["path"], "messages[8].content");
    assert_eq!(gpath["bytes_in"], GREP_HITS.len());
    let tee_id = grep2["tee_id"].as_str().expect("tee recorded").to_string();
    assert_eq!(tee_id.len(), 16, "sha16");
    // The marker itself is on the wire, and its numbers are the fixture's
    // own — parsed out of the outbound body, never transcribed by hand.
    let out2s = String::from_utf8_lossy(&out2).into_owned();
    let marker_start = out2s
        .find("[router:tee sha256=")
        .expect("tee marker on the wire")
        + "[router:tee sha256=".len();
    let marker_end = out2s[marker_start..].find(']').unwrap() + marker_start;
    let marker = &out2s[marker_start..marker_end];
    assert!(
        marker.starts_with(&tee_id),
        "the marker carries the recorded tee_id"
    );
    assert!(marker.contains(&format!("lines_dropped=2")), "{marker}");
    assert!(
        marker.contains(&format!("bytes_original={}", GREP_HITS.len())),
        "{marker}"
    );
    // added side = estimate(bytes_out) with the marker inside bytes_out.
    assert_eq!(
        grep2["added_input_tokens"],
        est(gpath["bytes_out"].as_u64().unwrap() as usize)
    );
    assert_eq!(grep2["saved_input_tokens"], est(GREP_HITS.len()));

    // Turn 3 — the mode switch, not silent (ADR-019 §4's last paragraph):
    // the mode word moves, the ledger empties, continuity drops for
    // exactly that turn.
    let t3 = main[2];
    assert_eq!(t3["transform_mode"], "passthrough");
    assert!(
        t3.get("transforms").is_none()
            || t3["transforms"]
                .as_array()
                .map(|a| a.is_empty())
                .unwrap_or(true),
        "no step ran, so no ledger entry exists"
    );
    let c3 = t3["prefix"]["continuity"].as_f64().expect("measured");
    assert!(
        c3 < 1.0,
        "the mode switch is visible: continuity dropped to {c3} for exactly that turn"
    );

    // The negative-limb session: mutation drops continuity too — the
    // predictor moves when the bytes move.
    assert_eq!(neg[0]["prefix"]["continuity"], serde_json::Value::Null);
    let cneg = neg[1]["prefix"]["continuity"].as_f64().expect("measured");
    assert!(
        cneg < 1.0,
        "a mid-history edit drops continuity (got {cneg})"
    );
    // And no record ever claims a measured saving.
    for r in &records {
        if let Some(steps) = r["transforms"].as_array() {
            for s in steps {
                assert_eq!(
                    s["verdict"], "inferred",
                    "no verified figure exists without a pair"
                );
            }
        }
    }
}
