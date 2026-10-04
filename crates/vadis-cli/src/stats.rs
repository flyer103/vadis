//! `vadis stats` (spec §9.2): the window's read-out over the trace files
//! and — read-only — the event log. Read-only by construction: the trace is
//! read directly, the store is opened with `SQLITE_OPEN_READ_ONLY` (no
//! create, no migration, not the writer role `serve` holds), and when even
//! that fails the one figure only the log holds is **omitted with a note**,
//! never estimated. Every figure carries its §7 label; a number that
//! cannot be computed is not printed (AGENTS constraints 4 and 5).

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use vadis_core::config::DurationVal;
use serde_json::Value;

/// Everything the report needs from the trace, already filtered to the
/// window by `ts` (spec §9.2: a record is counted when its own `ts` falls
/// inside — never by which file it happens to be in).
#[derive(Debug, Default)]
pub struct TraceFigures {
    pub requests: u64,
    pub succeeded: u64,
    pub failed: u64,
    /// `errors[].kind -> count`, failed records only (sorted on print).
    pub failure_kinds: BTreeMap<String, u64>,
    pub usage_missing: u64,
    /// Sums over records with usage present (an absent measurement is
    /// never read as 0) — **per currency** (spec §4.8/§9.2): a currency's
    /// figures never enter another currency's line, and no combined
    /// total exists. The map key is the ISO code (`"USD"` / `"CNY""),
    /// sorted by it (BTreeMap); a record whose `cost.currency` is absent
    /// is a v1 record and is USD by definition (spec §6).
    pub input_miss_nano: BTreeMap<String, u64>,
    pub input_hit_nano: BTreeMap<String, u64>,
    pub cache_write_nano: BTreeMap<String, u64>,
    pub output_nano: BTreeMap<String, u64>,
    /// The window's currency set (the report's `currencies:` header is
    /// printed from this; also decides the --json shape).
    pub currencies: BTreeMap<String, ()>,
    pub input_cached_tokens: u64,
    pub input_total_tokens: u64,
    /// Non-null `prefix.continuity` values, for the p50.
    pub continuity: Vec<f64>,
    pub verified_savings_tokens: u64,
    pub inferred_savings_tokens: u64,
    pub stateful_inbound: u64,
    pub overhead_ms: Vec<u64>,
    // plan family (present only when the config declares a policy)
    pub switches: u64,
    /// Σ the displaced records' own `cost.total` — the verified switch
    /// cost, **per currency**; excludes `usage_missing` records (counted
    /// in `switches_without_usage` instead).
    pub switch_cost_verified_nano: BTreeMap<String, u64>,
    pub switches_without_usage: u64,
    /// Σ `reprefill_tokens` / Σ `switch_cost_nano` over the switch records
    /// (the inferred columns; never added into the verified sum). The
    /// token sum is currency-free; the cost sum is grouped by
    /// `plan_switch.cost_currency` (spec §9.2's provenance row).
    pub reprefill_tokens: u64,
    pub reprefill_cost_nano: BTreeMap<String, u64>,
}

/// The one figure only the event log holds (ADR-010 item 4):
/// `upstream.submitted` in the window with no `upstream.responded` for the
/// same `request_id`. `log_was_read` is false when the store could not be
/// opened read-only — the figure is then omitted, never estimated.
#[derive(Debug, Default)]
pub struct EventFigures {
    pub unknown_outcome_requests: u64,
    pub log_was_read: bool,
}

