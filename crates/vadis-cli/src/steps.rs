//! `vadis steps` (spec §9.4, ADR-057): the session's step ledger — one
//! row per request of one session, read from the trace files **only**.
//!
//! Read-only by construction, like `stats`, and narrower: no store read
//! (the store is EXCLUSIVE-locked while `serve` runs, which is exactly
//! when this surface is most wanted), no `/state/events`, no `/health`,
//! no upstream call. The trace is append-only and readable while the
//! gateway runs, so there is no live-gateway fallback mode (§9.4.1).
//!
//! The reducer is a **pure function** `[DecisionRecord] → Report`
//! (AGENTS constraint 2): no clock, no RNG, no environment — every time
//! value comes from a record. It orders by `(ts, the order the rows
//! appear in the trace files)`: `ts` is the request's **start** and a
//! row is written when the request ends, so a tie is ordered by
//! completion — stated in the text header, never claimed as resolved.
//!
//! The step classification lives here in the CLI, beside its only
//! consumer, **not** in `vadis-core`: it is a reader's convention over
//! `prefix.blocks[i].hash`, and a core home would invite a
//! `DecisionRecord` field — a schema change with a version question
//! (ADR-057 §6.1).
//!
//! Labels (spec §7): `usage.*`, the block hashes and `lcp` are
//! **verified**; `new_tok` is **inferred** (GAP-Q14); an absent figure
//! prints `—`/null, never `0`. A row served by a provider whose roster
//! entry declares `account: coding_plan` prints the literal `plan` in
//! the money column — allowance headroom is not a zero price (GAP-Q17).

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use serde_json::Value;

use crate::config_load;
use crate::stats::{bucket_of, ts_to_epoch_ms, Bucket};

/// The money cell (spec §9.4.3 column 8): the measured amount with its
/// currency, the config-derived `plan` label, or an absence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Money {
    /// The serving provider's roster entry declares `account:
    /// coding_plan` — the row consumed allowance headroom this product
    /// cannot price (GAP-Q17); it is never added to a currency line.
    Plan,
    Amount {
        total_nano: u64,
        currency: String,
    },
}

/// One step row (spec §9.4.3): the 17 columns, in the spec's order.
/// Field names are the `--json` keys verbatim; `r#in` carries the
/// column literally named `in`.
#[derive(Debug, Clone)]
pub struct StepRow {
    pub step: u32,
    /// `ts` verbatim — the request's start.
    pub start: String,
    /// `ts(n) − ts(n−1) − overhead_ms(n−1)`, as measured (a negative
    /// value prints negative); null for step 1 and when the two `ts`
    /// are identical at the record's resolution.
    pub gap_ms: Option<i64>,
    /// `decision.provider/model`; null when both are empty (a refusal
    /// before any route resolved).
    pub route: Option<String>,
    pub r#in: Option<u64>,
    pub cached: Option<u64>,
    pub out: Option<u64>,
    pub money: Option<Money>,
    /// Signed block delta: `+k` new blocks, `−k` dropped, `0` on a
    /// repeat. **verified** (hash equality).
    pub blocks: i64,
    /// `lcp` — the length of the longest common block-hash prefix with
    /// the previous step. **verified**.
    pub kept: u64,
    /// `#<index>:<kind>` of the first diverging block; null for
    /// `first`, `append` and `repeat`.
    pub at: Option<String>,
    /// Σ `prefix.blocks[i ≥ lcp].tokens` — **inferred** (GAP-Q14).
    pub new_tok: u64,
    pub class: &'static str,
    /// `result.overhead_ms`. On a streamed record this spans the whole
    /// request — it is **not** the gateway's own overhead (§9.4.3).
    pub wall_ms: u32,
    pub head_ms: Option<u32>,
    /// `result.status`, plus `→ upstream_status` when the two differ.
    pub status: String,
    /// The closed §9.4.4 vocabulary, in table order.
    pub flags: Vec<String>,
}

/// The per-class aggregate of the summary's `by class` block.
#[derive(Debug, Default, Clone)]
pub struct ClassAgg {
    pub count: u64,
    pub new_tok: u64,
    /// Measured money per currency (plan rows excluded — counted in
    /// `plan_rows`, never added to a currency line).
    pub money_nano: BTreeMap<String, u64>,
    pub plan_rows: u64,
    /// Rows with `usage_missing: true` — excluded from the money sums.
    pub without_usage: u64,
}

