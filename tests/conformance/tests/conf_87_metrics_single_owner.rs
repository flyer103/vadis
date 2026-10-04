//! CONF-87 (spec §4.16 + §9.2's provenance table + §4.8 + §4.1, ADR-041
//! §4–§5): **the `/metrics` numbers are §9.2's, rendered — and the
//! surface cannot invent one.** The single-owner rule, asserted seven
//! ways (ADR-041 §5's table):
//!
//! - (a) **the figures are the derivation's.** Every served value equals
//!   the case's OWN independently computed count/sum/quantile/ratio over
//!   the trace records the run itself produced (CONF-41's method), and
//!   the figures both surfaces carry also equal `stats::report_json`'s
//!   for the same window (CONF-56's method) — one derivation, two
//!   readers. No series exists outside the frozen set of 21 names.
//! - (b) **the series set is a function of the config, not of traffic**:
//!   two rigs holding N and 10N records in the window produce identical
//!   metric-name+label sets and identical digit-stripped line multisets;
//!   both bodies stay under 8 KiB.
//! - (c) **the read is bounded and sourced**: `router_trace_files_read`
//!   equals the §4.1 rollover files the window actually intersects
//!   (≤ 2), and a decoy trace dir full of in-window records OUTSIDE the
//!   config's `trace.dir` contributes nothing.
//! - (d) **determinism**: two admitted scrapes with no intervening
//!   traffic are byte-identical (no timestamp, no uptime).
//! - (e) **zero is not absent**: a window with records but no input
//!   tokens keeps the token series at `0` and OMITS the ratio series,
//!   the `# vadis:` comment naming why; a window whose `trace.dir` is
//!   removed after boot answers `200` with every trace-derived figure
//!   absent, each hole named, `router_metrics_omitted_figures` counting
//!   them, and never a §8 body.
//! - (f) **the `provenance` label is §9.2's own word per series** and is
//!   never re-labelled.
//! - (g) **the formatter cannot see a record** — the structural half:
//!   `metrics::exposition` is called directly with a hand-built
//!   `TraceFigures` and renders exactly the series those values imply
//!   (CONF-41/CONF-56's exposed-builder precedent).
//!
//! Offline: mock upstreams and pre-seeded trace files only — no provider
//! is dialled, no credential is read.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::Path;

use vadis_conformance::testkit::{self, PlanRig};
use serde_json::Value;

const POLICY_1H: &str = "  family: m1\n  primary: p-plan/m1\n  overflow: p-api/m1\n  on_primary_exhausted: spill\n  recover: probe\n  cooldown: 1h";

/// The frozen series set (spec §4.16's table): nothing else is ever
/// emitted — limb (a)'s "no series exists that the derivation does not
/// produce".
const FROZEN_SERIES: [&str; 21] = [
    "router_metrics_window_seconds",
    "router_metrics_omitted_figures",
    "router_trace_files_read",
    "router_requests",
    "router_requests_succeeded",
    "router_requests_failed",
    "router_failures_by_kind",
    "router_requests_usage_missing",
    "router_cost_nano",
    "router_cache_input_cached_tokens",
    "router_cache_input_tokens",
    "router_cache_hit_rate",
    "router_prefix_continuity_p50",
    "router_transform_savings_tokens",
    "router_plan_switches",
    "router_plan_switch_cost_nano",
    "router_plan_switch_reprefill_tokens",
    "router_plan_switch_reprefill_cost_nano",
    "router_plan_switches_without_usage",
    "router_stateful_inbound_rate",
    "router_overhead_ms_p99",
];

/// §9.2's own label per series (limb (f)): `None` = the series carries
/// no `provenance` label at all.
fn expected_provenance(name: &str) -> Option<&'static [&'static str]> {
    match name {
        "router_cost_nano"
        | "router_cache_input_cached_tokens"
        | "router_cache_input_tokens"
        | "router_cache_hit_rate"
        | "router_plan_switch_cost_nano" => Some(&["verified"]),
        "router_prefix_continuity_p50"
        | "router_plan_switch_reprefill_tokens"
        | "router_plan_switch_reprefill_cost_nano" => Some(&["inferred"]),
        "router_transform_savings_tokens" => Some(&["verified", "inferred"]),
        "router_stateful_inbound_rate" => Some(&["count"]),
        "router_overhead_ms_p99" => Some(&["measured"]),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The exposition parser (test-side: no serde, the format is line-based)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Sample {
    name: String,
    labels: Vec<(String, String)>,
    value: String,
}

/// (samples, `# vadis:` comments) — comment lines are the omission
/// arms' names; HELP/TYPE lines are skipped.
fn parse_exposition(body: &str) -> (Vec<Sample>, Vec<String>) {
    let mut samples = Vec::new();
    let mut comments = Vec::new();
    for line in body.lines() {
        if let Some(c) = line.strip_prefix("# vadis: ") {
            comments.push(c.to_string());
            continue;
        }
        if line.starts_with("# ") || line.trim().is_empty() {
            continue;
        }
        let sp = line.rfind(' ').expect("a sample has a value");
        let (lhs, value) = (&line[..sp], line[sp + 1..].to_string());
        let (name, labels) = match lhs.find('{') {
            Some(ob) => {
                let inner = &lhs[ob + 1..lhs.len() - 1];
                let labels = inner
                    .split(',')
                    .map(|p| {
                        let (k, v) = p.split_once('=').expect("k=v");
                        (k.to_string(), v.trim_matches('"').to_string())
                    })
                    .collect();
                (lhs[..ob].to_string(), labels)
            }
            None => (lhs.to_string(), Vec::new()),
        };
        samples.push(Sample {
            name,
            labels,
            value,
        });
    }
    (samples, comments)
}

fn value_of<'s>(samples: &'s [Sample], name: &str, labels: &[(&str, &str)]) -> Option<&'s str> {
    samples
        .iter()
        .find(|s| {
            s.name == name
                && labels
                    .iter()
                    .all(|(k, v)| s.labels.iter().any(|(sk, sv)| sk == k && sv == v))
        })
        .map(|s| s.value.as_str())
}

