//! CONF-16 (DESIGN §12.8's row and §12.12, ADR-019): **the cache
//! regression, re-measured with each landed transform enabled separately**
//! — one arm per rule of the repo's own `rules/tool_output.toml` — each
//! arm's same-session two-turn `prefix.continuity` asserted **not below
//! the baseline measured on the same rig in the same run**, the baseline
//! arm being the identical conversation with the transform NOT requested
//! (CONF-15's measurement shape).
//!
//! History, as prose (the frozen assertion's wording is unchanged): this
//! case carried an ignore attribute because the v0.1 transform chain did
//! not exist —
//! "each transform individually" had an empty parameter list and any body
//! would have been vacuously green, while the empty chain's own prefix
//! behavior was already CONF-15's object. That reason no longer describes
//! the tree: the `builtin/transform_rules` engine, the mode channel
//! (CONF-60) and the rule file (4 rules, 13 inline tests) landed, and
//! CONF-63 drives them end to end through the real `serve` assembly — the
//! fixture that makes this case non-vacuous exists. The un-ignore was
//! authorized by the owner (R41-0, 2026-09-25; ADR-019:263 places it in
//! "the same change" as the mode landing, a human-allocated §12.8 id).
//!
//! Parameterization record (DESIGN:880's row says "one case per plugin";
//! the card settles the landed reading): the chain is ONE plugin
//! (`builtin/transform_rules`) whose configurable unit is the rule file;
//! the engine exposes no per-rule and no per-plugin toggle, and its
//! first-hit-wins semantics edit each payload node with at most one rule.
//! An arm therefore drives a payload that exactly one rule selects AND
//! applies, over the repo's own rule file loaded verbatim by the real
//! `serve` assembly (`plugins[].config.rules_file`, CONF-63's mechanism),
//! with the transform active only because the request asked for the mode
//! (CONF-60's channel). The ledger assertion (`transforms[]` holds exactly
//! one step, `plugin == <rule id>`, on every treatment turn) is what
//! proves the arm measured that rule alone, and the parameter list is
//! derived from the rule file's own `[filters.<id>]` headers — non-empty
//! by assertion, and a rule-set change without a fixture decision fails
//! this case loudly.
//!
//! Red control (mandatory, CONF-63's negative limb mirrored): the
//! `sess-conf16-red` session's turn 2 resends the history with a
//! mid-history edit of an earlier payload's SURVIVING line plus an append.
//! The assertion pair — an appended turn keeps continuity / a mid-history
//! edit breaks it — must discriminate. The forced run must FAIL and its
//! failure line is the evidence:
//! `CONF16_FORCE_RED=1 cargo test -p vadis-conformance --test conf_16_transform_cache_regression`
//!
//! Evidence export (opt-in; unset in the gate run): `CONF16_EVIDENCE_DIR`
//! names a directory that receives each arm's raw DecisionRecords (JSONL),
//! the upstream-visible bodies, and `arms.json` carrying every measured
//! number. Nothing outside the test's tempdir is written without it.

#![forbid(unsafe_code)]

use vadis_conformance::testkit::{self, CannedResponse};

/// The repo's landed rule file — the engine's acceptance-tested data,
/// loaded verbatim (CONF-63's mechanism; no test-local trimmer exists).
const RULES_FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../rules/tool_output.toml");

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
    api_key_env: CONF16_MOCK_KEY
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

// ---- The arm fixtures: one payload per rule, each shaped so that rule —
// and no other — selects AND applies on the live path. First-hit-wins
// (ascending `order`, ties by id) makes "at most one rule edits a node"
// structural; the per-arm ledger assertion below proves it per arm. ----

/// One parameterized arm: the rule under test, the wire tool name whose
/// declared kinds admit it, the payload it edits, and a probe substring
/// that survives the edit (its presence on the wire, with the raw payload
/// absent, is the byte-level proof the rule fired).
struct Arm {
    rule: &'static str,
    tool: &'static str,
    payload: &'static str,
    kept_probe: &'static str,
}

