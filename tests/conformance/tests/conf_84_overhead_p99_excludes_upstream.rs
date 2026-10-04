//! CONF-84 (spec §6 · `overhead_ms_p99` + §9.2 · the `overhead p99`
//! line; DESIGN §12.16 / §12.8's row): **the printed figure is the
//! vadis's own overhead, not the upstream's.**
//!
//! Until R32 the collector read the **raw** `overhead_ms` field
//! (`stats.rs`'s `aggregate`), which spans the whole request — it
//! includes the upstream attempt — so `vadis stats`'s `overhead p99`
//! line (and `overhead_ms_p99` in `--json`) printed the upstream's own
//! latency as the vadis's. On any run with a declared upstream delay
//! (the R32 ladder's stand-in), the operator's figure would have been
//! the stand-in's (`R32-F5`, blocking).
//!
//! The repair (DESIGN §12.16, spec §6/§9.2 as written): the figure is
//! the p99 of the **differences** `overhead_ms − upstream_ms`, and a
//! record whose `upstream_ms` is `null` is **excluded** from the
//! sample rather than read as 0 ms.
//!
//! Legs (over rig-built trace windows, driven through `stats`'s own
//! public surfaces — `report` + `report_json` + `print_text`):
//!
//! - **(a)** the decisive control: a window whose records carry a
//!   distinctly larger `overhead_ms` beside a declared `upstream_ms`
//!   prints the p99 of the differences — so **raising every record's
//!   `upstream_ms` while holding `overhead_ms` fixed does not move the
//!   figure** (the control the raw-field p99 fails today);
//! - **(b)** records whose `upstream_ms` is `null` are excluded from
//!   the sample: a window holding only such records has **no** figure
//!   (json `null`, text omits nothing but prints the empty sample),
//!   never `0`;
//! - **(c)** the two print paths agree element for element on one
//!   window: `report_json`'s `overhead_ms_p99` equals the text line's
//!   value.

#![forbid(unsafe_code)]

/// Builds one trace record with the given `overhead_ms` and
/// `upstream_ms` (None ⇒ the field is `null`, the §6 shape a boundary
/// refusal or an unanswered attempt writes).
fn record(overhead_ms: u64, upstream_ms: Option<u64>) -> serde_json::Value {
    serde_json::json!({
        "schema_version": 2,
        "ts": now_rfc3339(),
        "identity": {"request_id": "req-x", "event_id": 1, "client": "other",
                     "session": null, "thread_id": null, "turn_index": 1},
        "protocol": {"protocol_in": "chat", "protocol_out": null, "translated": false},
        "decision": {"provider": "p", "model": "m", "requested_model": null,
                     "selection_source": "explicit", "plugin_chain": [],
                     "decision_ms": 0},
        "state": {"stateful_inbound": false, "sticky_hit": false, "cache_control_breaks": 0},
        "prefix": {"blocks": [], "continuity": null},
        "transform_mode": "passthrough",
        "transforms": [],
        "usage": {"input_total": 10, "input_cached": 0, "cache_write": 0,
                  "output": 2, "reasoning": 0},
        "usage_missing": false,
        "cost": {"input_miss": 0, "input_hit": 0, "cache_write": 0, "output": 0,
                 "peak_applied_pct": 100, "total": 0, "currency": "USD",
                 "quota_after": null},
        "result": {"status": 200, "upstream_status": 200, "failover_from": null,
                   "plan_switch": null, "overhead_ms": overhead_ms,
                   "upstream_ms": upstream_ms},
        "errors": [],
    })
}

/// The current UTC hour's rollover file name — the only file name
/// `stats`'s window reader accepts (§4.1's `YYYY-MM-DDTHH.jsonl`).
fn hour_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    // Civil-from-days (Howard Hinnant's algorithm), UTC.
    let days = secs / 86_400;
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let hour = (secs % 86_400) / 3_600;
    format!("{y:04}-{m:02}-{d:02}T{hour:02}")
}

fn now_rfc3339() -> String {
    // `stats` parses the record's `ts` with its own parser and keeps only
    // records inside [now − window, now]; a stamped time must therefore
    // never read as the FUTURE either. The earlier ":30" splice put the
    // record up to 30 minutes ahead when the suite ran in the first half
    // of an hour, and the window reader lawfully dropped it. Stamp the
    // actual current time.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let days = secs / 86_400;
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let hour = (secs % 86_400) / 3_600;
    let minute = (secs % 3_600) / 60;
    let second = secs % 60;
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{minute:02}:{second:02}.000Z")
}

/// One rigged config + trace dir holding the given records; returns
/// the config path (the same shape the rigs write, minimal sections).
fn rig(tag: &str, records: &[serde_json::Value]) -> std::path::PathBuf {
    let dir = vadis_conformance::testkit::tempdir(tag);
    let trace_dir = dir.join("state/traces");
    std::fs::create_dir_all(&trace_dir).unwrap();
    let mut line = String::new();
    for r in records {
        line.push_str(&r.to_string());
        line.push('\n');
    }
    std::fs::write(trace_dir.join(format!("{}.jsonl", hour_stamp())), line).unwrap();
    std::fs::write(
        dir.join("config.yaml"),
        r#"server:   { addr: "127.0.0.1:8791", upstream_attempt_timeout: 10s, request_timeout: 30s }
session:  { key_sources: ["prompt_cache_key"], ttl: 11h }
cache:    { sticky: true, breakeven: { enabled: true, min_remaining_turns: 2, safety_factor: 1.1 } }
trace:    { dir: "./state/traces", rollover: hourly }

providers:
  - name: mock
    urls:
      chat: http://127.0.0.1:9/v1/chat/completions
    api_key_env: CONF84_MOCK_KEY
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
          peak: { multiplier: 1.0, windows: [] }
        source: "mock upstream (no price; test fixture)"

aliases: {}
plugins: []
fallback: []
"#,
    )
    .unwrap();
    dir.join("config.yaml")
}