fn has_series(samples: &[Sample], name: &str) -> bool {
    samples.iter().any(|s| s.name == name)
}

/// Every sample is in the frozen set, carries §9.2's own provenance word
/// for its series (never re-labelled), and no unlisted series carries a
/// `provenance` label at all.
fn assert_frozen_set_and_provenance(samples: &[Sample]) {
    for s in samples {
        assert!(
            FROZEN_SERIES.contains(&s.name.as_str()),
            "series {} is outside the frozen set of 21 (spec §4.16)",
            s.name
        );
        let carried: Vec<&str> = s
            .labels
            .iter()
            .filter(|(k, _)| k == "provenance")
            .map(|(_, v)| v.as_str())
            .collect();
        match expected_provenance(&s.name) {
            Some(words) => {
                assert_eq!(
                    carried.len(),
                    1,
                    "{} carries exactly one provenance label",
                    s.name
                );
                assert!(
                    words.contains(&carried[0]),
                    "{} re-labelled as {:?} (§9.2's word is {:?})",
                    s.name,
                    carried[0],
                    words
                );
            }
            None => assert!(
                carried.is_empty(),
                "{} must not carry a provenance label",
                s.name
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// HTTP + trace helpers
// ---------------------------------------------------------------------------

fn http_get_metrics(addr: &str) -> (u16, Vec<u8>, Vec<(String, String)>) {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(addr).expect("connect");
    let req = format!("GET /metrics HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    let split = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("head/body split");
    let head = String::from_utf8_lossy(&buf[..split]).into_owned();
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("status line");
    let headers = head
        .lines()
        .skip(1)
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    (status, buf[split + 4..].to_vec(), headers)
}

fn header<'h>(headers: &'h [(String, String)], name: &str) -> Option<&'h str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn trace_records(trace_dir: &Path) -> Vec<Value> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(trace_dir).expect("trace dir") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        for line in std::fs::read_to_string(path).unwrap().lines() {
            if !line.trim().is_empty() {
                out.push(serde_json::from_str(line).expect("record json"));
            }
        }
    }
    out
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Epoch ms → RFC3339 UTC with millis (test-side, over vadis-core's own
/// civil-from-days helper).
fn rfc3339(ms: i64) -> String {
    let s = (ms.div_euclid(1_000)).max(0) as u64;
    let milli = ms.rem_euclid(1_000) as u64;
    let (y, mo, d, minute, _) = vadis_core::peak::timestamp_parts(s, vadis_core::peak::Tz::Utc);
    format!(
        "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}.{milli:03}Z",
        minute / 60,
        minute % 60,
        s % 60
    )
}

/// Epoch ms → the §4.1 rollover file name of its UTC hour.
fn hour_file_name(ms: i64) -> String {
    let s = (ms.div_euclid(1_000)).max(0) as u64;
    let (y, mo, d, minute, _) = vadis_core::peak::timestamp_parts(s, vadis_core::peak::Tz::Utc);
    format!("{y:04}-{mo:02}-{d:02}T{:02}.jsonl", minute / 60)
}

/// `2026-09-19T07` → epoch ms of that UTC hour (test-side parser).
fn hour_stem_ms(stem: &str) -> Option<i64> {
    let (date, hour) = stem.split_once('T')?;
    let mut d = date.split('-');
    let y: i64 = d.next()?.parse().ok()?;
    let mo: u32 = d.next()?.parse().ok()?;
    let day: u32 = d.next()?.parse().ok()?;
    let h: i64 = hour.parse().ok()?;
    Some(vadis_core::peak::utc_midnight_epoch(y, mo, day) as i64 * 1_000 + h * 3_600_000)
}

/// How many §4.1 files the 900s window ending at `now` intersects —
/// computed test-side, never read off the implementation.
fn expected_files_read(trace_dir: &Path, now: i64) -> usize {
    let start = now - 900_000;
    let mut n = 0;
    for entry in std::fs::read_dir(trace_dir).expect("trace dir") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let Some(hour_ms) = hour_stem_ms(stem) else {
            continue;
        };
        if hour_ms <= now && hour_ms + 3_600_000 >= start {
            n += 1;
        }
    }
    n
}

// ---------------------------------------------------------------------------
// The independent derivation (CONF-41's method): every figure the case
// asserts is computed HERE from the raw records, never read off a vadis
// type. It mirrors spec §9.2's provenance table row by row.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Own {
    requests: u64,
    succeeded: u64,
    failed: u64,
    failure_kinds: BTreeMap<String, u64>,
    usage_missing: u64,
    currencies: BTreeMap<String, ()>,
    input_miss: BTreeMap<String, u64>,
    input_hit: BTreeMap<String, u64>,
    cache_write: BTreeMap<String, u64>,
    output: BTreeMap<String, u64>,
    input_total: u64,
    input_cached: u64,
    continuity: Vec<f64>,
    stateful: u64,
    overhead: Vec<u64>,
    switches: u64,
    switch_cost: BTreeMap<String, u64>,
    reprefill_tokens: u64,
    reprefill_cost: BTreeMap<String, u64>,
    switches_without_usage: u64,
}

fn compute_own(recs: &[Value]) -> Own {
    let mut o = Own::default();
    for r in recs {
        o.requests += 1;
        let status = r["result"]["status"].as_u64();
        if matches!(status, Some(s) if (200..300).contains(&s)) {
            o.succeeded += 1;
        } else {
            o.failed += 1;
            if let Some(errs) = r["errors"].as_array() {
                for e in errs {
                    if let Some(k) = e["kind"].as_str() {
                        *o.failure_kinds.entry(k.to_string()).or_insert(0) += 1;
                    }
                }
            }
        }
        if r["usage_missing"].as_bool() == Some(true) {
            o.usage_missing += 1;
            continue;
        }
        if let Some(cost) = r.get("cost") {
            let cur = cost["currency"].as_str().unwrap_or("USD").to_string();
            o.currencies.insert(cur.clone(), ());
            let add = |m: &mut BTreeMap<String, u64>, key: &str| {
                *m.entry(cur.clone()).or_insert(0) += cost[key].as_u64().unwrap_or(0);
            };
            add(&mut o.input_miss, "input_miss");
            add(&mut o.input_hit, "input_hit");
            add(&mut o.cache_write, "cache_write");
            add(&mut o.output, "output");
        }
        if let Some(u) = r.get("usage") {
            o.input_total += u["input_total"].as_u64().unwrap_or(0);
            o.input_cached += u["input_cached"].as_u64().unwrap_or(0);
        }
        if let Some(c) = r["prefix"]["continuity"].as_f64() {
            o.continuity.push(c);
        }
        if r["state"]["stateful_inbound"].as_bool() == Some(true) {
            o.stateful += 1;
        }
        if let (Some(oh), Some(up)) = (
            r["result"]["overhead_ms"].as_u64(),
            r["result"]["upstream_ms"].as_u64(),
        ) {
            o.overhead.push(oh.saturating_sub(up));
        }
        let sw = &r["result"]["plan_switch"];
        if sw.is_object() {
            o.switches += 1;
            let sw_cur = sw["cost_currency"].as_str().unwrap_or("USD").to_string();
            o.reprefill_tokens += sw["reprefill_tokens"].as_u64().unwrap_or(0);
            *o.reprefill_cost.entry(sw_cur).or_insert(0) +=
                sw["switch_cost_nano"].as_u64().unwrap_or(0);
            let cur = r["cost"]["currency"].as_str().unwrap_or("USD").to_string();
            *o.switch_cost.entry(cur).or_insert(0) += r["cost"]["total"].as_u64().unwrap_or(0);
        }
    }
    o
}

fn median_own(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    let mut v = xs.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

fn p99_own(xs: &[u64]) -> Option<u64> {
    if xs.is_empty() {
        return None;
    }
    let mut v = xs.to_vec();
    v.sort_unstable();
    let rank = ((v.len() as f64) * 0.99).ceil() as usize;
    Some(v[rank.saturating_sub(1).min(v.len() - 1)])
}

/// Assert every series the derivation produced, limb by limb, against
/// the case's own sums. `family` is the configured plan family, if any.
fn assert_figures(samples: &[Sample], comments: &[String], own: &Own, family: Option<&str>) {
    assert_eq!(
        value_of(samples, "router_metrics_window_seconds", &[]),
        Some("900")
    );
    assert_eq!(
        value_of(samples, "router_requests", &[]),
        Some(own.requests.to_string().as_str())
    );
    assert_eq!(
        value_of(samples, "router_requests_succeeded", &[]),
        Some(own.succeeded.to_string().as_str())
    );
    assert_eq!(
        value_of(samples, "router_requests_failed", &[]),
        Some(own.failed.to_string().as_str())
    );
    for (kind, n) in &own.failure_kinds {
        assert_eq!(
            value_of(samples, "router_failures_by_kind", &[("kind", kind)]),
            Some(n.to_string().as_str()),
            "failure kind {kind}"
        );
    }
    assert_eq!(
        samples
            .iter()
            .filter(|s| s.name == "router_failures_by_kind")
            .count(),
        own.failure_kinds.len(),
        "exactly the kinds the window produced — never a synthetic zero"
    );
    assert_eq!(
        value_of(samples, "router_requests_usage_missing", &[]),
        Some(own.usage_missing.to_string().as_str())
    );
    if own.currencies.is_empty() {
        assert!(!has_series(samples, "router_cost_nano"));
        assert!(
            comments
                .iter()
                .any(|c| c.contains("no currency in the window")),
            "the money hole is named"
        );
    } else {
        for cur in own.currencies.keys() {
            for (tier, map) in [
                ("input_miss", &own.input_miss),
                ("input_hit", &own.input_hit),
                ("cache_write", &own.cache_write),
                ("output", &own.output),
            ] {
                assert_eq!(
                    value_of(
                        samples,
                        "router_cost_nano",
                        &[
                            ("tier", tier),
                            ("currency", cur),
                            ("provenance", "verified")
                        ]
                    ),
                    Some(map.get(cur).copied().unwrap_or(0).to_string().as_str()),
                    "cost {tier} {cur}"
                );
            }
        }
    }
    assert_eq!(
        value_of(
            samples,
            "router_cache_input_cached_tokens",
            &[("provenance", "verified")]
        ),
        Some(own.input_cached.to_string().as_str())
    );
    assert_eq!(
        value_of(
            samples,
            "router_cache_input_tokens",
            &[("provenance", "verified")]
        ),
        Some(own.input_total.to_string().as_str())
    );
    if own.input_total == 0 {
        assert!(!has_series(samples, "router_cache_hit_rate"));
        assert!(
            comments
                .iter()
                .any(|c| c.contains("cache_hit_rate omitted — no input tokens in the window")),
            "the hit-rate hole is named"
        );
    } else {
        let want = format!("{:.4}", own.input_cached as f64 / own.input_total as f64);
        assert_eq!(
            value_of(
                samples,
                "router_cache_hit_rate",
                &[("provenance", "verified")]
            ),
            Some(want.as_str())
        );
    }
    match median_own(&own.continuity) {
        None => {
            assert!(!has_series(samples, "router_prefix_continuity_p50"));
            assert!(
                comments
                    .iter()
                    .any(|c| c.contains("prefix_continuity_p50 omitted")),
                "the continuity hole is named"
            );
        }
        Some(m) => {
            let want = format!("{m:.4}");
            assert_eq!(
                value_of(
                    samples,
                    "router_prefix_continuity_p50",
                    &[("provenance", "inferred")]
                ),
                Some(want.as_str())
            );
        }
    }
    assert!(
        !has_series(samples, "router_transform_savings_tokens"),
        "this traffic ran no transforms — no savings series exists"
    );
    match family {
        None => assert!(
            !has_series(samples, "router_plan_switches"),
            "no plan_policy ⇒ no plan series at all (§9.1's rule)"
        ),
        Some(fam) => {
            assert_eq!(
                value_of(samples, "router_plan_switches", &[("family", fam)]),
                Some(own.switches.to_string().as_str())
            );
            for cur in own.currencies.keys() {
                assert_eq!(
                    value_of(
                        samples,
                        "router_plan_switch_cost_nano",
                        &[
                            ("family", fam),
                            ("currency", cur),
                            ("provenance", "verified")
                        ]
                    ),
                    Some(
                        own.switch_cost
                            .get(cur)
                            .copied()
                            .unwrap_or(0)
                            .to_string()
                            .as_str()
                    ),
                    "switch cost {cur}"
                );
                assert_eq!(
                    value_of(
                        samples,
                        "router_plan_switch_reprefill_cost_nano",
                        &[
                            ("family", fam),
                            ("currency", cur),
                            ("provenance", "inferred")
                        ]
                    ),
                    Some(
                        own.reprefill_cost
                            .get(cur)
                            .copied()
                            .unwrap_or(0)
                            .to_string()
                            .as_str()
                    ),
                    "reprefill cost {cur}"
                );
            }
            assert_eq!(
                value_of(
                    samples,
                    "router_plan_switch_reprefill_tokens",
                    &[("family", fam), ("provenance", "inferred")]
                ),
                Some(own.reprefill_tokens.to_string().as_str())
            );
            assert_eq!(
                value_of(
                    samples,
                    "router_plan_switches_without_usage",
                    &[("family", fam)]
                ),
                Some(own.switches_without_usage.to_string().as_str())
            );
        }
    }
    if own.requests == 0 {
        assert!(!has_series(samples, "router_stateful_inbound_rate"));
        assert!(
            comments
                .iter()
                .any(|c| c.contains("stateful_inbound_rate omitted")),
            "the stateful-rate hole is named"
        );
    } else {
        let want = format!("{:.4}", own.stateful as f64 / own.requests as f64);
        assert_eq!(
            value_of(
                samples,
                "router_stateful_inbound_rate",
                &[("provenance", "count")]
            ),
            Some(want.as_str())
        );
    }
    match p99_own(&own.overhead) {
        None => {
            assert!(!has_series(samples, "router_overhead_ms_p99"));
            assert!(
                comments
                    .iter()
                    .any(|c| c.contains("overhead_ms_p99 omitted")),
                "the overhead hole is named"
            );
        }
        Some(p) => assert_eq!(
            value_of(
                samples,
                "router_overhead_ms_p99",
                &[("provenance", "measured")]
            ),
            Some(p.to_string().as_str())
        ),
    }
    // The constant omission (spec §4.16, "the one figure that is not
    // here") and the response's own bookkeeping.
    assert!(
        comments
            .iter()
            .any(|c| c.contains("unknown_outcome_requests omitted")),
        "the event-log figure is named, always"
    );
    assert_eq!(
        value_of(samples, "router_metrics_omitted_figures", &[]),
        Some(comments.len().to_string().as_str()),
        "omitted_figures is exactly this response's comment count"
    );
    assert_frozen_set_and_provenance(samples);
}

// ---------------------------------------------------------------------------
// (a) + (c) + (d): the figures are the derivation's, the read is bounded
// and sourced, and two scrapes are byte-identical.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_87_the_figures_are_the_derivations() {
    let (plan, api, dir, listen_addr) = testkit::plan_rig_parts("conf87-main", "", POLICY_1H).await;
    plan.queue(testkit::plan_ok("t1"));
    plan.queue(testkit::plan_forbidden_403());
    api.queue(testkit::plan_ok("spilled"));
    api.queue(testkit::CannedResponse::json(
        500,
        "Internal Server Error",
        br#"{"error":{"message":"boom","type":"server_error"}}"#,
    ));

    let cfg = dir.join("config.yaml").to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    let rig = PlanRig {
        plan,
        api,
        listen_addr: listen_addr.clone(),
        dir: dir.clone(),
        serve_task,
    };

    // Turn 1 in-plan; turn 2 meets the 403 and spills; turn 3 (family on
    // overflow) meets the 500 — three records, one of them failed.
    let (s1, _b, _h) = rig.post(Some("S1"), 1);
    assert_eq!(s1, 200);
    let (s2, _b, _h) = rig.post(Some("S1"), 2);
    assert_eq!(s2, 200, "turn 2 spills to the metered account");
    let (s3, _b, _h) = rig.post(Some("S1"), 3);
    assert_eq!(s3, 502, "the exhausted chain surfaces as 502 (spec §8)");

    // (c) the decoy: a full trace dir OUTSIDE the config's trace.dir,
    // holding an in-window record with absurd usage. It must contribute
    // nothing.
    let decoy = dir.join("state/decoy_traces");
    std::fs::create_dir_all(&decoy).unwrap();
    let decoy_rec = serde_json::json!({
        "schema_version": 2,
        "ts": rfc3339(now_ms()),
        "usage_missing": false,
        "usage": {"input_total": 999_999_999_u64, "input_cached": 999_999_999_u64},
        "cost": {"currency": "USD", "input_miss": 999_999_999_u64, "total": 999_999_999_u64},
        "result": {"status": 200, "overhead_ms": 1, "upstream_ms": 0},
    });
    std::fs::write(
        decoy.join(hour_file_name(now_ms())),
        format!("{decoy_rec}\n"),
    )
    .unwrap();

    // The scrape, twice: (d) determinism before anything else is read.
    let (status, body1, headers) = http_get_metrics(&listen_addr);
    assert_eq!(status, 200);
    assert_eq!(
        header(&headers, "content-type"),
        Some("text/plain; version=0.0.4; charset=utf-8")
    );
    let (status2, body2, _h2) = http_get_metrics(&listen_addr);
    assert_eq!(status2, 200);
    assert_eq!(
        body1, body2,
        "(d) two scrapes over an unchanged window are byte-identical"
    );
    assert!(body1.len() < 8192, "the body is O(series), under 8 KiB");
    assert!(
        header(&headers, "x-router-request-id").is_none(),
        "the admitted arm is never §8's answer"
    );

    let text = String::from_utf8(body1).unwrap();
    let (samples, comments) = parse_exposition(&text);

    // (a) the independent sums over the run's own records.
    let recs = trace_records(&dir.join("state/traces"));
    assert_eq!(recs.len(), 3, "the run wrote exactly its three records");
    let own = compute_own(&recs);
    assert_eq!(own.failed, 1, "the 500 is the one failed record");
    assert_eq!(own.switches, 1, "the 403 displaced turn 2");
    assert_figures(&samples, &comments, &own, Some("m1"));
    assert!(
        comments
            .iter()
            .all(|c| !c.contains("999_999_999") && !c.contains("999999999")),
        "the decoy is nowhere"
    );

    // (c) the read is bounded: the case's own §4.1 intersection count,
    // ≤ 2 — and the decoy dir did not add to it.
    let files_read = value_of(&samples, "router_trace_files_read", &[])
        .expect("files_read present")
        .parse::<usize>()
        .unwrap();
    let want_files = expected_files_read(&dir.join("state/traces"), now_ms());
    assert_eq!(files_read, want_files, "the files the window intersects");
    assert!(files_read <= 2, "a 900s window intersects ≤ 2 hourly files");

    let dir = rig.stop();

    // (a), the second method (CONF-56's): the figures both surfaces
    // carry also equal `stats::report_json`'s for the same window.
    let cfg_path = dir.join("config.yaml");
    let rep = vadis_cli::stats::report(&cfg_path.to_string_lossy(), "15m").expect("report");
    let rc = vadis_cli::config_load::load(&cfg_path).expect("config reloads");
    let json = vadis_cli::stats::report_json(&rc, "15m", &rep, &None);
    assert_eq!(
        json["trace"]["files_read"].as_u64().unwrap() as usize,
        files_read,
        "files_read is the derivation's count"
    );
    assert_eq!(json["requests"]["total"].as_u64().unwrap(), own.requests);
    let json_rate = json["cache"]["hit_rate"]["value"].as_f64().expect("rate");
    let want_rate = format!("{json_rate:.4}");
    assert_eq!(
        value_of(
            &samples,
            "router_cache_hit_rate",
            &[("provenance", "verified")]
        ),
        Some(want_rate.as_str()),
        "the served ratio is report_json's ratio, one derivation"
    );
    let json_cont = json["cache"]["continuity_p50"]["value"]
        .as_f64()
        .expect("continuity");
    let want_cont = format!("{json_cont:.4}");
    assert_eq!(
        value_of(
            &samples,
            "router_prefix_continuity_p50",
            &[("provenance", "inferred")]
        ),
        Some(want_cont.as_str())
    );
    assert_eq!(
        json["overhead_ms_p99"].as_u64().unwrap(),
        p99_own(&own.overhead).unwrap(),
        "report_json's p99 is the case's own"
    );
    assert_eq!(
        json["plan_family"]["switches"].as_u64().unwrap(),
        own.switches
    );
    assert_eq!(
        json["plan_family"]["switch_cost_verified_nano"]
            .as_u64()
            .unwrap(),
        own.switch_cost.get("USD").copied().unwrap_or(0),
        "the single-currency scalar is the served switch cost"
    );
}

// ---------------------------------------------------------------------------
// (b): the series set is a function of the config, not of traffic.
// ---------------------------------------------------------------------------

fn plain_config_yaml(upstream_port: u16, listen_port: u16) -> String {
    format!(
        r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: mock
    urls:
      chat: http://127.0.0.1:{upstream_port}/v1/chat/completions
    api_key_env: CONF87_MOCK_KEY
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
plugins: []
fallback: []
"#
    )
}

/// Drive `n` fresh single-turn sessions through a rig and return the
/// scrape's body bytes.
async fn scrape_after_n_requests(tag: &str, n: u32) -> Vec<u8> {
    let dir = testkit::tempdir(tag);
    let upstream = testkit::MockUpstream::start().await.unwrap();
    for i in 0..n {
        upstream.queue(testkit::plan_ok(&format!("r{i}")));
    }
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(
        &config_path,
        plain_config_yaml(upstream.addr.port(), listen_port),
    )
    .unwrap();
    std::env::set_var("CONF87_MOCK_KEY", "sk-conf87");
    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);

    for i in 0..n {
        // Each request is turn 1 of its own session, so the two rigs
        // differ in COUNT only — never in which figures exist.
        let body = format!(
            r#"{{"model":"mock/glm","messages":[{{"role":"user","content":"turn {i}"}}],"prompt_cache_key":"sess-{tag}-{i}","stream":false}}"#
        );
        let (status, _b, _h) =
            testkit::http_post(&listen_addr, "/v1/chat/completions", body.as_bytes(), &[]);
        assert_eq!(status, 200);
    }
    let (status, body, _h) = http_get_metrics(&listen_addr);
    assert_eq!(status, 200);
    serve_task.abort();
    let _ = serve_task.await;
    body
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_87_series_set_is_config_not_traffic() {
    let body_n = scrape_after_n_requests("conf87-n1", 1).await;
    let body_10n = scrape_after_n_requests("conf87-n10", 10).await;
    assert!(
        body_n.len() < 8192 && body_10n.len() < 8192,
        "both bodies under 8 KiB"
    );

    let text_n = String::from_utf8(body_n).unwrap();
    let text_10n = String::from_utf8(body_10n).unwrap();
    let (samples_n, comments_n) = parse_exposition(&text_n);
    let (samples_10n, comments_10n) = parse_exposition(&text_10n);

    // Identical metric-name+label sets.
    let set_n: std::collections::BTreeSet<(&str, &Vec<(String, String)>)> = samples_n
        .iter()
        .map(|s| (s.name.as_str(), &s.labels))
        .collect();
    let set_10n: std::collections::BTreeSet<(&str, &Vec<(String, String)>)> = samples_10n
        .iter()
        .map(|s| (s.name.as_str(), &s.labels))
        .collect();
    assert_eq!(
        set_n, set_10n,
        "the series set is the config's, not the traffic's"
    );
    assert_eq!(
        comments_n, comments_10n,
        "the same omission arms fire in both windows"
    );

    // Identical digit-stripped line multisets: only the figures differ.
    let strip = |t: &str| -> Vec<String> {
        let mut v: Vec<String> = t
            .lines()
            .map(|l| l.chars().filter(|c| !c.is_ascii_digit()).collect())
            .collect();
        v.sort();
        v
    };
    assert_eq!(
        strip(&text_n),
        strip(&text_10n),
        "N and 10N records: identical lines modulo digits"
    );
    assert_eq!(
        text_n.lines().count(),
        text_10n.lines().count(),
        "identical line counts"
    );

    // And the counts themselves ARE the traffic's — the limbs above
    // would also pass on a frozen surface, so witness the difference.
    assert_eq!(value_of(&samples_n, "router_requests", &[]), Some("1"));
    assert_eq!(value_of(&samples_10n, "router_requests", &[]), Some("10"));
}

// ---------------------------------------------------------------------------
// (e): zero is not absent — the two shapes of a hole.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_87_zero_is_not_absent() {
    // Shape 1 — a window with records but NO input tokens: the token
    // series are present as 0, the ratio series is absent, and its
    // comment names why. The record is pre-seeded by hand so the shape
    // does not depend on any provider behaviour.
    let dir = testkit::tempdir("conf87-norate");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(
        &config_path,
        plain_config_yaml(upstream.addr.port(), listen_port),
    )
    .unwrap();
    std::env::set_var("CONF87_MOCK_KEY", "sk-conf87");
    let trace_dir = dir.join("state/traces");
    std::fs::create_dir_all(&trace_dir).unwrap();
    let seeded = serde_json::json!({
        "schema_version": 2,
        "ts": rfc3339(now_ms()),
        "usage_missing": false,
        "usage": {"input_total": 0, "input_cached": 0},
        "cost": {"currency": "USD", "input_miss": 100, "input_hit": 0, "cache_write": 0, "output": 50, "total": 150},
        "result": {"status": 200, "overhead_ms": 10, "upstream_ms": 4},
    });
    std::fs::write(
        trace_dir.join(hour_file_name(now_ms())),
        format!("{seeded}\n"),
    )
    .unwrap();

    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);

    let (status, body, _h) = http_get_metrics(&listen_addr);
    assert_eq!(status, 200);
    let text = String::from_utf8(body).unwrap();
    let (samples, comments) = parse_exposition(&text);
    let own = compute_own(&[seeded]);
    assert_figures(&samples, &comments, &own, None);
    // The limb's own words: 0 is a read, the hole is named.
    assert_eq!(
        value_of(
            &samples,
            "router_cache_input_tokens",
            &[("provenance", "verified")]
        ),
        Some("0"),
        "the token series is present as a real zero"
    );
    assert!(
        !has_series(&samples, "router_cache_hit_rate"),
        "the ratio is ABSENT — never a 0"
    );
    assert!(
        comments
            .iter()
            .any(|c| c == "cache_hit_rate omitted — no input tokens in the window"),
        "the omission is named with its reason: {comments:?}"
    );
    assert_eq!(
        value_of(
            &samples,
            "router_overhead_ms_p99",
            &[("provenance", "measured")]
        ),
        Some("6"),
        "overhead_ms − upstream_ms, the derivation's own quantity"
    );

    serve_task.abort();
    let _ = serve_task.await;

    // Shape 2 — the trace dir is removed under a running process: `200`,
    // every trace-derived figure absent, each hole named, the
    // bookkeeping counting them, and never a §8 body.
    let dir = testkit::tempdir("conf87-gonedir");
    let upstream = testkit::MockUpstream::start().await.unwrap();
    let listen_port = testkit::free_port();
    let listen_addr = format!("127.0.0.1:{listen_port}");
    let config_path = dir.join("config.yaml");
    std::fs::write(
        &config_path,
        plain_config_yaml(upstream.addr.port(), listen_port),
    )
    .unwrap();
    let cfg = config_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { vadis_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    std::fs::remove_dir_all(dir.join("state/traces")).expect("the live dir removes");

    let (status, body, headers) = http_get_metrics(&listen_addr);
    assert_eq!(status, 200, "an unreadable window is not a failure");
    assert_eq!(
        header(&headers, "content-type"),
        Some("text/plain; version=0.0.4; charset=utf-8")
    );
    assert!(
        header(&headers, "x-router-request-id").is_none(),
        "never a §8 body on the admitted arm"
    );
    assert!(
        serde_json::from_slice::<Value>(&body).is_err(),
        "the body is the exposition, not a JSON error"
    );
    let text = String::from_utf8(body).unwrap();
    let (samples, comments) = parse_exposition(&text);
    assert!(
        comments
            .iter()
            .any(|c| c.starts_with("trace unreadable — ")),
        "the read's own failure is named: {comments:?}"
    );
    for arm in [
        "cache_hit_rate omitted",
        "prefix_continuity_p50 omitted",
        "overhead_ms_p99 omitted",
        "stateful_inbound_rate omitted",
        "no currency in the window",
        "unknown_outcome_requests omitted",
    ] {
        assert!(
            comments.iter().any(|c| c.contains(arm)),
            "arm {arm} is named"
        );
    }
    for gone in [
        "router_requests",
        "router_trace_files_read",
        "router_cost_nano",
        "router_cache_input_tokens",
        "router_cache_hit_rate",
        "router_overhead_ms_p99",
        "router_stateful_inbound_rate",
    ] {
        assert!(
            !has_series(&samples, gone),
            "{gone} is a trace-derived figure — absent on an unreadable window"
        );
    }
    assert_eq!(
        value_of(&samples, "router_metrics_window_seconds", &[]),
        Some("900"),
        "the window is the surface's own constant — present always"
    );
    assert_eq!(
        value_of(&samples, "router_metrics_omitted_figures", &[]),
        Some(comments.len().to_string().as_str()),
    );
    assert_eq!(
        comments.len(),
        7,
        "the read arm plus the five below it plus the constant one"
    );

    serve_task.abort();
    let _ = serve_task.await;
}