/// `tool-result-json` (order -1, `match_kind [log, text]`, min 32 bytes):
/// a Bash payload that is pretty-printed JSON over the byte budget —
/// Bash's declared kinds are `[log, text]`, the rule selects and
/// `json_compact` applies.
const JSON_PAYLOAD: &str =
    "{\n  \"total\": 2,\n  \"hits\": [\n    {\"path\": \"a.rs\", \"line\": 12},\n    {\"path\": \"b.rs\", \"line\": 40}\n  ]\n}\n";

/// `bash-log-noise` (`match_tool` shell family): a Bash build log — not
/// JSON, so `tool-result-json` selects but fails to apply (`json_compact`
/// parse failure is fail-safe) and the fall-through lands here; the
/// progress lines are dropped, the diagnostics kept.
const BASH_LOG: &str =
    "   Compiling serde v1.0.210\nwarning: unused variable: `x`\n[ 91%] Building object\nerror: could not compile\n";

/// `grep-hits-budget` (`match_kind [text]` AND `match_tool` search
/// family): a `grep -C` shape — `tool-result-json` selects on the text
/// kind, fails to parse, falls through; the context lines are dropped and
/// the tee marker rides along on the added side.
const GREP_HITS: &str =
    "src/a.rs-10-  fn unrelated() {\nsrc/a.rs:12:let x = foo();\nsrc/a.rs-13-  trailing context\n";

/// `diff-budget` (`match_kind [diff]`): a Diff payload — the diff kind
/// selects no other rule; the `index` noise line is dropped, the hunk
/// capped at `max_lines`, the original tee'd.
const DIFF_PAYLOAD: &str = "diff --git a/src/lib.rs b/src/lib.rs\nindex 1a2b3c4..5d6e7f8 100644\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,4 @@ fn main() {\n-    let x = 1;\n+    let x = 2;\n+    let y = 3;\n    tail();\n";

const ARMS: &[Arm] = &[
    Arm {
        rule: "tool-result-json",
        tool: "Bash",
        payload: JSON_PAYLOAD,
        kept_probe: "{\"total\":2,\"hits\":",
    },
    Arm {
        rule: "bash-log-noise",
        tool: "Bash",
        payload: BASH_LOG,
        kept_probe: "warning: unused variable: `x`",
    },
    Arm {
        rule: "grep-hits-budget",
        tool: "Grep",
        payload: GREP_HITS,
        kept_probe: "src/a.rs:12:let x = foo();",
    },
    Arm {
        rule: "diff-budget",
        tool: "Diff",
        payload: DIFF_PAYLOAD,
        kept_probe: "+++ b/src/lib.rs",
    },
];

/// The parameter list, derived from the rule file itself: one arm per
/// `[filters.<id>]` header.
fn rule_file_ids() -> Vec<String> {
    let text = std::fs::read_to_string(RULES_FILE).expect("the repo rule file reads");
    text.lines()
        .filter_map(|l| {
            l.strip_prefix("[filters.")
                .and_then(|s| s.strip_suffix(']'))
        })
        .map(str::to_string)
        .collect()
}