/// The report: the ordered rows plus the §9.4.5 summary. Every summary
/// figure restates rows already computed — this surface defines no
/// metric (§6's definitions are the only ones; the three buckets are
/// ADR-056 §2.4's predicate, reused from `stats::bucket_of`).
#[derive(Debug, Clone)]
pub struct Report {
    pub session: String,
    pub rows: Vec<StepRow>,
    /// Trace files read while collecting the session (the reader's
    /// check that no hour was missed).
    pub files_read: usize,
    pub succeeded: u64,
    pub failed: u64,
    pub stream_incomplete: u64,
    pub usage_missing: u64,
    /// `cost.total` per currency over measured rows; a sum never
    /// crosses a currency (§4.8).
    pub money_nano: BTreeMap<String, u64>,
    pub plan_rows: u64,
    pub by_class: BTreeMap<&'static str, ClassAgg>,
    /// The five largest `new_tok` steps: `(step, class, blocks)`.
    pub top_steps: Vec<(u32, &'static str, i64)>,
    /// `(Σcached, Σin)` over rows that carry usage — a ratio of the
    /// provider's own counters, never a block-derived figure; null
    /// when no row carries usage.
    pub cache: Option<(u64, u64)>,
    /// Rows excluded from the cache ratio (`usage_missing`).
    pub cache_excluded: u64,
}

/// A session's records read from the trace files, plus the two facts
/// the config contributes (the trace directory's location and the
/// roster's `account: coding_plan` provider names for the `plan`
/// label). Records are kept in **file order** (files in name order,
/// lines in append order) — the tie-break of the ordering rule.
pub struct SessionTrace {
    pub records: Vec<Value>,
    pub files_read: usize,
    pub plan_providers: BTreeSet<String>,
    pub trace_dir: String,
}

/// `vadis steps --session <key> --config <file> [--last <n>] [--json]`
/// (spec §9.4.1). Exit codes: `0` a report was printed; `2` a load or
/// parse refusal, or a session with no records. Returns the process
/// exit code.
pub fn steps(config_path: &str, session: &str, last: Option<usize>, json: bool) -> i32 {
    let st = match load_session(config_path, session) {
        Ok(st) => st,
        Err(reason) => {
            eprintln!("vadis: {reason}");
            return 2;
        }
    };
    if st.records.is_empty() {
        eprintln!(
            "vadis: no records for session '{session}' in the trace directory {}",
            st.trace_dir
        );
        return 2;
    }
    let rep = match reduce(&st) {
        Ok(r) => r,
        Err(reason) => {
            eprintln!("vadis: {reason}");
            return 2;
        }
    };
    if json {
        print_json(&rep);
    } else {
        print_text(&rep, last);
    }
    0
}

/// Read one session's records out of the trace files under the config's
/// `trace.dir` — the input medium, and the only one (§9.4.1). Every
/// hourly rollover file is scanned in name order, so a session that
/// crosses an hour boundary is still one session (ADR-057 §2.2).
///
/// The torn tail (ADR-041 §4, `stats::read_window_records`'s rule): a
/// live writer may be mid-line in the final line of the newest file;
/// that one line, when it is not valid JSON, is skipped — never
/// fabricated. Any other malformed line is the refusal it always was.
pub fn load_session(config_path: &str, session: &str) -> Result<SessionTrace, String> {
    let rc = config_load::load(Path::new(config_path))?;
    let plan_providers: BTreeSet<String> = rc
        .vadis
        .providers
        .iter()
        .filter(|p| p.account == vadis_core::config::AccountKind::CodingPlan)
        .map(|p| p.name.clone())
        .collect();
    let trace_dir = rc.trace_dir.clone();
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&trace_dir)
        .map_err(|e| {
            format!(
                "cannot read the trace directory {}: {e}",
                trace_dir.display()
            )
        })?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .collect();
    files.sort();
    let mut records = Vec::new();
    for (fi, path) in files.iter().enumerate() {
        let newest = fi == files.len() - 1;
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
            let in_session = rec
                .get("identity")
                .and_then(|i| i.get("session"))
                .is_some_and(|s| s.as_str() == Some(session));
            if in_session {
                records.push(rec);
            }
        }
    }
    Ok(SessionTrace {
        records,
        files_read: files.len(),
        plan_providers,
        trace_dir: trace_dir.to_string_lossy().into_owned(),
    })
}