// ---------------------------------------------------------------------------
// (g): the formatter cannot see a record — the single-owner rule's
// structural half, asserted by calling `metrics::exposition` directly
// with a hand-built `TraceFigures` (CONF-41/CONF-56's exposed-builder
// precedent).
// ---------------------------------------------------------------------------

#[test]
fn conf_87_the_formatter_renders_exactly_the_figures_it_is_handed() {
    let mut f = vadis_cli::stats::TraceFigures::default();
    f.requests = 5;
    f.succeeded = 4;
    f.failed = 1;
    f.failure_kinds.insert("upstream_error".to_string(), 1);
    f.usage_missing = 1;
    f.currencies.insert("USD".to_string(), ());
    f.input_miss_nano.insert("USD".to_string(), 10);
    f.input_hit_nano.insert("USD".to_string(), 20);
    f.cache_write_nano.insert("USD".to_string(), 0);
    f.output_nano.insert("USD".to_string(), 30);
    f.input_cached_tokens = 50;
    f.input_total_tokens = 200;
    f.continuity = vec![0.5, 0.75];
    f.stateful_inbound = 1;
    f.overhead_ms = vec![3, 9, 1];
    f.verified_savings_tokens = 7;
    f.inferred_savings_tokens = 0;
    f.switches = 2;
    f.switch_cost_verified_nano.insert("USD".to_string(), 40);
    f.switches_without_usage = 1;
    f.reprefill_tokens = 11;
    f.reprefill_cost_nano.insert("USD".to_string(), 12);

    let text = vadis_cli::metrics::exposition(&f, 2, Some("fam-x"), None);
    let (samples, comments) = parse_exposition(&text);

    let at = |name: &str, labels: &[(&str, &str)]| value_of(&samples, name, labels);
    assert_eq!(at("router_metrics_window_seconds", &[]), Some("900"));
    assert_eq!(at("router_trace_files_read", &[]), Some("2"));
    assert_eq!(at("router_requests", &[]), Some("5"));
    assert_eq!(at("router_requests_succeeded", &[]), Some("4"));
    assert_eq!(at("router_requests_failed", &[]), Some("1"));
    assert_eq!(
        at("router_failures_by_kind", &[("kind", "upstream_error")]),
        Some("1")
    );
    assert_eq!(at("router_requests_usage_missing", &[]), Some("1"));
    // A held currency's zero tier is a READ — cache_write is present at 0.
    assert_eq!(
        at(
            "router_cost_nano",
            &[
                ("tier", "cache_write"),
                ("currency", "USD"),
                ("provenance", "verified")
            ]
        ),
        Some("0")
    );
    assert_eq!(
        at(
            "router_cost_nano",
            &[
                ("tier", "input_miss"),
                ("currency", "USD"),
                ("provenance", "verified")
            ]
        ),
        Some("10")
    );
    assert_eq!(
        at("router_cache_hit_rate", &[("provenance", "verified")]),
        Some("0.2500")
    );
    assert_eq!(
        at(
            "router_prefix_continuity_p50",
            &[("provenance", "inferred")]
        ),
        Some("0.6250")
    );
    assert_eq!(
        at(
            "router_transform_savings_tokens",
            &[("provenance", "verified")]
        ),
        Some("7")
    );
    assert!(
        value_of(
            &samples,
            "router_transform_savings_tokens",
            &[("provenance", "inferred")]
        )
        .is_none(),
        "a verdict the window did not produce gets no synthetic zero"
    );
    assert_eq!(
        at("router_plan_switches", &[("family", "fam-x")]),
        Some("2")
    );
    assert_eq!(
        at(
            "router_plan_switch_cost_nano",
            &[
                ("family", "fam-x"),
                ("currency", "USD"),
                ("provenance", "verified")
            ]
        ),
        Some("40")
    );
    assert_eq!(
        at(
            "router_plan_switch_reprefill_tokens",
            &[("family", "fam-x"), ("provenance", "inferred")]
        ),
        Some("11")
    );
    assert_eq!(
        at(
            "router_plan_switch_reprefill_cost_nano",
            &[
                ("family", "fam-x"),
                ("currency", "USD"),
                ("provenance", "inferred")
            ]
        ),
        Some("12")
    );
    assert_eq!(
        at("router_plan_switches_without_usage", &[("family", "fam-x")]),
        Some("1")
    );
    assert_eq!(
        at("router_stateful_inbound_rate", &[("provenance", "count")]),
        Some("0.2000")
    );
    assert_eq!(
        at("router_overhead_ms_p99", &[("provenance", "measured")]),
        Some("9")
    );
    assert_eq!(comments.len(), 1, "only the constant omission fires");
    assert_eq!(at("router_metrics_omitted_figures", &[]), Some("1"));
    assert_frozen_set_and_provenance(&samples);

    // No policy ⇒ no plan series at all, even with switch figures in the
    // input: the series set is the CONFIG's, and the formatter is handed
    // the family, never the config.
    let text = vadis_cli::metrics::exposition(&f, 2, None, None);
    let (samples, _c) = parse_exposition(&text);
    for plan_series in [
        "router_plan_switches",
        "router_plan_switch_cost_nano",
        "router_plan_switch_reprefill_tokens",
        "router_plan_switch_reprefill_cost_nano",
        "router_plan_switches_without_usage",
    ] {
        assert!(
            !has_series(&samples, plan_series),
            "{plan_series} cannot exist without a declared plan_policy"
        );
    }

    // The read-error arm, directly: every trace-derived figure is a
    // named hole and nothing else is emitted.
    let text = vadis_cli::metrics::exposition(
        &vadis_cli::stats::TraceFigures::default(),
        0,
        None,
        Some("a test-given reason"),
    );
    let (samples, comments) = parse_exposition(&text);
    assert!(
        comments
            .iter()
            .any(|c| c.starts_with("trace unreadable — a test-given reason")),
        "the reason the read gave is carried: {comments:?}"
    );
    assert_eq!(comments.len(), 7);
    let names: std::collections::BTreeSet<&str> = samples.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "router_metrics_window_seconds",
            "router_metrics_omitted_figures"
        ]
        .into_iter()
        .collect(),
        "only the surface's own two series survive an unreadable window"
    );
}