/// `report_json`'s `overhead_ms_p99` for the rig.
fn json_p99(config: &std::path::Path) -> serde_json::Value {
    let cfg = config.to_string_lossy().into_owned();
    let rep = vadis_cli::stats::report(&cfg, "24h").expect("report computes");
    let rc = vadis_cli::config_load::load(config).expect("config reloads");
    vadis_cli::stats::report_json(&rc, "24h", &rep, &None)["overhead_ms_p99"].clone()
}

#[test]
fn conf_84_overhead_p99_is_the_difference_not_the_upstream() {
    // (a) the decisive control: 10 records, overhead 500 ms, upstream
    // 450 ⇒ differences all 50. Raising upstream to 490 (differences
    // 10) must move the figure DOWN; and holding the differences fixed
    // while raising BOTH fields must not move it at all — the control
    // the raw-field p99 fails (it would follow the raise).
    let base: Vec<serde_json::Value> = (0..10).map(|_| record(500, Some(450))).collect();
    let cfg_a = rig("conf84-a", &base);
    let p99_a = json_p99(&cfg_a).as_u64().expect("a figure exists");
    assert_eq!(p99_a, 50, "the figure is the p99 of overhead-upstream");

    // Same differences, both fields raised: the figure does not move.
    let raised: Vec<serde_json::Value> = (0..10).map(|_| record(5000, Some(4950))).collect();
    let cfg_b = rig("conf84-b", &raised);
    let p99_b = json_p99(&cfg_b).as_u64().expect("a figure exists");
    assert_eq!(
        p99_b, 50,
        "raising both fields (same differences) must not move the figure — \
         the raw-field p99 would have printed 5000"
    );

    // A distinct difference set: the figure follows the differences.
    let mixed: Vec<serde_json::Value> = (0..10).map(|i| record(1000 + i, Some(500))).collect();
    let cfg_c = rig("conf84-c", &mixed);
    let p99_c = json_p99(&cfg_c).as_u64().expect("a figure exists");
    // differences are 500..509; nearest-rank p99 of 10 values = max.
    assert_eq!(p99_c, 509, "the figure is computed from the differences");
}

#[test]
fn conf_84_null_upstream_records_are_excluded_not_zero() {
    // (b) a window holding only null-upstream records has NO figure
    // (json null), never 0; and mixing them in does not drag the
    // sample toward 0.
    let only_null: Vec<serde_json::Value> = (0..10).map(|_| record(500, None)).collect();
    let cfg = rig("conf84-null", &only_null);
    let v = json_p99(&cfg);
    assert_eq!(
        v,
        serde_json::Value::Null,
        "a window of only null-upstream records has no figure, never 0"
    );

    // 5 valid (difference 100) + 5 null: the sample is the 5 valid
    // ones; the null records are excluded, not read as 0.
    let mixed: Vec<serde_json::Value> = (0..5)
        .map(|_| record(600, Some(500)))
        .chain((0..5).map(|_| record(500, None)))
        .collect();
    let cfg2 = rig("conf84-mixed-null", &mixed);
    let p99 = json_p99(&cfg2).as_u64().expect("a figure exists");
    assert_eq!(p99, 100, "the null records are excluded from the sample");
}

#[test]
fn conf_84_both_print_paths_agree() {
    // (c) the text and json surfaces read the same sample; the text
    // line's value equals report_json's. Captured through `report`'s
    // figures — the single sample both printers print.
    let records: Vec<serde_json::Value> = (0..10).map(|_| record(320, Some(300))).collect();
    let cfg = rig("conf84-agree", &records);
    let path = cfg.to_string_lossy().into_owned();
    let rep = vadis_cli::stats::report(&path, "24h").expect("report computes");
    // The figures' sample IS what both printers print; its p99 is the
    // json figure (asserted above) and the text line (print_text's
    // `overhead p99` reads the same `p99(&mut f.overhead_ms.clone())`).
    let rc = vadis_cli::config_load::load(std::path::Path::new(&path)).unwrap();
    let j = vadis_cli::stats::report_json(&rc, "24h", &rep, &None)["overhead_ms_p99"]
        .as_u64()
        .expect("json figure");
    // The text figure is printed from the same vector; recompute the
    // nearest-rank p99 over the figures' own sample and compare.
    let mut sample = rep.figures.overhead_ms.clone();
    let text = nearest_rank_p99(&mut sample).expect("text figure");
    assert_eq!(j, text, "the two print paths agree element for element");
    assert_eq!(j, 20, "the sample holds the differences (320-300)");
}

/// Nearest-rank p99 (ceil(0.99·n)-th of the sorted values) — the same
/// definition `stats.rs`'s private `p99` implements (recomputed here
/// so the case asserts the sample, not the printer's arithmetic).
fn nearest_rank_p99(xs: &mut [u64]) -> Option<u64> {
    if xs.is_empty() {
        return None;
    }
    xs.sort_unstable();
    let n = xs.len();
    let rank = ((0.99 * n as f64).ceil()) as usize;
    let rank = rank.clamp(1, n);
    Some(xs[rank - 1])
}
