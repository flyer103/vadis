//! CONF-120 (§12.8, spec §2.1/§9, ADR-019): **`vadis wrap` supplies the
//! client's own request** — the launcher exists so the finished transform
//! lever has a trigger (measured: 0 of 8,461 production records fire it),
//! and this case proves the wiring end to end **on the record the serve
//! writes**, never on the plumbing:
//!
//! - **arm ① `wrap codex`** — the child (the in-repo double, standing in
//!   for codex via a PATH shim) reads the `-c … env_http_headers={…}`
//!   override wrap injects plus the `VADIS_TRANSFORM` env var, and sends the
//!   header **the way codex itself does**; the serve's record carries
//!   `transform_mode: "transform"` and a **non-empty `transforms[]`** (one
//!   step, `plugin == "tool-result-json"`, `verdict == "inferred"`), and the
//!   upstream received the **edited** bytes, never the raw payload;
//! - **arm ② `wrap claude`** — same shape through Claude Code's own
//!   `ANTHROPIC_CUSTOM_HEADERS` surface; identical record and wire
//!   assertions;
//! - **arm ③ the control (the promise survived)** — the identical double,
//!   launched **without** wrap, sends no header: `transform_mode:
//!   "passthrough"`, no `transforms[]` member, and the upstream bytes are
//!   the client's own modulo mutations (a)/(b) — a client that did not ask
//!   is still byte-identical, which is the half that proves wrap did not
//!   quietly turn editing on anywhere;
//! - **refusal arms** — an unknown client name and a codex config naming no
//!   top-level `model_provider` each exit `2`.
//!
//! The double (`src/bin/wrap_child_double.rs`) makes the request itself; a
//! case that only asserted "the env var was set" would assert plumbing, not
//! that editing happened. The real-client evidence (codex-cli 0.163.0,
//! Claude Code 2.1.300) is the round's manual run, not this case.

#![forbid(unsafe_code)]

use std::path::PathBuf;

use vadis_conformance::testkit::{self, CannedResponse};

const RULES_FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../rules/tool_output.toml");

/// The double binary (same package, `CARGO_BIN_EXE_`).
const DOUBLE: &str = env!("CARGO_BIN_EXE_wrap_child_double");

fn config_yaml(upstream_port: u16, listen_port: u16) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: mock
    urls:
      chat: http://127.0.0.1:{upstream_port}/v1/chat/completions
    api_key_env: CONF118_MOCK_KEY
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
"#,
    )
}

/// CONF-16's `tool-result-json` fixture: a pretty-printed JSON tool payload
/// the rule compacts — one rule, one edit, a non-empty ledger.
const JSON_PAYLOAD: &str =
    "{\n  \"total\": 2,\n  \"hits\": [\n    {\"path\": \"a.rs\", \"line\": 12},\n    {\"path\": \"b.rs\", \"line\": 40}\n  ]\n}\n";
const KEPT_PROBE: &str = "{\"total\":2,\"hits\":";

fn client_body(session: &str) -> String {
    let payload = serde_json::to_string(JSON_PAYLOAD).unwrap();
    format!(
        r#"{{"model":"mock/glm","prompt_cache_key":"{session}","messages":[{{"role":"user","content":"run"}},{{"role":"assistant","tool_calls":[{{"id":"c1","function":{{"name":"Bash"}}}}]}},{{"role":"tool","tool_call_id":"c1","content":{payload}}}]}}"#
    )
}

/// The passthrough expectation: the client's bytes with the top-level
/// `model` value replaced by the native id (mutation (b) alone here).
fn expected_passthrough(session: &str) -> String {
    client_body(session).replace("\"model\":\"mock/glm\"", "\"model\":\"glm\"")
}

fn canned() -> CannedResponse {
    let body = r#"{"id":"r","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":5,"total_tokens":105}}"#;
    CannedResponse::json(200, "OK", body.as_bytes())
}

fn read_trace(dir: &std::path::Path) -> Vec<serde_json::Value> {
    let mut records = Vec::new();
    for entry in std::fs::read_dir(dir.join("state/traces")).expect("trace dir exists") {
        let entry = entry.unwrap();
        if entry.path().extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        for line in std::fs::read_to_string(entry.path()).unwrap().lines() {
            records.push(serde_json::from_str(line).expect("record json"));
        }
    }
    records
}

/// A `Name: Value`-shaped probe as it appears inside a JSON string body.
fn encoded(probe: &str) -> String {
    let e = serde_json::to_string(probe).unwrap();
    e[1..e.len() - 1].to_string()
}