/// Pure aggregation over the window's records (spec §9.2's provenance
/// table, one row per field). Records with `usage_missing: true`
/// contribute to nothing but their own count line.
pub fn aggregate(records: &[Value]) -> TraceFigures {
    let mut f = TraceFigures::default();
    for r in records {
        f.requests += 1;
        let usage_missing = r.get("usage_missing").and_then(Value::as_bool) == Some(true);
        let status = r
            .get("result")
            .and_then(|v| v.get("status"))
            .and_then(Value::as_u64);
        let succeeded = matches!(status, Some(s) if (200..300).contains(&s));
        if succeeded {
            f.succeeded += 1;
        } else {
            f.failed += 1;
            if let Some(errs) = r.get("errors").and_then(Value::as_array) {
                for e in errs {
                    if let Some(kind) = e.get("kind").and_then(Value::as_str) {
                        *f.failure_kinds.entry(kind.to_string()).or_insert(0) += 1;
                    }
                }
            }
        }
        if usage_missing {
            f.usage_missing += 1;
            continue;
        }
        if let Some(cost) = r.get("cost") {
            // §4.8: every money figure is grouped by the record's own
            // `cost.currency`; an absent field is a v1 record and is USD
            // by definition (spec §6 — an old file and a new one can sit
            // in one window).
            let cur = cost
                .get("currency")
                .and_then(Value::as_str)
                .unwrap_or("USD")
                .to_string();
            f.currencies.insert(cur.clone(), ());
            *f.input_miss_nano.entry(cur.clone()).or_insert(0) += cost_nano(cost, "input_miss");
            *f.input_hit_nano.entry(cur.clone()).or_insert(0) += cost_nano(cost, "input_hit");
            *f.cache_write_nano.entry(cur.clone()).or_insert(0) += cost_nano(cost, "cache_write");
            *f.output_nano.entry(cur.clone()).or_insert(0) += cost_nano(cost, "output");
        }
        if let Some(u) = r.get("usage") {
            f.input_total_tokens += u.get("input_total").and_then(Value::as_u64).unwrap_or(0);
            f.input_cached_tokens += u.get("input_cached").and_then(Value::as_u64).unwrap_or(0);
        }
        if let Some(c) = r
            .get("prefix")
            .and_then(|p| p.get("continuity"))
            .and_then(Value::as_f64)
        {
            f.continuity.push(c);
        }
        if let Some(ts) = r.get("transforms").and_then(Value::as_array) {
            for t in ts {
                let saved = t
                    .get("saved_input_tokens")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
                    + t.get("saved_output_tokens")
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                let saved = saved.max(0) as u64;
                match t.get("verdict").and_then(Value::as_str) {
                    Some("verified") => f.verified_savings_tokens += saved,
                    _ => f.inferred_savings_tokens += saved,
                }
            }
        }
        if r.get("state")
            .and_then(|s| s.get("stateful_inbound"))
            .and_then(Value::as_bool)
            == Some(true)
        {
            f.stateful_inbound += 1;
        }
        if let Some(oh) = r
            .get("result")
            .and_then(|x| x.get("overhead_ms"))
            .and_then(Value::as_u64)
        {
            // Spec §6 / §9.2 / DESIGN §12.16 (the R32-F5 repair): the
            // gate quantity is the vadis's OWN overhead —
            // `overhead_ms − upstream_ms` — and a record whose
            // `upstream_ms` is null (a boundary refusal, an unanswered
            // attempt) is EXCLUDED from the sample rather than read as
            // 0 ms. The raw `overhead_ms` field spans the whole request
            // (it includes the upstream attempt), so reading it
            // directly would print the upstream's own latency as the
            // vadis's.
            let upstream = r
                .get("result")
                .and_then(|x| x.get("upstream_ms"))
                .and_then(Value::as_u64);
            if let Some(up) = upstream {
                f.overhead_ms.push(oh.saturating_sub(up));
            }
        }
        // `plan_switch` is present-and-null when the policy did not
        // displace the request (spec §6); only a present object counts.
        let switched = r
            .get("result")
            .and_then(|x| x.get("plan_switch"))
            .is_some_and(|p| !p.is_null());
        if switched {
            f.switches += 1;
            let sw = &r["result"]["plan_switch"];
            // The inferred figures are grouped by the switch's own unit
            // (§9.2): `plan_switch.cost_currency`, USD when absent (v1).
            let sw_cur = sw
                .get("cost_currency")
                .and_then(Value::as_str)
                .unwrap_or("USD")
                .to_string();
            if usage_missing {
                f.switches_without_usage += 1;
            } else {
                // The switched request's own measured total IS the switch's
                // verified cost (the destination's re-prefill is inside it);
                // an in-plan destination verifies at 0 by the same rule.
                // Grouped by the record's `cost.currency` — the record, not
                // the switch, is what was measured.
                let cur = r
                    .get("cost")
                    .and_then(|c| c.get("currency"))
                    .and_then(Value::as_str)
                    .unwrap_or("USD")
                    .to_string();
                *f.switch_cost_verified_nano.entry(cur).or_insert(0) +=
                    r.get("cost").map(|c| cost_nano(c, "total")).unwrap_or(0);
            }
            f.reprefill_tokens += sw
                .get("reprefill_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let cost = sw
                .get("switch_cost_nano")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            *f.reprefill_cost_nano.entry(sw_cur).or_insert(0) += cost;
        }
    }
    f
}

fn cost_nano(cost: &Value, key: &str) -> u64 {
    cost.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// Everything `stats` computed for one invocation — the same figures it
/// prints, exposed so conformance (CONF-41) can assert them directly
/// against sums computed independently in the case itself.
pub struct Report {
    pub figures: TraceFigures,
    pub events: EventFigures,
    pub files_read: usize,
    pub start_ms: i64,
    pub now_ms: i64,
}

/// Compute the window's figures (the whole of `stats` minus the printing).
pub fn report(config_path: &str, window: &str) -> Result<Report, String> {
    let rc = crate::config_load::load(Path::new(config_path))
        .map_err(|reason| format!("vadis: {reason}"))?;
    // `--window` uses the config duration grammar — parsed by the same
    // `DurationVal` the config itself uses, so two grammars cannot drift.
    let window_val: DurationVal =
        serde_json::from_value(serde_json::Value::String(window.to_string()))
            .map_err(|e| format!("invalid window '{window}': {e}"))?;
    let now_ms = now_epoch_ms();
    let window_ms = window_val.0 as i64;
    let start_ms = now_ms.saturating_sub(window_ms);

    if !rc.trace_dir.is_dir() {
        return Err(format!(
            "trace directory {} does not exist (trace.dir, spec 4.1)",
            rc.trace_dir.display()
        ));
    }

    let (records, files_read) =
        read_window_records(&rc.trace_dir, start_ms, now_ms).map_err(|e| {
            format!(
                "cannot read trace directory {}: {e}",
                rc.trace_dir.display()
            )
        })?;
    let figs = aggregate(&records);

    // The event log, read-only; failure omits exactly one figure (the
    // partial-report rule — the rest is printed unchanged).
    let mut events = EventFigures::default();
    let mut unknown_note = None;
    let state_db = rc.state_db.clone();
    match vadis_store::SqliteStore::open_read_only(&state_db) {
        Ok(store) => {
            use vadis_core::store::{Query, QueryRow, Store as _};
            match store.query(Query::AllEvents) {
                Ok(QueryRow::Events(rows)) => {
                    events.log_was_read = true;
                    let start_us = start_ms * 1_000;
                    let end_us = now_ms * 1_000;
                    let mut submitted: BTreeMap<String, bool> = BTreeMap::new();
                    for ev in &rows {
                        match ev.kind_raw.as_str() {
                            "upstream.submitted" => {
                                if ev.ts_us >= start_us && ev.ts_us <= end_us {
                                    if let Some(rid) = &ev.request_id {
                                        // first submission per request in the window
                                        submitted.entry(rid.clone()).or_insert(false);
                                    }
                                }
                            }
                            "upstream.responded" => {
                                if let Some(rid) = &ev.request_id {
                                    if let Some(seen) = submitted.get_mut(rid) {
                                        *seen = true;
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    events.unknown_outcome_requests =
                        submitted.values().filter(|answered| !**answered).count() as u64;
                }
                Ok(_) => {
                    unknown_note =
                        Some("state store opened but the event log could not be read".into());
                }
                Err(e) => unknown_note = Some(format!("state store could not be read: {e}")),
            }
        }
        Err(e) => {
            unknown_note = Some(format!(
                "unknown outcome requests omitted: state store {} could not be opened \
                 read-only: {e}",
                state_db.display()
            ));
        }
    }
    if let Some(note) = &unknown_note {
        eprintln!("vadis: {note}");
    }
    Ok(Report {
        figures: figs,
        events,
        files_read,
        start_ms,
        now_ms,
    })
}

/// `vadis stats --config <path> --window <duration> [--json]`.
/// Exit codes (spec §9.2): `0` a report was produced; `2` the invocation
/// itself is unusable. Returns the process exit code.
pub fn stats(config_path: &str, window: &str, json: bool) -> i32 {
    let rep = match report(config_path, window) {
        Ok(r) => r,
        Err(reason) => {
            eprintln!("vadis: {reason}");
            return 2;
        }
    };
    let rc = match crate::config_load::load(Path::new(config_path)) {
        Ok(rc) => rc,
        Err(reason) => {
            eprintln!("vadis: {reason}");
            return 2;
        }
    };
    let unknown_note = if rep.events.log_was_read {
        None
    } else {
        Some("unknown outcome requests omitted (state store unreadable)".to_string())
    };

    if json {
        let v = report_json(&rc, window, &rep, &unknown_note);
        println!(
            "{}",
            serde_json::to_string_pretty(&v).expect("figures are JSON")
        );
    } else {
        print_text(
            &rc,
            window,
            rep.start_ms,
            rep.now_ms,
            rep.files_read,
            &rep.figures,
            &rep.events,
        );
    }
    0
}

// -- input -------------------------------------------------------------------

/// RFC3339 UTC (`2026-09-19T07:41:02.123Z`) → epoch milliseconds.
fn ts_to_epoch_ms(ts: &str) -> Option<i64> {
    let (date, rest) = ts.split_once('T')?;
    let mut d = date.split('-');
    let y: i64 = d.next()?.parse().ok()?;
    let mo: u32 = d.next()?.parse().ok()?;
    let day: u32 = d.next()?.parse().ok()?;
    let (time, frac) = rest.split_once('.').unwrap_or((rest, ""));
    let mut t = time.split(':');
    let h: i64 = t.next()?.parse().ok()?;
    let mi: i64 = t.next()?.trim_end_matches('Z').parse().ok()?;
    let s: i64 = t.next()?.trim_end_matches('Z').parse().ok()?;
    let ms: i64 = if frac.is_empty() {
        0
    } else {
        let mut f = frac.trim_end_matches('Z').to_string();
        while f.len() < 3 {
            f.push('0');
        }
        f[..3].parse().ok()?
    };
    Some(
        vadis_core::peak::utc_midnight_epoch(y, mo, day) as i64 * 1_000
            + h * 3_600_000
            + mi * 60_000
            + s * 1_000
            + ms,
    )
}

/// Read every trace file whose hourly range (§4.1: `YYYY-MM-DDTHH.jsonl`)
/// intersects the window, then keep the records whose own `ts` is inside
/// `[start_ms, now_ms]` — never by which file they happen to be in.
///
/// `pub(crate)` for the `/metrics` surface (spec §4.16, ADR-041 §4): the
/// window's read has ONE owner, shared by `vadis stats` and the scrape —
/// two readers of one derivation, never two derivations.
///
/// **The torn tail** (ADR-041 §4): the serving process appends to the
/// newest of these files while a reader scans it, so the *final* line of
/// the *newest* file may be observed mid-write. That one line, when it is
/// not valid JSON, is skipped — never fabricated, never counted (the next
/// read sees it whole). Every other malformed line stays the error it
/// always was.
pub(crate) fn read_window_records(
    dir: &Path,
    start_ms: i64,
    now_ms: i64,
) -> Result<(Vec<Value>, usize), String> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .collect();
    files.sort();
    // The files the window actually intersects, in name order — the last
    // of them is the newest, the one a live writer may be mid-line in.
    let targets: Vec<PathBuf> = files
        .into_iter()
        .filter(|path| {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let Some(hour_ms) = hour_start_ms(stem) else {
                return false; // not a rollover-named file; §4.1 names them all
            };
            let hour_end_ms = hour_ms + 3_600_000;
            hour_ms <= now_ms && hour_end_ms >= start_ms
        })
        .collect();
    let mut out = Vec::new();
    let mut read = 0usize;
    for (fi, path) in targets.iter().enumerate() {
        read += 1;
        let newest = fi == targets.len() - 1;
        let content =
            std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
        for (li, line) in lines.iter().enumerate() {
            let rec: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(e) => {
                    if newest && li == lines.len() - 1 {
                        continue; // the torn tail — skipped, never fabricated
                    }
                    return Err(format!("{}: not a trace record: {e}", path.display()));
                }
            };
            let Some(ts) = rec
                .get("ts")
                .and_then(Value::as_str)
                .and_then(ts_to_epoch_ms)
            else {
                continue;
            };
            if ts >= start_ms && ts <= now_ms {
                out.push(rec);
            }
        }
    }
    Ok((out, read))
}

/// `2026-09-19T07` → epoch ms of that UTC hour.
fn hour_start_ms(stem: &str) -> Option<i64> {
    let (date, hour) = stem.split_once('T')?;
    let mut d = date.split('-');
    let y: i64 = d.next()?.parse().ok()?;
    let mo: u32 = d.next()?.parse().ok()?;
    let day: u32 = d.next()?.parse().ok()?;
    let h: i64 = hour.parse().ok()?;
    Some(vadis_core::peak::utc_midnight_epoch(y, mo, day) as i64 * 1_000 + h * 3_600_000)
}

fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// -- output ------------------------------------------------------------------

fn rfc3339_millis(ms: i64) -> String {
    let s = (ms.div_euclid(1_000)).max(0) as u64;
    let milli = ms.rem_euclid(1_000) as u64;
    let (y, mo, d, minute, _) = vadis_core::peak::timestamp_parts(s, vadis_core::peak::Tz::Utc);
    let (h, mi) = (minute / 60, minute % 60);
    format!(
        "{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{:02}.{milli:03}Z",
        s % 60
    )
}

/// Median (average of the two middle values on an even count).
/// `pub(crate)` so the `/metrics` formatter renders the SAME quantile
/// (ADR-041 §4: one derivation, two readers — never a second copy).
pub(crate) fn median(xs: &mut [f64]) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    xs.sort_by(|a, b| a.partial_cmp(b).expect("continuity values are ordered"));
    let n = xs.len();
    Some(if n % 2 == 1 {
        xs[n / 2]
    } else {
        (xs[n / 2 - 1] + xs[n / 2]) / 2.0
    })
}

/// Nearest-rank p99 (ceil(0.99·n)-th of the sorted values).
/// `pub(crate)` for the same single-owner reason as `median`.
pub(crate) fn p99(xs: &mut [u64]) -> Option<u64> {
    if xs.is_empty() {
        return None;
    }
    xs.sort_unstable();
    let rank = ((xs.len() as f64) * 0.99).ceil() as usize;
    Some(xs[rank.saturating_sub(1).min(xs.len() - 1)])
}

/// §9.2's `hit rate`: `None` when the window holds no input tokens (the
/// absence is a hole, never a 0). Hoisted out of `print_text`/`report_json`
/// so the ratio is divided in ONE place (ADR-041 §4) — the printer, the
/// `--json` builder and the `/metrics` formatter all call this.
pub(crate) fn cache_hit_rate(f: &TraceFigures) -> Option<f64> {
    if f.input_total_tokens == 0 {
        None
    } else {
        Some(f.input_cached_tokens as f64 / f.input_total_tokens as f64)
    }
}

/// §9.2's `stateful inbound rate`: `None` when the window holds no
/// records. Same single-owner hoist as `cache_hit_rate`.
pub(crate) fn stateful_inbound_rate(f: &TraceFigures) -> Option<f64> {
    if f.requests == 0 {
        None
    } else {
        Some(f.stateful_inbound as f64 / f.requests as f64)
    }
}

/// The `--json` `cost` member (spec §9.2's omission rule): with exactly
/// one currency present the scalar keys are kept and a `"currency"`
/// string is added; with several the scalar keys are **absent** and the
/// figures appear under a per-currency map, so a consumer that assumes
/// one total fails loudly instead of summing silently.
fn cost_json(f: &TraceFigures) -> serde_json::Value {
    let cs: Vec<&str> = f.currencies.keys().map(String::as_str).collect();
    if cs.len() == 1 {
        let cur = cs[0];
        serde_json::json!({
            "label": "verified",
            "currency": cur,
            "input_miss_nano": f.input_miss_nano.get(cur).copied().unwrap_or(0),
            "input_hit_nano": f.input_hit_nano.get(cur).copied().unwrap_or(0),
            "cache_write_nano": f.cache_write_nano.get(cur).copied().unwrap_or(0),
            "output_nano": f.output_nano.get(cur).copied().unwrap_or(0),
        })
    } else {
        serde_json::json!({
            "label": "verified",
            "by_currency": {
                "input_miss_nano": &f.input_miss_nano,
                "input_hit_nano": &f.input_hit_nano,
                "cache_write_nano": &f.cache_write_nano,
                "output_nano": &f.output_nano,
            },
        })
    }
}

/// The stdout shape is spec §9.2's frozen example: the same lines, in the
/// same order, with each figure's §7 label inline. Spacing matches the
/// example's columns (label, value, then a three-space note).
#[allow(clippy::format_in_format_args)]
pub(crate) fn print_text(
    rc: &crate::config_load::ResolvedConfig,
    window: &str,
    start_ms: i64,
    now_ms: i64,
    files_read: usize,
    f: &TraceFigures,
    e: &EventFigures,
) {
    let out = std::io::stdout();
    let mut w = std::io::BufWriter::new(out.lock());
    use std::io::Write as _;
    let _ = writeln!(
        w,
        "window:      {window} ({} .. {})",
        rfc3339_millis(start_ms),
        rfc3339_millis(now_ms)
    );
    let _ = writeln!(
        w,
        "trace:       {} ({files_read} files, {} records)",
        rc.trace_dir.display(),
        f.requests
    );
    let _ = writeln!(w);
    let _ = writeln!(w, "requests                         {}", f.requests);
    let _ = writeln!(w, "  succeeded                      {}", f.succeeded);
    let kinds = if f.failure_kinds.is_empty() {
        String::new()
    } else {
        let parts: Vec<String> = f
            .failure_kinds
            .iter()
            .map(|(k, n)| format!("{k} {n}"))
            .collect();
        format!("   ({})", parts.join(", "))
    };
    let _ = writeln!(w, "  failed                          {}{kinds}", f.failed);
    let _ = writeln!(
        w,
        "  usage missing                   {}   (excluded from every rate and every sum below — never read as 0)",
        f.usage_missing
    );
    let _ = writeln!(w);
    // §9.2: the header names the currencies the window holds; money is
    // reported per currency and never summed across them (§4.8).
    let currencies: Vec<&str> = f.currencies.keys().map(String::as_str).collect();
    let _ = writeln!(
        w,
        "currencies:  {}{}",
        currencies.join(", "),
        if currencies.len() > 1 {
            "             (money is reported per currency and never summed across them)"
        } else {
            "                  (money is reported per currency and never summed across them)"
        }
    );
    // One labelled money block per currency present — counts above stay
    // single because they are not money (spec §9.2).
    for cur in &currencies {
        let cur: &str = cur;
        let miss = f.input_miss_nano.get(cur).copied().unwrap_or(0);
        let hit = f.input_hit_nano.get(cur).copied().unwrap_or(0);
        let write = f.cache_write_nano.get(cur).copied().unwrap_or(0);
        let out = f.output_nano.get(cur).copied().unwrap_or(0);
        let _ = writeln!(
            w,
            "cost (verified, {cur})       {} nano",
            miss + hit + write + out
        );
        let _ = writeln!(
            w,
            "  input_miss {miss} | input_hit {hit} | cache_write {write} | output {out}"
        );
    }
    let _ = writeln!(w);
    let _ = writeln!(w, "cache");
    // The hoisted single-owner ratio (ADR-041 §4); the printer's own
    // rendering of the hole is the `0.0000` it has always printed.
    let rate = cache_hit_rate(f)
        .map(|r| format!("{r:.4}"))
        .unwrap_or_else(|| String::from("0.0000"));
    let _ = writeln!(
        w,
        "  hit rate (verified)        {rate}   ({} / {} input tokens)",
        f.input_cached_tokens, f.input_total_tokens
    );
    let cont = median(&mut f.continuity.clone());
    let cont_s = cont
        .map(|c| format!("{c:.3}"))
        .unwrap_or_else(|| "—".into());
    let _ = writeln!(
        w,
        "  continuity p50 (inferred)   {cont_s}   (a predictor, not a measurement of what the provider did)"
    );
    let _ = writeln!(w);
    let _ = writeln!(w, "transforms");
    let _ = writeln!(
        w,
        "  verified savings tokens         {}",
        f.verified_savings_tokens
    );
    let _ = writeln!(
        w,
        "  inferred savings tokens         {}   (labeled; never counted with the line above)",
        f.inferred_savings_tokens
    );
    let _ = writeln!(w);
    // A family nobody configured is not reported, never fabricated.
    if let Some(policy) = &rc.vadis.plan_policy {
        let _ = writeln!(w, "plan family '{}'", policy.family);
        let _ = writeln!(
            w,
            "  switches                        {}   (requests whose result.plan_switch is present)",
            f.switches
        );
        for cur in f.currencies.keys() {
            let _ = writeln!(
                w,
                "  switch cost (verified, {cur})     {} nano",
                f.switch_cost_verified_nano.get(cur).copied().unwrap_or(0)
            );
        }
        let _ = writeln!(
            w,
            "  switch re-prefill (inferred)  {} tokens",
            f.reprefill_tokens
        );
        for cur in f.currencies.keys() {
            let _ = writeln!(
                w,
                "    re-prefill cost (inferred, {cur})  {} nano",
                f.reprefill_cost_nano.get(cur).copied().unwrap_or(0)
            );
        }
        let _ = writeln!(
            w,
            "  switches without usage          {}   (they keep the inferred label and say so)",
            f.switches_without_usage
        );
        let _ = writeln!(w);
    }
    let _ = writeln!(w, "state");
    let stateful = stateful_inbound_rate(f).unwrap_or(0.0);
    let _ = writeln!(
        w,
        "  stateful inbound rate       {stateful:.4}   (always 0 in v0.1 — gap G-F; printed so the constant is visible)"
    );
    // The only omittable figure (the read-only store open above).
    if let Some(unknown) = e_unknown(e) {
        let _ = writeln!(
            w,
            "  unknown outcome requests         {unknown}   (event log)"
        );
    }
    let overhead = p99(&mut f.overhead_ms.clone()).unwrap_or(0);
    let _ = writeln!(w, "overhead p99                     {overhead} ms");
}

fn e_unknown(e: &EventFigures) -> Option<u64> {
    // The figure exists only when the log was read; omission is decided by
    // the caller (which prints the note on stderr). A read log with zero
    // unknowns still prints the line — the ambiguity *is* the number.
    if e.log_was_read {
        Some(e.unknown_outcome_requests)
    } else {
        None
    }
}

/// The `--json` document (spec §9.2), as a value: pure, so conformance
/// (CONF-56) can assert the omission rule's two shapes directly against
/// the same builder the printer uses — the same standing as `report()`
/// for the figures (CONF-41). Exposed for that reason and no other.
pub fn report_json(
    rc: &crate::config_load::ResolvedConfig,
    window: &str,
    rep: &Report,
    unknown_note: &Option<String>,
) -> serde_json::Value {
    let (f, e) = (&rep.figures, &rep.events);
    let (start_ms, now_ms, files_read) = (rep.start_ms, rep.now_ms, rep.files_read);
    let mut v = serde_json::json!({
        "window": {
            "arg": window,
            "from": rfc3339_millis(start_ms),
            "to": rfc3339_millis(now_ms),
        },
        "trace": {
            "dir": rc.trace_dir.display().to_string(),
            "files_read": files_read,
            "records": f.requests,
        },
        "requests": {
            "total": f.requests,
            "succeeded": f.succeeded,
            "failed": f.failed,
            "failure_kinds": f.failure_kinds,
            "usage_missing": f.usage_missing,
        },
        "cost": cost_json(f),
        "cache": {
            "hit_rate": {
                "label": "verified",
                "value": cache_hit_rate(f),
                "input_cached": f.input_cached_tokens,
                "input_total": f.input_total_tokens,
            },
            "continuity_p50": {
                "label": "inferred",
                "value": median(&mut f.continuity.clone()),
            },
        },
        "transforms": {
            "verified_savings_tokens": f.verified_savings_tokens,
            "inferred_savings_tokens": f.inferred_savings_tokens,
        },
        "state": {
            "stateful_inbound_rate": {
                "value": stateful_inbound_rate(f),
                "note": "always 0 in v0.1 (gap G-F)",
            },
        },
        "overhead_ms_p99": p99(&mut f.overhead_ms.clone()),
    });
    if let Some(policy) = &rc.vadis.plan_policy {
        let mut pf = serde_json::json!({
            "family": policy.family,
            "switches": f.switches,
            "reprefill_tokens_inferred": f.reprefill_tokens,
            "switches_without_usage": f.switches_without_usage,
        });
        // §9.2's omission rule, the same one `cost` obeys: with one
        // currency the scalar key stays and gains `"currency"`; with
        // several the scalar is absent and the figures live under a
        // per-currency map — a consumer that assumes one total fails
        // loudly instead of adding silently.
        let cs: Vec<&str> = f.currencies.keys().map(String::as_str).collect();
        if cs.len() == 1 {
            pf["switch_cost_verified_nano"] =
                serde_json::json!(f.switch_cost_verified_nano.get(cs[0]).copied().unwrap_or(0));
            pf["switch_cost_currency"] = serde_json::json!(cs[0]);
            pf["reprefill_cost_nano_inferred"] =
                serde_json::json!(f.reprefill_cost_nano.get(cs[0]).copied().unwrap_or(0));
        } else {
            pf["switch_cost_verified_nano_by_currency"] =
                serde_json::to_value(&f.switch_cost_verified_nano).unwrap_or_default();
            pf["reprefill_cost_nano_inferred_by_currency"] =
                serde_json::to_value(&f.reprefill_cost_nano).unwrap_or_default();
        }
        v["plan_family"] = pf;
    }
    if e.log_was_read {
        v["unknown_outcome_requests"] = serde_json::json!(e.unknown_outcome_requests);
    }
    if let Some(note) = unknown_note {
        v["notes"] = serde_json::json!([note]);
    }
    v
}