/// The pure reducer (spec §9.4.2–§9.4.5): records in file order → the
/// ordered report. Orders by `(ts, file order)` — a stable sort on the
/// parsed `ts`, so equal stamps keep the trace's own (completion)
/// order. No clock, no RNG, no environment.
pub fn reduce(st: &SessionTrace) -> Result<Report, String> {
    // (ts_ms, file-order seq, record, parsed ts) — seq makes the sort
    // stable-by-construction and documents the tie-break.
    let mut keyed: Vec<(i64, usize, &Value)> = Vec::with_capacity(st.records.len());
    for (seq, rec) in st.records.iter().enumerate() {
        let ts = rec
            .get("ts")
            .and_then(Value::as_str)
            .and_then(ts_to_epoch_ms)
            .ok_or_else(|| {
                format!(
                    "record #{} carries no parsable ts ({:?})",
                    seq + 1,
                    rec.get("ts").and_then(Value::as_str).unwrap_or("?")
                )
            })?;
        keyed.push((ts, seq, rec));
    }
    keyed.sort_by_key(|(ts, seq, _)| (*ts, *seq));

    let mut rows: Vec<StepRow> = Vec::with_capacity(keyed.len());
    // The previous step's block-hash list and facts, for the relation.
    let mut prev_hashes: Vec<String> = Vec::new();
    let mut prev_ts_ms: Option<i64> = None;
    let mut prev_overhead_ms: u32 = 0;

    for (n, (ts_ms, _seq, rec)) in keyed.iter().enumerate() {
        // The previous step **in sorted order** — for a shorten's first
        // dropped block, which exists only in the previous list.
        let prev_rec: Option<&Value> = n.checked_sub(1).and_then(|i| keyed.get(i).map(|k| k.2));
        let blocks = rec
            .get("prefix")
            .and_then(|p| p.get("blocks"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let hashes: Vec<String> = blocks
            .iter()
            .filter_map(|b| b.get("hash").and_then(Value::as_str).map(String::from))
            .collect();
        // A block without a hash cannot take part in a verified
        // relation; the trace writer always emits one, so this is a
        // defensive equality on the shorter list, never an invention.
        let lcp = prev_hashes
            .iter()
            .zip(hashes.iter())
            .take_while(|(a, b)| a == b)
            .count();

        let is_first = n == 0;
        let same = !is_first && prev_hashes == hashes;
        let class: &'static str = if is_first {
            "first"
        } else if same {
            "repeat"
        } else if lcp == prev_hashes.len() && hashes.len() > lcp {
            "append"
        } else if lcp == hashes.len() && hashes.len() < prev_hashes.len() {
            "shorten"
        } else {
            // §9.4.2's `rewrite` is `0 < lcp < min(..)`. A divergence
            // whose common prefix is empty (the FIRST block changed)
            // satisfies no listed predicate but is the same relation —
            // "the first differing block is at index `lcp`", with
            // `lcp == 0`; it is classified `rewrite` and printed with
            // its own `at`, so the row's facts never overclaim.
            "rewrite"
        };

        let blocks_delta: i64 = if same {
            0
        } else if class == "shorten" {
            hashes.len() as i64 - prev_hashes.len() as i64
        } else {
            hashes.len() as i64 - lcp as i64
        };

        // `at`: the first diverging block's index and kind — from the
        // current list when it has one at `lcp` (a rewrite), else from
        // the previous list (a shorten's first dropped block).
        let at: Option<String> = match class {
            "rewrite" | "shorten" => {
                let kind = blocks
                    .get(lcp)
                    .or_else(|| {
                        prev_rec
                            .and_then(|p| p.get("prefix"))
                            .and_then(|p| p.get("blocks"))
                            .and_then(Value::as_array)
                            .and_then(|bs| bs.get(lcp))
                    })
                    .and_then(|b| b.get("kind"))
                    .and_then(Value::as_str)
                    .map(String::from);
                kind.map(|k| format!("#{lcp}:{k}"))
            }
            _ => None,
        };

        let new_tok: u64 = blocks
            .iter()
            .skip(lcp)
            .filter_map(|b| b.get("tokens").and_then(Value::as_u64))
            .sum();

        // -- measured facts ------------------------------------------------
        let usage_missing = rec.get("usage_missing").and_then(Value::as_bool) == Some(true);
        let usage = rec.get("usage");
        let (vin, vcached, vout) = if usage_missing {
            (None, None, None)
        } else {
            let g = |k: &str| usage.and_then(|u| u.get(k)).and_then(Value::as_u64);
            (g("input_total"), g("input_cached"), g("output"))
        };
        let provider = rec
            .get("decision")
            .and_then(|d| d.get("provider"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let model = rec
            .get("decision")
            .and_then(|d| d.get("model"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let route = if provider.is_empty() && model.is_empty() {
            None
        } else {
            Some(format!("{provider}/{model}"))
        };
        let money = if usage_missing {
            None
        } else if st.plan_providers.contains(&provider) {
            Some(Money::Plan)
        } else {
            let cost = rec.get("cost");
            let total_nano = cost
                .and_then(|c| c.get("total"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let currency = cost
                .and_then(|c| c.get("currency"))
                .and_then(Value::as_str)
                .unwrap_or("USD") // a v1 record is USD by definition (§6)
                .to_string();
            Some(Money::Amount {
                total_nano,
                currency,
            })
        };

        let result = rec.get("result");
        let rstatus = result
            .and_then(|r| r.get("status"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as u16;
        let upstream_status = result
            .and_then(|r| r.get("upstream_status"))
            .and_then(Value::as_u64)
            .map(|s| s as u16);
        let status = match upstream_status {
            Some(us) if us != rstatus => format!("{rstatus}→{us}"),
            _ => rstatus.to_string(),
        };
        let wall_ms = result
            .and_then(|r| r.get("overhead_ms"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;
        let head_ms = result
            .and_then(|r| r.get("upstream_ms"))
            .and_then(Value::as_u64)
            .map(|v| v as u32);

        // gap: null for step 1 and when the two ts are identical at
        // the record's resolution; printed as measured otherwise —
        // including a negative value, never clamped (§9.4.3).
        let gap_ms = match (n, prev_ts_ms) {
            (0, _) | (_, None) => None,
            (_, Some(prev)) if prev == *ts_ms => None,
            (_, Some(prev)) => Some(ts_ms - prev - prev_overhead_ms as i64),
        };

        rows.push(StepRow {
            step: (n + 1) as u32,
            start: rec
                .get("ts")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            gap_ms,
            route,
            r#in: vin,
            cached: vcached,
            out: vout,
            money,
            blocks: blocks_delta,
            kept: lcp as u64,
            at,
            new_tok,
            class,
            wall_ms,
            head_ms,
            status,
            flags: flags_of(rec, &provider),
        });

        prev_hashes = hashes;
        prev_ts_ms = Some(*ts_ms);
        prev_overhead_ms = wall_ms;
    }

    // -- the summary (§9.4.5): a restatement of the rows above --------
    let mut rep = Report {
        session: st
            .records
            .first()
            .and_then(|r| r.get("identity"))
            .and_then(|i| i.get("session"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        succeeded: 0,
        failed: 0,
        stream_incomplete: 0,
        usage_missing: 0,
        money_nano: BTreeMap::new(),
        plan_rows: 0,
        by_class: BTreeMap::new(),
        top_steps: Vec::new(),
        cache: None,
        cache_excluded: 0,
        rows: Vec::new(),
        files_read: st.files_read,
    };
    let mut cached_sum = 0u64;
    let mut in_sum = 0u64;
    for (row, (_ts, _seq, rec)) in rows.iter().zip(keyed.iter()) {
        match bucket_of(rec) {
            Bucket::Succeeded => rep.succeeded += 1,
            Bucket::StreamIncomplete => rep.stream_incomplete += 1,
            Bucket::Failed => rep.failed += 1,
        }
        let agg = rep.by_class.entry(row.class).or_default();
        agg.count += 1;
        agg.new_tok += row.new_tok;
        match &row.money {
            Some(Money::Plan) => {
                agg.plan_rows += 1;
                rep.plan_rows += 1;
            }
            Some(Money::Amount {
                total_nano,
                currency,
            }) => {
                *rep.money_nano.entry(currency.clone()).or_insert(0) += total_nano;
                *agg.money_nano.entry(currency.clone()).or_insert(0) += total_nano;
            }
            None => {
                agg.without_usage += 1;
                rep.usage_missing += 1;
            }
        }
        match (row.cached, row.r#in) {
            (Some(c), Some(i)) => {
                cached_sum += c;
                in_sum += i;
            }
            _ => rep.cache_excluded += 1,
        }
    }
    if in_sum > 0 || cached_sum > 0 {
        rep.cache = Some((cached_sum, in_sum));
    }
    // The five largest new_tok steps, largest first; ties keep step
    // order (a stable sort over the row index).
    let mut idx: Vec<usize> = (0..rows.len()).collect();
    idx.sort_by(|&a, &b| rows[b].new_tok.cmp(&rows[a].new_tok).then(a.cmp(&b)));
    rep.top_steps = idx
        .into_iter()
        .take(5)
        .map(|i| (rows[i].step, rows[i].class, rows[i].blocks))
        .collect();
    rep.rows = rows;
    Ok(rep)
}

/// The closed flag vocabulary (§9.4.4), in table order. Each flag is
/// one predicate over one field, never an interpretation; `retry` is
/// deliberately absent (it would duplicate `class == repeat`).
fn flags_of(rec: &Value, provider: &str) -> Vec<String> {
    let mut flags = Vec::new();
    let trunc = rec
        .get("errors")
        .and_then(Value::as_array)
        .is_some_and(|errs| {
            errs.iter().any(|e| {
                e.get("details")
                    .and_then(|d| d.get("error_class"))
                    .and_then(Value::as_str)
                    == Some("stream_truncated")
            })
        });
    if trunc {
        flags.push("trunc".into());
    }
    if rec.get("usage_missing").and_then(Value::as_bool) == Some(true) {
        flags.push("no_usage".into());
    }
    if rec.get("cache").is_some_and(|c| !c.is_null()) {
        flags.push("cachereplay".into());
    }
    if rec
        .get("transforms")
        .and_then(Value::as_array)
        .is_some_and(|t| !t.is_empty())
    {
        flags.push("xform".into());
    }
    if let Some(from) = rec
        .get("result")
        .and_then(|r| r.get("failover_from"))
        .and_then(Value::as_str)
    {
        flags.push(format!("failover={from}"));
    }
    if let Some(sw) = rec
        .get("result")
        .and_then(|r| r.get("plan_switch"))
        .filter(|s| !s.is_null())
    {
        let from = sw.get("from").and_then(Value::as_str).unwrap_or("");
        let to = sw.get("to").and_then(Value::as_str).unwrap_or("");
        flags.push(format!("plan={from}→{to}"));
    }
    if provider.is_empty() {
        flags.push("no_route".into());
    }
    if let Some(k) = rec
        .get("decision")
        .and_then(|d| d.get("key_index"))
        .filter(|k| !k.is_null())
    {
        flags.push(format!("key={}", number_as_u32(k)));
    }
    flags
}

fn number_as_u32(v: &Value) -> u32 {
    v.as_u64().map(|x| x as u32).unwrap_or(0)
}

// -- output -------------------------------------------------------------------

fn money_text(m: &Option<Money>) -> String {
    match m {
        None => "—".into(),
        Some(Money::Plan) => "plan".into(),
        Some(Money::Amount {
            total_nano,
            currency,
        }) => format!("{total_nano}n {currency}"),
    }
}

/// The text report: the header (with the ordering rule and the
/// `wall_ms` caveat stated, §9.4.1/§9.4.3), the 17-column rows, then
/// the §9.4.5 summary.
pub fn print_text(rep: &Report, last: Option<usize>) {
    let mut out = String::new();
    let shown: &[StepRow] = match last {
        Some(n) if n < rep.rows.len() => &rep.rows[rep.rows.len() - n..],
        _ => &rep.rows[..],
    };
    let _ = writeln!(
        out,
        "vadis steps — session {}   (trace files read: {})",
        rep.session, rep.files_read
    );
    let _ = writeln!(
        out,
        "ordered by (ts, trace order): ts is the request's start; a tie orders by completion"
    );
    let _ = writeln!(
        out,
        "wall_ms spans the whole request on a streamed row (not the gateway's own overhead); new_tok is inferred; in/cached/out and the block facts are verified"
    );
    if let Some(n) = last.filter(|n| *n < rep.rows.len()) {
        let _ = writeln!(out, "showing the last {n} of {}", rep.rows.len());
    }
    // The header words are format ARGS here rather than inline write!
    // literals — `clippy::write_literal` (the repo's gate runs `-D
    // warnings`) fires on literal arguments to `writeln!` itself.
    let header = format!(
        "{:<4} {:<24} {:>7} {:<16} {:>6} {:>6} {:>5} {:<13} {:>6} {:>4} {:<12} {:>7} {:<7} {:>7} {:>7} {:<6} {}",
        "step", "start", "gap_ms", "route", "in", "cached", "out", "money", "blocks",
        "kept", "at", "new_tok", "class", "wall_ms", "head_ms", "status", "flags"
    );
    let _ = writeln!(out, "{header}");
    for r in shown {
        let _ = writeln!(
            out,
            "{:<4} {:<24} {:>7} {:<16} {:>6} {:>6} {:>5} {:<13} {:>6} {:>4} {:<12} {:>7} {:<7} {:>7} {:>7} {:<6} {}",
            r.step,
            r.start,
            r.gap_ms.map(|g| g.to_string()).unwrap_or_else(|| "—".into()),
            r.route.clone().unwrap_or_else(|| "—".into()),
            r.r#in.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
            r.cached.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
            r.out.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
            money_text(&r.money),
            format_blocks(r.blocks),
            r.kept,
            r.at.clone().unwrap_or_else(|| "—".into()),
            r.new_tok,
            r.class,
            r.wall_ms,
            r.head_ms.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
            r.status,
            r.flags.join(" "),
        );
    }
    let _ = writeln!(out);
    let span_from = rep
        .rows
        .first()
        .map(|r| r.start.clone())
        .unwrap_or_default();
    let span_to = rep.rows.last().map(|r| r.start.clone()).unwrap_or_default();
    let _ = writeln!(out, "session          {span_from} .. {span_to}");
    let _ = writeln!(out, "steps            {}", rep.rows.len());
    let _ = writeln!(
        out,
        "buckets          succeeded {} | failed {} | stream_incomplete {}   (sums to steps; ADR-056 §2.4's predicate)",
        rep.succeeded, rep.failed, rep.stream_incomplete
    );
    let _ = writeln!(out, "usage missing    {}", rep.usage_missing);
    if rep.money_nano.is_empty() && rep.plan_rows == 0 {
        let _ = writeln!(out, "money            —");
    } else {
        let lines: Vec<String> = rep
            .money_nano
            .iter()
            .map(|(cur, nano)| format!("{nano} nano {cur}"))
            .collect();
        // `lines.join` of an empty vec is already empty — the plan-only
        // session prints no lone separator.
        let mut line = format!("money            {}", lines.join(", "));
        if !lines.is_empty() {
            line.push_str("; ");
        }
        if rep.plan_rows > 0 {
            line.push_str(&format!(
                "plan rows: {} (marginal 0 — allowance headroom, not a zero price)",
                rep.plan_rows
            ));
        }
        let _ = writeln!(out, "{line}");
    }
    for (class, agg) in &rep.by_class {
        let mut money_bits: Vec<String> = agg
            .money_nano
            .iter()
            .map(|(cur, nano)| format!("{nano} nano {cur}"))
            .collect();
        if agg.plan_rows > 0 {
            money_bits.push(format!("plan×{}", agg.plan_rows));
        }
        if agg.without_usage > 0 {
            money_bits.push(format!("—×{}", agg.without_usage));
        }
        let _ = writeln!(
            out,
            "  {class:<8} count {:<4} new_tok {:<8} money {}",
            agg.count,
            agg.new_tok,
            if money_bits.is_empty() {
                "—".into()
            } else {
                money_bits.join(", ")
            }
        );
    }
    let top: Vec<String> = rep
        .top_steps
        .iter()
        .map(|(s, c, b)| format!("#{s} ({c}, {} blocks)", format_blocks(*b)))
        .collect();
    let _ = writeln!(
        out,
        "top steps        {}   (the five largest new_tok — where the prompt grew)",
        if top.is_empty() {
            "—".into()
        } else {
            top.join(", ")
        }
    );
    match rep.cache {
        None => {
            let _ = writeln!(out, "cache            —   (no row carries usage)");
        }
        Some((cached, intotal)) => {
            let _ = writeln!(
                out,
                "cache            {:.4} ({} / {} input tokens, verified — the provider's own counters; {} rows without usage excluded)",
                cached as f64 / intotal as f64,
                cached,
                intotal,
                rep.cache_excluded
            );
        }
    }
    print!("{out}");
}

fn format_blocks(b: i64) -> String {
    if b >= 0 {
        format!("+{b}")
    } else {
        format!("−{}", -b)
    }
}

/// The `--json` form (§9.4.1): the same fields under the same names,
/// one `rows` array plus the `summary` object — a field never appears
/// in one form and not the other.
pub fn print_json(rep: &Report) {
    let rows: Vec<serde_json::Value> = rep
        .rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "step": r.step,
                "start": r.start,
                "gap_ms": r.gap_ms,
                "route": r.route,
                "in": r.r#in,
                "cached": r.cached,
                "out": r.out,
                "money": match &r.money {
                    None => serde_json::Value::Null,
                    Some(Money::Plan) => serde_json::json!("plan"),
                    Some(Money::Amount { total_nano, currency }) =>
                        serde_json::json!({ "total_nano": total_nano, "currency": currency }),
                },
                "blocks": r.blocks,
                "kept": r.kept,
                "at": r.at,
                "new_tok": r.new_tok,
                "class": r.class,
                "wall_ms": r.wall_ms,
                "head_ms": r.head_ms,
                "status": r.status,
                "flags": r.flags,
            })
        })
        .collect();
    let by_class: serde_json::Value = rep
        .by_class
        .iter()
        .map(|(class, agg)| {
            (
                class.to_string(),
                serde_json::json!({
                    "count": agg.count,
                    "new_tok": agg.new_tok,
                    "money_nano": agg.money_nano,
                    "plan_rows": agg.plan_rows,
                    "without_usage": agg.without_usage,
                }),
            )
        })
        .collect::<serde_json::Map<String, serde_json::Value>>()
        .into();
    let v = serde_json::json!({
        "session": rep.session,
        "rows": rows,
        "summary": {
            "session": rep.session,
            "span": {
                "from": rep.rows.first().map(|r| r.start.clone()),
                "to": rep.rows.last().map(|r| r.start.clone()),
                "files_read": rep.files_read,
            },
            "steps": rep.rows.len(),
            "succeeded": rep.succeeded,
            "failed": rep.failed,
            "stream_incomplete": rep.stream_incomplete,
            "usage_missing": rep.usage_missing,
            "money_nano": rep.money_nano,
            "plan_rows": rep.plan_rows,
            "by_class": by_class,
            "top_steps": rep.top_steps.iter().map(|(s, c, b)| serde_json::json!({
                "step": s, "class": c, "blocks": b,
            })).collect::<Vec<_>>(),
            "cache": match rep.cache {
                None => serde_json::Value::Null,
                Some((cached, intotal)) => serde_json::json!({
                    "cached": cached,
                    "in": intotal,
                    "excluded": rep.cache_excluded,
                }),
            },
        },
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&v).expect("the report is JSON")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(ts: &str, hashes: &[(&str, u64)]) -> Value {
        serde_json::json!({
            "ts": ts,
            "identity": { "request_id": "r", "event_id": 1, "client": "other",
                          "session": "s", "thread_id": null, "turn_index": 1 },
            "decision": { "provider": "p", "model": "m", "requested_model": null,
                          "selection_source": "explicit", "decision_ms": 0, "key_index": null },
            "prefix": { "blocks": hashes
                .iter()
                .map(|(h, t)| serde_json::json!({ "hash": h, "kind": "message", "index": 0, "tokens": t }))
                .collect::<Vec<_>>(), "continuity": null },
            "usage": { "input_total": 100, "input_cached": 20, "cache_write": 0, "output": 5, "reasoning": 0 },
            "usage_missing": false,
            "cost": { "input_miss": 0, "input_hit": 0, "cache_write": 0, "output": 0,
                      "peak_applied_pct": 100, "total": 92, "currency": "USD" },
            "result": { "status": 200, "upstream_status": null, "failover_from": null,
                        "plan_switch": null, "overhead_ms": 2, "upstream_ms": 1 },
            "errors": [],
        })
    }

    fn report(records: Vec<Value>) -> Report {
        let st = SessionTrace {
            records,
            files_read: 1,
            plan_providers: BTreeSet::new(),
            trace_dir: "/tmp".into(),
        };
        reduce(&st).expect("reduces")
    }

    /// The five classes, over the §9.4.2 predicates — including the
    /// same-length divergence (a rewrite, not a hole) and the
    /// first-block change (`lcp == 0`, §9.4.2's boundary, classified
    /// rewrite with its own `at`).
    #[test]
    fn the_five_classes_and_their_relation_facts() {
        let rep = report(vec![
            rec("2026-10-18T05:00:00.100Z", &[("a", 10)]),
            rec("2026-10-18T05:00:01.100Z", &[("a", 10), ("b", 5)]),
            rec("2026-10-18T05:00:02.100Z", &[("a", 10), ("b", 5)]),
            rec("2026-10-18T05:00:03.100Z", &[("a", 10)]),
            rec("2026-10-18T05:00:04.100Z", &[("a2", 9), ("b", 5)]),
            rec("2026-10-18T05:00:05.100Z", &[("a2", 9), ("x", 4), ("b", 5)]),
        ]);
        let classes: Vec<&str> = rep.rows.iter().map(|r| r.class).collect();
        assert_eq!(
            classes,
            ["first", "append", "repeat", "shorten", "rewrite", "rewrite"]
        );
        let shorten = &rep.rows[3];
        assert_eq!(
            (shorten.kept, shorten.blocks, shorten.at.as_deref()),
            (1, -1, Some("#1:message"))
        );
        // Row 5: the FIRST block changed — lcp == 0, §9.4.2's boundary.
        let zero_lcp = &rep.rows[4];
        assert_eq!(
            (zero_lcp.kept, zero_lcp.blocks, zero_lcp.at.as_deref()),
            (0, 2, Some("#0:message"))
        );
        // Row 6: a mid-prefix rewrite — `0 < lcp < min(..)`, the listed
        // predicate, with its divergence named.
        let rewrite = &rep.rows[5];
        assert_eq!(
            (rewrite.kept, rewrite.blocks, rewrite.at.as_deref()),
            (1, 2, Some("#1:message"))
        );
        assert_eq!(rep.rows[2].blocks, 0);
        assert_eq!(rep.rows[2].new_tok, 0);
    }

    /// gap_ms: null on step 1 and on identical ts; as measured —
    /// including negative — otherwise (§9.4.3).
    #[test]
    fn gap_ms_null_rules_and_sign() {
        let rep = report(vec![
            rec("2026-10-18T05:00:00.100Z", &[("a", 1)]),
            rec("2026-10-18T05:00:00.100Z", &[("a", 1), ("b", 1)]),
            rec("2026-10-18T05:00:00.500Z", &[("a", 1), ("b", 1), ("c", 1)]),
        ]);
        assert_eq!(rep.rows[0].gap_ms, None, "step 1");
        assert_eq!(
            rep.rows[1].gap_ms, None,
            "identical ts — unresolvable, never 0"
        );
        assert_eq!(
            rep.rows[2].gap_ms,
            Some(400 - 2),
            "500ms−100ms start delta minus the previous row's overhead"
        );
    }

    /// A usage-missing row prints absences, never zeros, and carries
    /// `no_usage` (§9.4.3's absence rule).
    #[test]
    fn absence_is_absence() {
        let mut no_usage = rec("2026-10-18T05:00:00.100Z", &[("a", 7)]);
        no_usage["usage_missing"] = serde_json::json!(true);
        let rep = report(vec![no_usage]);
        let r = &rep.rows[0];
        assert_eq!(r.r#in, None);
        assert_eq!(r.cached, None);
        assert_eq!(r.out, None);
        assert_eq!(r.money, None);
        assert!(r.flags.contains(&"no_usage".to_string()));
        assert_eq!(rep.usage_missing, 1);
        assert_eq!(rep.cache, None, "no ratio over unmeasured rows");
    }
}