/// Waits until the trace holds a record for `session` (deadline generous:
/// the reload cases' lesson — a poll, never a fixed sleep).
fn wait_for_session_record(dir: &std::path::Path, session: &str) -> serde_json::Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        for r in read_trace(dir) {
            if r["identity"]["session"] == session {
                return r;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no record for session {session} after 60s"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Prepares the PATH shim (`claude`/`codex` → the double) and the isolated
/// `CODEX_HOME` naming `model_provider = "mock"`, and points the ambient
/// environment at them for the wrapped arms.
fn install_shims(dir: &PathBuf) {
    let shim_dir = dir.join("shims");
    std::fs::create_dir_all(&shim_dir).unwrap();
    for name in ["claude", "codex"] {
        let link = shim_dir.join(name);
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(DOUBLE, &link).expect("shim symlink");
    }
    let codex_home = dir.join("codex-home");
    std::fs::create_dir_all(&codex_home).unwrap();
    std::fs::write(
        codex_home.join("config.toml"),
        "model_provider = \"mock\"\n",
    )
    .unwrap();
    std::env::set_var("CODEX_HOME", &codex_home);
    let path = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{}:{path}", shim_dir.display()));
}

/// Runs the real launcher **in its own process**: `wrap` execs the client
/// (replacing the process image), so a success arm may never call
/// `wrap::run` in-process — the double stands in as that process.
fn run_wrap_via_double(dir: &std::path::Path, role: &str) -> std::process::Output {
    std::process::Command::new(DOUBLE)
        .env("WRAP_DOUBLE_ROLE", role)
        .env(
            "WRAP_DOUBLE_ADDR",
            std::env::var("WRAP_DOUBLE_ADDR").unwrap(),
        )
        .env(
            "WRAP_DOUBLE_BODY",
            std::env::var("WRAP_DOUBLE_BODY").unwrap(),
        )
        .current_dir(dir)
        .output()
        .expect("wrap double runs")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_120_wrap_supplies_the_clients_own_request() {
    let dir = testkit::tempdir("conf118");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    for _ in 0..3 {
        upstream.queue(canned());
    }

    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(upstream.addr.port(), listen_port)).unwrap();
    std::env::set_var("CONF118_MOCK_KEY", "sk-conf118");

    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);

    install_shims(&dir);
    let addr_for_double = listen_addr.clone();

    // ---- arm ① `wrap codex`: the -c/env_http_headers wiring ----
    std::env::set_var("WRAP_DOUBLE_ROLE", "wrap-codex");
    std::env::set_var("WRAP_DOUBLE_ADDR", &addr_for_double);
    std::env::set_var("WRAP_DOUBLE_BODY", client_body("conf118-codex"));
    let out = run_wrap_via_double(&dir, "wrap-codex");
    assert!(
        out.status.success(),
        "the wrapped child exits with its own code: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rec_codex = wait_for_session_record(&dir, "conf118-codex");

    // ---- arm ② `wrap claude`: the ANTHROPIC_CUSTOM_HEADERS wiring ----
    std::env::set_var("WRAP_DOUBLE_BODY", client_body("conf118-claude"));
    let out = run_wrap_via_double(&dir, "wrap-claude");
    assert!(
        out.status.success(),
        "the wrapped child exits with its own code: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rec_claude = wait_for_session_record(&dir, "conf118-claude");

    std::env::remove_var("WRAP_DOUBLE_ROLE");
    std::env::remove_var("WRAP_DOUBLE_ADDR");
    std::env::remove_var("WRAP_DOUBLE_BODY");

    // ---- arm ③ the control: same double, no wrap, no header ----
    let ctl = std::process::Command::new(DOUBLE)
        .env_clear()
        .env("WRAP_DOUBLE_ROLE", "claude")
        .env("WRAP_DOUBLE_ADDR", &listen_addr)
        .env("WRAP_DOUBLE_BODY", client_body("conf118-ctl"))
        .output()
        .expect("control double runs");
    assert!(ctl.status.success(), "control double exited cleanly");
    let rec_ctl = wait_for_session_record(&dir, "conf118-ctl");

    // ---- the refusal arms (exit 2, the CLI convention). Both return
    // before any exec, so calling the launcher in-process is safe here. ----
    std::env::set_var("CODEX_HOME", dir.join("codex-home-empty"));
    std::fs::create_dir_all(dir.join("codex-home-empty")).unwrap();
    assert_eq!(
        vadis_cli::wrap::run("cursor", &[]),
        2,
        "unknown client → exit 2"
    );
    assert_eq!(
        vadis_cli::wrap::run("codex", &[]),
        2,
        "a codex config with no model_provider → exit 2"
    );
    std::env::remove_var("CODEX_HOME");

    serve_task.abort();
    let _ = serve_task.await;

    // ---- the records: the wrapped arms asked, and editing happened ----
    for (label, rec) in [("codex", &rec_codex), ("claude", &rec_claude)] {
        assert_eq!(
            rec["result"]["status"], 200,
            "arm {label}: the request was served"
        );
        assert_eq!(
            rec["transform_mode"], "transform",
            "arm {label}: the child's own mechanism supplied the header"
        );
        let steps = rec["transforms"]
            .as_array()
            .unwrap_or_else(|| panic!("arm {label}: transforms[] is non-empty"));
        assert_eq!(
            steps.len(),
            1,
            "arm {label}: exactly one rule edited the node"
        );
        assert_eq!(steps[0]["plugin"], "tool-result-json");
        assert_eq!(
            steps[0]["verdict"], "inferred",
            "decision-time figures are inferred, never verified"
        );
    }

    // ---- the control record: the promise survived ----
    assert_eq!(rec_ctl["result"]["status"], 200);
    assert_eq!(rec_ctl["transform_mode"], "passthrough");
    assert!(
        rec_ctl.get("transforms").is_none(),
        "a client that did not ask runs no transform: {:?}",
        rec_ctl.get("transforms")
    );

    // ---- the wire: the wrapped arms carried edited bytes, the control the
    // client's own modulo (a)/(b) ----
    let seen = upstream.requests();
    assert_eq!(seen.len(), 3, "one upstream attempt per request");
    for (i, label) in [(0usize, "codex"), (1, "claude")] {
        let wire = String::from_utf8_lossy(&seen[i].body).into_owned();
        assert!(
            !wire.contains(&encoded(JSON_PAYLOAD)),
            "arm {label}: the raw payload never reached the upstream"
        );
        assert!(
            wire.contains(&encoded(KEPT_PROBE)),
            "arm {label}: the compacted payload is on the wire"
        );
    }
    assert_eq!(
        String::from_utf8_lossy(&seen[2].body),
        expected_passthrough("conf118-ctl"),
        "control: upstream bytes = client bytes modulo (a)/(b)"
    );
}