/// One turn's body: the stateless-client shape (the full history resent,
/// appended per turn) under one session key — CONF-15's measurement
/// shape, carrying one tool-result node the arm's rule edits.
fn turn_body(session: &str, tool: &str, payload: &str, turn: usize) -> String {
    let first_turn = format!(
        r#"{{"role":"user","content":"run"}},{{"role":"assistant","tool_calls":[{{"id":"c1","function":{{"name":"{tool}"}}}}]}},{{"role":"tool","tool_call_id":"c1","content":{}}}"#,
        serde_json::to_string(payload).unwrap()
    );
    let messages = if turn >= 2 {
        format!(
            r#"{first_turn},{{"role":"assistant","content":"ok"}},{{"role":"user","content":"next"}}"#
        )
    } else {
        first_turn
    };
    format!(r#"{{"model":"mock/glm","prompt_cache_key":"{session}","messages":[{messages}]}}"#)
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

/// `probe` as it appears inside the outbound body's JSON string encoding
/// (the inner slice of its own encoding — boundary-safe for these probes,
/// which start and end on literal characters).
fn encoded(probe: &str) -> String {
    let e = serde_json::to_string(probe).unwrap();
    e[1..e.len() - 1].to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_16_transform_cache_regression() {
    // The parameter list is the landed rule set itself: non-empty, one arm
    // per rule, drift-locked to the file in both directions.
    let mut file_ids = rule_file_ids();
    file_ids.sort();
    assert!(
        !file_ids.is_empty(),
        "the parameter list is the rule file's own rule set"
    );
    let mut arm_ids: Vec<&str> = ARMS.iter().map(|a| a.rule).collect();
    arm_ids.sort_unstable();
    assert_eq!(
        arm_ids, file_ids,
        "one arm per rule of rules/tool_output.toml — a rule-set change is a fixture decision"
    );

    let dir = testkit::tempdir("conf16");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    // Per arm: 2 baseline turns + 2 treatment turns; plus the red pair.
    let n_requests = ARMS.len() * 4 + 2;
    for _ in 0..n_requests {
        upstream.queue(canned());
    }

    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, config_yaml(upstream.addr.port(), listen_port)).unwrap();
    std::env::set_var("CONF16_MOCK_KEY", "sk-conf16");
    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);

    let post = |body: String, transform: bool| {
        let headers: Vec<(&str, &str)> = if transform {
            vec![("x-router-transform", "transform")]
        } else {
            vec![]
        };
        testkit::http_post(
            &listen_addr,
            "/v1/chat/completions",
            body.as_bytes(),
            &headers,
        )
    };

    // Fixed post order — the upstream request log is indexed by it below:
    // per arm, baseline turns 1-2 then treatment turns 1-2; then the red
    // pair. The baseline arm is the identical conversation with the
    // transform NOT requested (no mode header): the same rig, the same run.
    for arm in ARMS {
        for turn in 1..=2 {
            let (status, body, _) = post(
                turn_body(
                    &format!("sess-conf16-base-{}", arm.rule),
                    arm.tool,
                    arm.payload,
                    turn,
                ),
                false,
            );
            assert_eq!(
                status,
                200,
                "baseline {} turn {turn}: {}",
                arm.rule,
                String::from_utf8_lossy(&body)
            );
        }
        for turn in 1..=2 {
            let (status, body, _) = post(
                turn_body(
                    &format!("sess-conf16-rule-{}", arm.rule),
                    arm.tool,
                    arm.payload,
                    turn,
                ),
                true,
            );
            assert_eq!(
                status,
                200,
                "treatment {} turn {turn}: {}",
                arm.rule,
                String::from_utf8_lossy(&body)
            );
        }
    }

    // The red control: turn 2 resends the history with a mid-history edit
    // of the earlier payload's SURVIVING line, then appends. (An edit
    // inside a stripped line would be normalized away by the rule itself —
    // the transform is a pure function, so that shape stays monotone by
    // construction; the mutation must land in a line the rule keeps.)
    let red_turn1 = turn_body("sess-conf16-red", "Bash", BASH_LOG, 1);
    let red_turn2_clean = turn_body("sess-conf16-red", "Bash", BASH_LOG, 2);
    let red_turn2 = red_turn2_clean.replace("unused variable", "unused variab1e");
    assert_ne!(
        red_turn2, red_turn2_clean,
        "the mutation lands in a line the rule keeps"
    );
    let (status, body, _) = post(red_turn1, true);
    assert_eq!(
        status,
        200,
        "red turn 1: {}",
        String::from_utf8_lossy(&body)
    );
    let (status, body, _) = post(red_turn2, true);
    assert_eq!(
        status,
        200,
        "red turn 2: {}",
        String::from_utf8_lossy(&body)
    );

    serve_task.abort();
    let _ = serve_task.await;

    let records = read_trace(&dir);
    assert_eq!(records.len(), n_requests, "one record per request");
    let seen = upstream.requests();
    assert_eq!(seen.len(), n_requests, "one upstream attempt per request");

    let session_records = |session: &str| -> Vec<serde_json::Value> {
        let mut v: Vec<serde_json::Value> = records
            .iter()
            .filter(|r| r["identity"]["session"] == session)
            .cloned()
            .collect();
        v.sort_by_key(|r| r["identity"]["turn_index"].as_u64().unwrap());
        v
    };
    let continuity = |rec: &serde_json::Value| rec["prefix"]["continuity"].as_f64();

    let mut arms_summary: Vec<serde_json::Value> = Vec::new();
    for (i, arm) in ARMS.iter().enumerate() {
        let base = session_records(&format!("sess-conf16-base-{}", arm.rule));
        let treat = session_records(&format!("sess-conf16-rule-{}", arm.rule));
        assert_eq!(base.len(), 2, "baseline session has its two turns");
        assert_eq!(treat.len(), 2, "treatment session has its two turns");

        // Turn 1 of a session has no previous request: absent, not invented.
        assert!(continuity(&base[0]).is_none());
        assert!(continuity(&treat[0]).is_none());

        // The baseline arm — the transform not requested — reproduces
        // CONF-15's passthrough continuity on this rig in this run.
        let baseline_c = continuity(&base[1]).expect("baseline turn 2 measured");
        assert_eq!(
            baseline_c, 1.0,
            "baseline arm ({}): same-session passthrough continuity",
            arm.rule
        );
        assert_eq!(base[0]["transform_mode"], "passthrough");
        assert_eq!(base[1]["transform_mode"], "passthrough");
        assert!(
            base[1].get("transforms").is_none(),
            "the baseline ran no transform"
        );

        // The object under test (DESIGN:880): with this rule enabled, the
        // same-session cache regression is not below the baseline.
        let treatment_c = continuity(&treat[1]).expect("treatment turn 2 measured");
        assert!(
            treatment_c >= baseline_c,
            "CONF-16 arm {}: continuity {treatment_c} is below the same-run baseline {baseline_c}",
            arm.rule
        );

        // The mechanism behind the number (CONF-15's block-level form):
        // turn 1's outbound blocks are a prefix of turn 2's, re-derived
        // identically under the transform.
        let blocks1 = treat[0]["prefix"]["blocks"].as_array().expect("blocks");
        let blocks2 = treat[1]["prefix"]["blocks"].as_array().expect("blocks");
        assert!(
            blocks2.len() > blocks1.len(),
            "arm {}: the stateless-client shape ({} vs {} blocks)",
            arm.rule,
            blocks2.len(),
            blocks1.len()
        );
        for (j, b) in blocks1.iter().enumerate() {
            assert_eq!(
                b["hash"], blocks2[j]["hash"],
                "arm {}: block {j} hash moved between turns",
                arm.rule
            );
        }

        // Non-vacuity, per arm: the ledger names exactly this rule on every
        // treatment turn (first-hit-wins made singular), labelled inferred —
        // no measured saving is claimed anywhere.
        for rec in &treat {
            assert_eq!(rec["transform_mode"], "transform");
            let steps = rec["transforms"].as_array().expect("the arm's rule ran");
            assert_eq!(
                steps.len(),
                1,
                "arm {}: exactly one rule edited the node",
                arm.rule
            );
            assert_eq!(steps[0]["plugin"], arm.rule);
            assert_eq!(
                steps[0]["verdict"], "inferred",
                "decision-time figures are inferred, never verified"
            );
        }
        // And the wire carries the edited bytes, not the client's: the raw
        // payload never reached the upstream, the kept probe did.
        let wire_t1 = String::from_utf8_lossy(&seen[i * 4 + 2].body).into_owned();
        assert!(
            !wire_t1.contains(&encoded(arm.payload)),
            "arm {}: the raw payload never reached the upstream",
            arm.rule
        );
        assert!(
            wire_t1.contains(&encoded(arm.kept_probe)),
            "arm {}: the kept line is on the wire",
            arm.rule
        );

        arms_summary.push(serde_json::json!({
            "rule": arm.rule,
            "tool": arm.tool,
            "baseline_arm": "transform not requested (no X-Router-Transform header)",
            "baseline_prefix_continuity": baseline_c,
            "treatment_prefix_continuity": treatment_c,
            "delta": treatment_c - baseline_c,
            "comparison": "treatment >= baseline (DESIGN.md:880: not below the baseline)",
            "ledger_plugin": arm.rule,
            "ledger_verdict": "inferred",
            "treatment_blocks_turn1": blocks1.len(),
            "treatment_blocks_turn2": blocks2.len(),
        }));
    }

    // The red control's measurement.
    let red = session_records("sess-conf16-red");
    assert_eq!(red.len(), 2, "the red session has its two turns");
    assert!(continuity(&red[0]).is_none());
    let red_c = continuity(&red[1]).expect("red turn 2 measured");
    if std::env::var_os("CONF16_FORCE_RED").is_some() {
        // FORCED RED RUN — this arm MUST fail: it asserts the green claim
        // (an append keeps the prefix) over a mid-history edit. A pass here
        // would mean the metric cannot move; the verbatim failure line is
        // the evidence that the assertion pair discriminates.
        assert_eq!(
            red_c, 1.0,
            "CONF16_FORCE_RED: the green claim over a mid-history edit must fail (continuity was {red_c})"
        );
    } else {
        assert!(
            red_c < 1.0,
            "red control: a mid-history edit breaks prefix continuity (got {red_c}); \
             force the green claim with CONF16_FORCE_RED=1 (must fail)"
        );
    }

    // Opt-in evidence export: the raw DecisionRecords per arm, the
    // upstream-visible bodies, and the summary. Unset in the gate run —
    // nothing outside the tempdir is written without it.
    if let Some(ev_dir) = std::env::var_os("CONF16_EVIDENCE_DIR") {
        let ev = std::path::PathBuf::from(ev_dir);
        std::fs::create_dir_all(ev.join("upstream")).unwrap();
        let write_jsonl = |name: &str, recs: &[serde_json::Value]| {
            let mut s = String::new();
            for r in recs {
                s.push_str(&serde_json::to_string(r).unwrap());
                s.push('\n');
            }
            std::fs::write(ev.join(name), s).unwrap();
        };
        for (i, arm) in ARMS.iter().enumerate() {
            write_jsonl(
                &format!("baseline-{}.jsonl", arm.rule),
                &session_records(&format!("sess-conf16-base-{}", arm.rule)),
            );
            write_jsonl(
                &format!("rule-{}.jsonl", arm.rule),
                &session_records(&format!("sess-conf16-rule-{}", arm.rule)),
            );
            let labels = [
                "baseline-turn1",
                "baseline-turn2",
                "treatment-turn1",
                "treatment-turn2",
            ];
            for (k, label) in labels.iter().enumerate() {
                std::fs::write(
                    ev.join("upstream")
                        .join(format!("{}.{}.body", arm.rule, label)),
                    &seen[i * 4 + k].body,
                )
                .unwrap();
            }
        }
        write_jsonl("red.jsonl", &red);
        std::fs::write(
            ev.join("upstream").join("red.turn1.body"),
            &seen[ARMS.len() * 4].body,
        )
        .unwrap();
        std::fs::write(
            ev.join("upstream").join("red.turn2.body"),
            &seen[ARMS.len() * 4 + 1].body,
        )
        .unwrap();
        let summary = serde_json::json!({
            "case": "CONF-16",
            "object": "cache regression (same-session two-turn prefix.continuity), re-measured with each rule of rules/tool_output.toml enabled separately, not below the same-run baseline",
            "rules_file": RULES_FILE,
            "parameter_list": file_ids,
            "arms": arms_summary,
            "red_control": {
                "session": "sess-conf16-red",
                "shape": "turn 2 = mid-history edit of a surviving line + append",
                "prefix_continuity": red_c,
                "discriminates": red_c < 1.0,
                "force_green_claim": "CONF16_FORCE_RED=1 (must fail)",
            },
        });
        std::fs::write(
            ev.join("arms.json"),
            serde_json::to_string_pretty(&summary).unwrap(),
        )
        .unwrap();
    }
}
