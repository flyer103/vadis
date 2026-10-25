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
//! **measured**; `new_tok`, `kept_tok` and `rebill_tok` are
//! **inferred** (GAP-Q14 / §9.4.8); an absent figure
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
    /// repeat. **measured** (hash equality).
    pub blocks: i64,
    /// `lcp` — the length of the longest common block-hash prefix with
    /// the previous step. **measured**.
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
    /// `result.status`. The JSON form emits the **integer** when
    /// `upstream_status` is absent or equal, the `"<status>→<upstream>"`
    /// string otherwise (§9.4.3 column 16); the text form always
    /// prints the composed cell (`status_text`).
    pub status: u16,
    /// `result.upstream_status`, when the record carries one — the
    /// arrow form of column 16 exists only when it differs from
    /// `status`.
    pub upstream_status: Option<u16>,
    /// Σ `prefix.blocks[i < lcp].tokens` — the inferred size of the
    /// prefix the previous step also carried, i.e. the part of the
    /// prompt **the break did not touch** (§9.4.3 column 18).
    /// **inferred** (GAP-Q14, like `new_tok`). Never null: a row whose
    /// `lcp == 0` prints a genuine empty sum, the same measured zero
    /// `kept 0` prints beside it — never the `—` absence.
    pub kept_tok: u64,
    /// §9.4.8's step term — the tokens this step billed **above its
    /// own growth**: `max(0, miss − growth)` over the usage-carrying
    /// rows. **inferred**. Null exactly when `in`/`cached` are (the
    /// row carries no usage; §9.4.3's absence rule).
    pub rebill_tok: Option<u64>,
    /// The closed §9.4.4 vocabulary, in table order.
    pub flags: Vec<String>,
}

impl StepRow {
    /// The text form's status cell (§9.4.3 column 16): the integer, or
    /// the arrow composition when the upstream's status differs.
    pub fn status_text(&self) -> String {
        match self.upstream_status {
            Some(us) if us != self.status => format!("{}→{us}", self.status),
            _ => self.status.to_string(),
        }
    }
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

/// §9.4.8's summary block: the session's **re-billed tokens** — the
/// measured cache-miss volume above the session's one-pass floor —
/// with its printed operands (`Σmiss`, `floor`, `surplus`), its
/// per-currency money and its two groupings. `tokens` is the one
/// member allowed to be null: when no row carries usage, §9.4.8.6's
/// absence rule applies (the object itself is still present, in both
/// forms, with `excluded_rows == steps`).
#[derive(Debug, Default, Clone)]
pub struct Rebill {
    /// §9.4.8.2's figure — **inferred**. `None` only when no row
    /// carries usage.
    pub tokens: Option<u64>,
    /// One amount per currency (§9.4.8.3) — **inferred**, priced at
    /// each row's own measured miss rate; `{}` when no currency
    /// receives one. Never crosses a currency (§4.8).
    pub money_nano: BTreeMap<String, u64>,
    /// Re-billed tokens on a `coding_plan` account — counted, never a
    /// zero in a currency line (GAP-Q17). **inferred**.
    pub plan_tokens: u64,
    /// How many rows carry `rebill_tok > 0`.
    pub rows: u64,
    /// `Σ(in − cached)` over the usage-carrying rows — the identity's
    /// first operand. measured.
    pub miss_tokens: u64,
    /// `max(in)` over the same rows — the session's largest prompt,
    /// the one-pass floor (§9.4.8.2 consequence 3). measured.
    pub floor_tokens: u64,
    /// `Σ surplus(n)` — the clamp's correction (§9.4.8.2). **inferred**.
    pub surplus_tokens: u64,
    /// Rows without usage — excluded from every figure, counted.
    pub excluded_rows: u64,
    /// Rows with re-billed tokens no currency prices (§9.4.8.3).
    pub unpriced_rows: u64,
    /// §9.4.5's `by break kind`: for each of the four block kinds,
    /// always all four (`BREAK_KINDS`'s order), the count of `rewrite`
    /// rows whose `at` names that kind and the sum of their
    /// `rebill_tok` (a no-usage row sums 0). A `rewrite` whose block
    /// carries no kind has `at: null` and lands in no bucket — an
    /// absence, never a guess.
    pub by_break_kind: BTreeMap<&'static str, (u64, u64)>,
    /// §9.4.5's `top rebilled`: the five largest `rebill_tok` rows,
    /// largest first, ties by step order — `(step, class, at,
    /// rebill_tok)`; never padded with zero rows.
    pub top_rebilled: Vec<(u32, &'static str, Option<String>, u64)>,
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
    /// §9.4.8's re-bill block — always present, in both forms.
    pub rebill: Rebill,
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
        print_json(&rep, last);
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
    // §9.4.8.2's running maximum of `usage.input_total` over the
    // preceding usage-carrying rows — `peak(0) = 0`. Pure state of the
    // fold, like `prev_hashes` beside it.
    let mut peak_prev: u64 = 0;

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
        // A block without a hash cannot take part in a measured
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

        // §9.4.3 column 18 — the kept prefix's token weight, the
        // complement of `new_tok` over the same block list.
        let kept_tok: u64 = blocks
            .iter()
            .take(lcp)
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
        // §9.4.3 column 19 / §9.4.8.2's step term, over the
        // usage-carrying rows only (both counters present — the same
        // predicate the `cache` ratio uses). The arithmetic itself is
        // the §9.4.8.2 sentence: `max(0, miss − growth)`, with
        // `growth = max(0, in − peak(n−1))` — the session's own cold
        // start falls out at `rebill(1) = −cached(1) → 0`, a
        // consequence, never a special case.
        let rebill_tok: Option<u64> = match (vin, vcached) {
            (Some(in_tot), Some(cached)) => {
                let miss = in_tot.saturating_sub(cached);
                let growth = in_tot.saturating_sub(peak_prev);
                Some(miss.saturating_sub(growth))
            }
            _ => None,
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
            status: rstatus,
            upstream_status,
            kept_tok,
            rebill_tok,
            flags: flags_of(rec, &provider),
        });

        prev_hashes = hashes;
        prev_ts_ms = Some(*ts_ms);
        prev_overhead_ms = wall_ms;
        // `peak(n)` admits only usage-carrying rows (§9.4.8.2).
        if let (Some(in_tot), Some(_)) = (vin, vcached) {
            peak_prev = peak_prev.max(in_tot);
        }
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
        rebill: Rebill::default(),
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

    // -- the re-bill block (§9.4.8): the same rows, one more fold ----
    // A second running `peak` over the same predicate — fold state
    // again, so the summary restates the rows without `StepRow`
    // carrying the intermediate terms.
    let mut reb = Rebill::default();
    for k in BREAK_KINDS {
        reb.by_break_kind.insert(k, (0, 0));
    }
    let mut peak: u64 = 0;
    for (row, (_ts, _seq, rec)) in rows.iter().zip(keyed.iter()) {
        // `by break kind` (§9.4.5): rewrite rows only, keyed by the
        // kind `at` names — an `at: null` rewrite (no resolvable kind)
        // lands in no bucket, an absence, never a guess.
        if row.class == "rewrite" {
            if let Some(kind) = row.at.as_deref().and_then(|a| a.split(':').nth(1)) {
                if let Some((count, sum)) = reb.by_break_kind.get_mut(kind) {
                    *count += 1;
                    *sum += row.rebill_tok.unwrap_or(0);
                }
            }
        }
        match (row.r#in, row.cached) {
            (Some(in_tot), Some(cached)) => {
                let miss = in_tot.saturating_sub(cached);
                let growth = in_tot.saturating_sub(peak);
                let rebill = miss.saturating_sub(growth);
                let surplus = growth.saturating_sub(miss);
                reb.miss_tokens += miss;
                reb.floor_tokens = reb.floor_tokens.max(in_tot);
                reb.surplus_tokens += surplus;
                reb.tokens = Some(reb.tokens.unwrap_or(0) + rebill);
                if rebill > 0 {
                    reb.rows += 1;
                }
                match &row.money {
                    // A plan row is counted, never priced (§9.4.8.3,
                    // GAP-Q17): its tokens land in `plan_tokens` and
                    // no currency line receives a zero.
                    Some(Money::Plan) => reb.plan_tokens += rebill,
                    Some(Money::Amount { currency, .. }) => {
                        let input_miss = rec
                            .get("cost")
                            .and_then(|c| c.get("input_miss"))
                            .and_then(Value::as_u64)
                            .unwrap_or(0);
                        if rebill > 0 && miss > 0 {
                            if input_miss == 0 {
                                // Re-billed tokens no currency prices —
                                // counted and labelled, never
                                // zero-filled (§9.4.8.3).
                                reb.unpriced_rows += 1;
                            } else {
                                // Integer NanoUsd, floored per row, at
                                // the row's own measured miss rate —
                                // `p_miss` as the cost engine applied
                                // it; no price table is read here.
                                let nano =
                                    ((rebill as u128 * input_miss as u128) / miss as u128) as u64;
                                *reb.money_nano.entry(currency.clone()).or_insert(0) += nano;
                            }
                        }
                    }
                    // A usage-carrying row is never `usage_missing`, so
                    // its money is always present; the arm exists for
                    // the match, not a case.
                    None => unreachable!("a usage-carrying row always carries money"),
                }
                peak = peak.max(in_tot);
            }
            _ => reb.excluded_rows += 1,
        }
    }
    // `top rebilled` (§9.4.5): the five largest `rebill_tok`, largest
    // first, ties by step order — never padded with zero rows.
    let mut ridx: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.rebill_tok.is_some_and(|t| t > 0))
        .map(|(i, _)| i)
        .collect();
    ridx.sort_by(|&a, &b| {
        rows[b]
            .rebill_tok
            .unwrap()
            .cmp(&rows[a].rebill_tok.unwrap())
            .then(a.cmp(&b))
    });
    reb.top_rebilled = ridx
        .into_iter()
        .take(5)
        .map(|i| {
            (
                rows[i].step,
                rows[i].class,
                rows[i].at.clone(),
                rows[i].rebill_tok.unwrap(),
            )
        })
        .collect();
    rep.rebill = reb;
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

/// §9.4.2's five words, in the table's order — `by class` prints all
/// five (§9.4.5): a class that never occurs prints `count 0`, a count,
/// never a zero the reader infers from an absence.
pub const CLASSES: [&str; 5] = ["first", "append", "repeat", "shorten", "rewrite"];

/// The four `prefix::BlockKind` words, in §9.4.5's print order —
/// `by break kind` prints all four (a kind that never breaks a prefix
/// prints `0/0`), never the subset the rows happened to carry.
pub const BREAK_KINDS: [&str; 4] = ["system", "tool", "message", "input_item"];

/// The rows `--last <n>` selects (§9.4.1): the SAME tail slice in both
/// output forms — one rule, so `--json` cannot silently diverge from
/// the text form. `n` at or above the row count shows them all.
fn shown_rows(rows: &[StepRow], last: Option<usize>) -> &[StepRow] {
    match last {
        Some(n) if n < rows.len() => &rows[rows.len() - n..],
        _ => rows,
    }
}

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
/// the §9.4.5 summary. `print_text`'s whole body over a String — the
/// seam the unit tests and the CONF cases drive (a rendering rule is
/// tested at the renderer, not through a process's stdout).
pub fn render_text(rep: &Report, last: Option<usize>) -> String {
    let mut out = String::new();
    let shown: &[StepRow] = shown_rows(&rep.rows, last);
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
        "wall_ms spans the whole request on a streamed row (not the gateway's own overhead); new_tok is inferred; in/cached/out and the block facts are measured; kept_tok and rebill_tok are inferred (a proportional attribution, and a miss volume above the session's one-pass floor); class names the measured block relation (kept/blocks/at), not what the client did"
    );
    if let Some(n) = last.filter(|n| *n < rep.rows.len()) {
        let _ = writeln!(out, "showing the last {n} of {}", rep.rows.len());
    }
    // The header words are format ARGS here rather than inline write!
    // literals — `clippy::write_literal` (the repo's gate runs `-D
    // warnings`) fires on literal arguments to `writeln!` itself.
    // §9.4.3's print order: columns 1–16, then 18 (`kept_tok`) and 19
    // (`rebill_tok`), then 17 `flags` — the variable-length cell stays
    // last (a right-aligned number after a ragged tail is unreadable).
    let header = format!(
        "{:<4} {:<24} {:>7} {:<16} {:>6} {:>6} {:>5} {:<13} {:>6} {:>4} {:<12} {:>7} {:<7} {:>7} {:>7} {:<6} {:>9} {:>10} {}",
        "step", "start", "gap_ms", "route", "in", "cached", "out", "money", "blocks",
        "kept", "at", "new_tok", "class", "wall_ms", "head_ms", "status", "kept_tok",
        "rebill_tok", "flags"
    );
    let _ = writeln!(out, "{header}");
    for r in shown {
        let _ = writeln!(
            out,
            "{:<4} {:<24} {:>7} {:<16} {:>6} {:>6} {:>5} {:<13} {:>6} {:>4} {:<12} {:>7} {:<7} {:>7} {:>7} {:<6} {:>9} {:>10} {}",
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
            r.status_text(),
            r.kept_tok,
            r.rebill_tok.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
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
    // §9.4.5: the FIVE counts, in §9.4.2's table order — an absent
    // class prints `count 0` (a count, never a zero the reader infers
    // from an absence; the same five keys the JSON form carries).
    for class in CLASSES {
        let agg = rep.by_class.get(class);
        let count = agg.map(|a| a.count).unwrap_or(0);
        let new_tok = agg.map(|a| a.new_tok).unwrap_or(0);
        let mut money_bits: Vec<String> = agg
            .map(|a| {
                a.money_nano
                    .iter()
                    .map(|(cur, nano)| format!("{nano} nano {cur}"))
                    .collect()
            })
            .unwrap_or_default();
        if agg.is_some_and(|a| a.plan_rows > 0) {
            money_bits.push(format!("plan×{}", agg.map(|a| a.plan_rows).unwrap_or(0)));
        }
        if agg.is_some_and(|a| a.without_usage > 0) {
            money_bits.push(format!("—×{}", agg.map(|a| a.without_usage).unwrap_or(0)));
        }
        let _ = writeln!(
            out,
            "  {class:<8} count {count:<4} new_tok {new_tok:<8} money {}",
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
                "cache            {:.4} ({} / {} input tokens, measured — the provider's own counters; {} rows without usage excluded)",
                cached as f64 / intotal as f64,
                cached,
                intotal,
                rep.cache_excluded
            );
        }
    }
    // -- the re-bill block (§9.4.8.5): three lines, then the two ------
    // groupings. Every one of the nine keys is printed here — the
    // counts clause carries rows/excluded_rows/unpriced_rows, the
    // first line carries tokens/miss_tokens/floor_tokens/
    // surplus_tokens, the money line carries money_nano.
    let rb = &rep.rebill;
    match rb.tokens {
        None => {
            let _ = writeln!(out, "re-billed        — (no step carries usage)");
        }
        Some(tokens) => {
            let _ = writeln!(
                out,
                "re-billed        {} tokens (inferred)  = Σ miss {} − floor {} + surplus {}   ({} rows re-billed; {} rows without usage; {} rows unpriced)",
                tokens, rb.miss_tokens, rb.floor_tokens, rb.surplus_tokens, rb.rows,
                rb.excluded_rows, rb.unpriced_rows
            );
        }
    }
    let money_bits: Vec<String> = rb
        .money_nano
        .iter()
        .map(|(cur, nano)| format!("{nano} nano {cur}"))
        .collect();
    let _ = writeln!(
        out,
        "re-billed money  {} (inferred; each row's own measured miss rate — no price table is read here)",
        if money_bits.is_empty() {
            "—".to_string()
        } else {
            money_bits.join(", ")
        }
    );
    // The one cell allowed to be absent: printed only when non-zero
    // (the `money` line's own `plan rows` rule), and its absence means
    // zero.
    if rb.plan_tokens > 0 {
        let _ = writeln!(
            out,
            "re-billed plan   {} tokens (marginal 0 — allowance headroom, not a zero price)",
            rb.plan_tokens
        );
    }
    // `by break kind`: all four words, always, `count/Σ rebill_tok`,
    // over rewrite rows only (§9.4.5).
    let kinds: Vec<String> = BREAK_KINDS
        .iter()
        .map(|k| {
            let (count, sum) = rb.by_break_kind.get(*k).copied().unwrap_or((0, 0));
            format!("{k} {count}/{sum}")
        })
        .collect();
    let _ = writeln!(
        out,
        "by break kind    {}   (rewrite rows only; count/Σ rebill_tok)",
        kinds.join(" | ")
    );
    let top: Vec<String> = rb
        .top_rebilled
        .iter()
        .map(|(s, c, at, tok)| {
            format!(
                "#{s} ({c}, {}, {tok} tok)",
                at.clone().unwrap_or_else(|| "—".into())
            )
        })
        .collect();
    let _ = writeln!(
        out,
        "top rebilled     {}   (the five largest rebill_tok — which step cost the money)",
        if top.is_empty() {
            "—".to_string()
        } else {
            top.join(", ")
        }
    );
    out
}

/// The text report to stdout (§9.4.1): `render_text`'s one caller on
/// the CLI path.
pub fn print_text(rep: &Report, last: Option<usize>) {
    print!("{}", render_text(rep, last));
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
/// in one form and not the other. `report_json` is the builder the
/// CONF cases read, so the contract's typing is pinned where the bytes
/// are made, not through a process's stdout.
pub fn report_json(rep: &Report, last: Option<usize>) -> serde_json::Value {
    // §9.4.1: `--last` selects the SAME tail slice in both forms —
    // `rows` is the slice, `steps` stays the pre-`--last` count
    // (§9.4.5), and the summary states the fact the text header does.
    let shown = shown_rows(&rep.rows, last);
    let rows: Vec<serde_json::Value> = shown
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
                // §9.4.3 column 16: an INTEGER when `upstream_status`
                // is absent or equal, the arrow string only when the
                // two differ — the extension point ADR-057 §6.1 names.
                "status": match r.upstream_status {
                    Some(us) if us != r.status => serde_json::json!(r.status_text()),
                    _ => serde_json::json!(r.status),
                },
                // Columns 18 and 19 (§9.4.3's print order — before the
                // variable-length `flags` cell, as in the text table).
                "kept_tok": r.kept_tok,
                "rebill_tok": r.rebill_tok,
                "flags": r.flags,
            })
        })
        .collect();
    // §9.4.5's five counts, in §9.4.2's table order: an absent class
    // carries count 0 (the same zeros the text form prints — a count,
    // never a zero inferred from an absence).
    let by_class: serde_json::Value = CLASSES
        .iter()
        .map(|class| {
            let agg = rep.by_class.get(class).cloned().unwrap_or_default();
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
    // §9.4.8.5's rebill block — the same nine values the text form
    // prints, plus the two groupings, under the same names. Built
    // before the summary macro: `json!` cannot take a block as an
    // object value.
    let rebill_json = {
        let rb = &rep.rebill;
        serde_json::json!({
            "tokens": rb.tokens,
            "money_nano": rb.money_nano,
            "plan_tokens": rb.plan_tokens,
            "rows": rb.rows,
            "miss_tokens": rb.miss_tokens,
            "floor_tokens": rb.floor_tokens,
            "surplus_tokens": rb.surplus_tokens,
            "excluded_rows": rb.excluded_rows,
            "unpriced_rows": rb.unpriced_rows,
            "by_break_kind": BREAK_KINDS
                .iter()
                .map(|k| {
                    let (count, sum) = rb.by_break_kind.get(*k).copied().unwrap_or((0, 0));
                    (
                        k.to_string(),
                        serde_json::json!({ "rows": count, "rebill_tok": sum }),
                    )
                })
                .collect::<serde_json::Map<String, serde_json::Value>>(),
            "top_rebilled": rb.top_rebilled.iter().map(|(s, c, at, tok)| serde_json::json!({
                "step": s, "class": c, "at": at, "rebill_tok": tok,
            })).collect::<Vec<_>>(),
        })
    };
    let mut summary = serde_json::json!({
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
        // §9.4.8.5's rebill block — the same nine values the text form
        // prints, plus the two groupings, under the same names.
        "rebill": rebill_json,
    });
    if let Some(n) = last.filter(|n| *n < rep.rows.len()) {
        summary["showing"] = serde_json::json!({ "last": n, "of": rep.rows.len() });
    }
    serde_json::json!({
        "session": rep.session,
        "rows": rows,
        "summary": summary,
    })
}

/// The `--json` report to stdout (§9.4.1): `report_json`'s one caller
/// on the CLI path.
pub fn print_json(rep: &Report, last: Option<usize>) {
    println!(
        "{}",
        serde_json::to_string_pretty(&report_json(rep, last)).expect("the report is JSON")
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

    fn report_plan(records: Vec<Value>, plan_providers: &[&str]) -> Report {
        let st = SessionTrace {
            records,
            files_read: 1,
            plan_providers: plan_providers.iter().map(|s| s.to_string()).collect(),
            trace_dir: "/tmp".into(),
        };
        reduce(&st).expect("reduces")
    }

    /// `rec` with the usage counters (and optionally the miss money)
    /// set — the inputs §9.4.8.2 folds over. The provider stays `p`
    /// unless the caller repoints it (the `plan` label is per-provider).
    fn rec_usage(ts: &str, hashes: &[(&str, u64)], input_total: u64, input_cached: u64) -> Value {
        let mut r = rec(ts, hashes);
        r["usage"]["input_total"] = serde_json::json!(input_total);
        r["usage"]["input_cached"] = serde_json::json!(input_cached);
        r
    }

    fn on_provider(mut r: Value, provider: &str) -> Value {
        r["decision"]["provider"] = serde_json::json!(provider);
        r
    }

    /// §9.4.8.2 consequence 1: the session's own cold start cannot be
    /// a re-bill — `rebill(1) = −cached(1) → 0` by construction, on a
    /// session that grew, not a degenerate one.
    #[test]
    fn rebill_first_is_zero_by_construction() {
        let rep = report(vec![
            rec_usage("2026-10-18T05:00:00.100Z", &[("a", 10)], 10_000, 0),
            rec_usage(
                "2026-10-18T05:00:01.100Z",
                &[("a", 10), ("b", 5)],
                15_000,
                10_000,
            ),
        ]);
        assert_eq!(
            rep.rows[0].rebill_tok,
            Some(0),
            "rebill(first) = −cached → 0"
        );
        assert_eq!(
            rep.rows[1].rebill_tok,
            Some(0),
            "a perfect-cache append is all growth"
        );
        assert_eq!(rep.rebill.tokens, Some(0));
        // kept_tok: the complement of new_tok over the same list.
        assert_eq!(rep.rows[1].kept_tok, 10);
        assert_eq!(rep.rows[1].new_tok, 5);
    }

    /// §9.4.8.1's second shape: a same-session cold re-prefill (a
    /// `rewrite` with `lcp == 0`) bills the whole prompt minus its own
    /// growth — the entire `in` when the prompt stayed below the
    /// session's peak, `in − growth` when it grew past it.
    #[test]
    fn cold_reprefill_bills_the_whole_prompt_minus_growth() {
        let rep = report(vec![
            rec_usage("2026-10-18T05:00:00.100Z", &[("a", 10)], 10_000, 0),
            // the previous prompt replaced (first block differs):
            // below the peak ⇒ the whole prompt is a re-bill.
            rec_usage("2026-10-18T05:00:01.100Z", &[("z", 8)], 8_000, 0),
            // and one that grew past the peak ⇒ in − growth.
            rec_usage("2026-10-18T05:00:02.100Z", &[("y", 9)], 20_000, 0),
        ]);
        assert_eq!(rep.rows[1].class, "rewrite");
        assert_eq!(rep.rows[1].kept_tok, 0, "lcp == 0 ⇒ a genuine empty sum");
        assert_eq!(rep.rows[1].rebill_tok, Some(8_000), "the whole prompt");
        assert_eq!(rep.rows[2].rebill_tok, Some(20_000 - 10_000), "in − growth");
    }

    /// §9.4.8.2's identity on a session with a clamped row (a row
    /// whose miss fell short of its growth — the surplus path):
    /// `rebilled = Σmiss − floor + surplus` must hold on the page.
    #[test]
    fn rebill_identity_holds_on_a_clamped_session() {
        let rep = report(vec![
            rec_usage("2026-10-18T05:00:00.100Z", &[("a", 10)], 100, 20),
            // miss 10 < growth 100 ⇒ rebill clamps to 0, surplus 90.
            rec_usage("2026-10-18T05:00:01.100Z", &[("a", 10), ("b", 5)], 200, 190),
        ]);
        let rb = &rep.rebill;
        assert_eq!(rb.tokens, Some(0));
        assert_eq!(rb.miss_tokens, 90);
        assert_eq!(rb.floor_tokens, 200);
        assert_eq!(rb.surplus_tokens, 110);
        assert_eq!(
            rb.tokens.unwrap(),
            rb.miss_tokens + rb.surplus_tokens - rb.floor_tokens,
            "the identity holds with the clamp's correction added back \
             (evaluated add-first: the page's `Σmiss − floor + surplus` \
             can go negative mid-sum, the u64 fold never does)"
        );
    }

    /// §9.4.8.6's first case: a session with no break and no cache
    /// miss reports exactly 0 — a zero, never null, never an absent
    /// key.
    #[test]
    fn a_no_break_session_reports_exactly_zero() {
        let rep = report(vec![
            rec_usage("2026-10-18T05:00:00.100Z", &[("a", 10)], 100, 0),
            rec_usage("2026-10-18T05:00:01.100Z", &[("a", 10), ("b", 5)], 150, 100),
            rec_usage(
                "2026-10-18T05:00:02.100Z",
                &[("a", 10), ("b", 5), ("c", 1)],
                180,
                150,
            ),
        ]);
        let rb = &rep.rebill;
        assert_eq!(rb.tokens, Some(0));
        assert_eq!(rb.rows, 0);
        assert!(rb.money_nano.is_empty());
        assert_eq!(rb.plan_tokens, 0);
        assert_eq!(rb.surplus_tokens, 0);
        assert_eq!(
            rb.miss_tokens, rb.floor_tokens,
            "Σmiss = floor on a one-pass chain"
        );
    }

    /// §9.4.8.2 consequence 3, worked: on `(10k,0)(15k,10k)(3k,0)` the
    /// peak floor reports 3 000 — the replacement text's own miss —
    /// where a final-context floor would book 15 000.
    #[test]
    fn the_floor_is_the_peak_not_the_final_context() {
        let rep = report(vec![
            rec_usage("2026-10-18T05:00:00.100Z", &[("a", 10)], 10_000, 0),
            rec_usage(
                "2026-10-18T05:00:01.100Z",
                &[("a", 10), ("b", 5)],
                15_000,
                10_000,
            ),
            // compaction: shorter, and the cache missed it all.
            rec_usage("2026-10-18T05:00:02.100Z", &[("a", 3)], 3_000, 0),
        ]);
        assert_eq!(rep.rebill.tokens, Some(3_000));
        assert_eq!(rep.rebill.miss_tokens, 18_000);
        assert_eq!(rep.rebill.floor_tokens, 15_000);
        assert_eq!(rep.rebill.surplus_tokens, 0);
    }

    /// §9.4.8.6's absence case: no row carries usage ⇒ `tokens` is
    /// null (the one null), the object still present, every row
    /// counted in `excluded_rows`.
    #[test]
    fn rebill_tokens_null_when_no_step_carries_usage() {
        let mut no_usage = rec("2026-10-18T05:00:00.100Z", &[("a", 7)]);
        no_usage["usage_missing"] = serde_json::json!(true);
        let rep = report(vec![no_usage]);
        assert_eq!(rep.rows[0].rebill_tok, None);
        let rb = &rep.rebill;
        assert_eq!(rb.tokens, None, "§9.4.8.6's one null");
        assert_eq!(rb.rows, 0);
        assert!(rb.money_nano.is_empty());
        assert_eq!(rb.excluded_rows, 1, "excluded_rows == steps");
    }

    /// §9.4.8.3: the money is each row's own measured miss rate —
    /// `floor(rebill_tok × cost.input_miss / miss)` per row — never a
    /// price table read. A plan row counts tokens into `plan_tokens`
    /// and prices nothing; an unpriced row (`input_miss == 0` while
    /// `miss > 0`) contributes tokens and is counted, never
    /// zero-filled.
    #[test]
    fn rebill_money_uses_each_rows_own_measured_miss_rate() {
        let mut metered = rec_usage("2026-10-18T05:00:00.100Z", &[("a", 10)], 10_000, 0);
        metered["cost"]["input_miss"] = serde_json::json!(50_000); // 5 nano/tok
        let mut unpriced = rec_usage("2026-10-18T05:00:01.100Z", &[("z", 8)], 8_000, 0);
        unpriced["cost"]["input_miss"] = serde_json::json!(0);
        unpriced["cost"]["currency"] = serde_json::json!("USD");
        let mut plan = rec_usage("2026-10-18T05:00:02.100Z", &[("y", 9)], 9_000, 0);
        plan["cost"]["input_miss"] = serde_json::json!(90_000);
        // `m` is metered, `p` is the plan provider — only row 3 rides it.
        let rep = report_plan(
            vec![on_provider(metered, "m"), on_provider(unpriced, "m"), plan],
            &["p"],
        );
        // Row 1 is all growth (the session's own cold start ⇒ rebill 0);
        // rows 2-3 sit below the peak ⇒ the whole prompt is the re-bill.
        assert_eq!(rep.rows[0].rebill_tok, Some(0));
        assert_eq!(rep.rows[1].rebill_tok, Some(8_000));
        assert_eq!(rep.rows[2].rebill_tok, Some(9_000));
        assert_eq!(rep.rebill.tokens, Some(17_000));
        assert_eq!(rep.rebill.rows, 2);
        assert_eq!(rep.rebill.unpriced_rows, 1, "tokens, no money, counted");
        assert!(
            rep.rebill.money_nano.is_empty(),
            "no currency received a figure"
        );
        assert_eq!(
            rep.rebill.plan_tokens, 9_000,
            "the plan row counts tokens, prices nothing"
        );
    }

    /// §9.4.8.3's priced arm, exercised: a rewrite below the peak on a
    /// metered row prices at its own miss rate; a plan rewrite counts
    /// into `plan_tokens` and prices into no currency line.
    #[test]
    fn a_priced_rebill_and_a_plan_rebill() {
        let mut first = rec_usage("2026-10-18T05:00:00.100Z", &[("a", 10)], 10_000, 2_000);
        first["cost"]["input_miss"] = serde_json::json!(64_000); // 8 nano/tok over 8 000 miss
        let mut rewrite = rec_usage("2026-10-18T05:00:01.100Z", &[("z", 6)], 6_000, 0);
        rewrite["cost"]["input_miss"] = serde_json::json!(30_000); // 5 nano/tok over 6 000 miss
        rewrite["cost"]["currency"] = serde_json::json!("CNY");
        let mut plan_rewrite = rec_usage("2026-10-18T05:00:02.100Z", &[("y", 4)], 4_000, 0);
        plan_rewrite["cost"]["input_miss"] = serde_json::json!(40_000);
        // `m` is metered (rows 1-2), `p` is the plan provider (row 3).
        let rep = report_plan(
            vec![
                on_provider(first, "m"),
                on_provider(rewrite, "m"),
                plan_rewrite,
            ],
            &["p"],
        );
        assert_eq!(rep.rows[1].rebill_tok, Some(6_000));
        assert_eq!(rep.rows[2].rebill_tok, Some(4_000));
        assert_eq!(rep.rebill.tokens, Some(10_000));
        // 6 000 × 30 000 / 6 000 = 30 000 nano CNY — nothing else.
        assert_eq!(rep.rebill.money_nano.get("CNY"), Some(&30_000));
        assert_eq!(rep.rebill.money_nano.len(), 1);
        assert_eq!(rep.rebill.plan_tokens, 4_000);
        assert_eq!(rep.rebill.unpriced_rows, 0);
        assert_eq!(rep.rebill.rows, 2);
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

    /// §9.4.1's `--last <n>` under `--json`: `rows` is the SAME tail
    /// slice the text form prints, the summary states the fact, and
    /// `steps` stays the pre-`--last` count (§9.4.5). A silent no-op
    /// here was the review's defect 1.
    #[test]
    fn json_last_is_the_same_tail_slice() {
        let rep = report(vec![
            rec("2026-10-18T05:00:00.100Z", &[("a", 10)]),
            rec("2026-10-18T05:00:01.100Z", &[("a", 10), ("b", 5)]),
            rec("2026-10-18T05:00:02.100Z", &[("a", 10), ("b", 5), ("c", 1)]),
        ]);
        let v = report_json(&rep, Some(2));
        let rows = v["rows"].as_array().expect("rows is an array");
        assert_eq!(rows.len(), 2, "the same tail slice the text form prints");
        let steps_shown: Vec<u64> = rows
            .iter()
            .map(|r| r["step"].as_u64().expect("step is an integer"))
            .collect();
        assert_eq!(steps_shown, vec![2, 3]);
        assert_eq!(
            v["summary"]["steps"], 3,
            "steps stays the pre---last count (§9.4.5)"
        );
        assert_eq!(v["summary"]["showing"]["last"], 2);
        assert_eq!(v["summary"]["showing"]["of"], 3);
        // Without --last there is nothing to state: no showing member.
        let full = report_json(&rep, None);
        assert_eq!(full["rows"].as_array().unwrap().len(), 3);
        assert!(
            full["summary"].get("showing").is_none(),
            "showing exists only when --last actually truncated"
        );
    }

    /// §9.4.3 column 16 in the JSON form: `status` is an **integer**
    /// when `upstream_status` is absent or equal, the `"<status>→<upstream>"`
    /// string only when the two differ. An unconditional string was the
    /// review's defect 2.
    #[test]
    fn json_status_is_integer_unless_the_upstream_differs() {
        let mut differed = rec("2026-10-18T05:00:01.100Z", &[("a", 10), ("b", 5)]);
        differed["result"]["upstream_status"] = serde_json::json!(502);
        let rep = report(vec![
            rec("2026-10-18T05:00:00.100Z", &[("a", 10)]),
            differed,
        ]);
        let v = report_json(&rep, None);
        let rows = v["rows"].as_array().unwrap();
        assert!(
            rows[0]["status"].is_u64(),
            "a normal row's status is an integer: {:?}",
            rows[0]["status"]
        );
        assert_eq!(
            rows[1]["status"].as_str(),
            Some("200→502"),
            "the arrow string only when the two differ"
        );
        // The text form keeps the composed cell either way (§9.4.3).
        let text = render_text(&rep, None);
        assert!(text.contains("200→502"));
        assert!(text.lines().any(|l| l.contains(" 200 ")));
    }

    /// §9.4.5's `by class` is **the five counts** (§9.4.2's words, in
    /// table order), a class that never occurs printing `count 0` in
    /// both forms — a count, never a zero inferred from an absence.
    /// Printing only the classes present was the review's defect 4.
    #[test]
    fn by_class_is_the_five_counts_in_both_forms() {
        // first + append only: no repeat, no shorten, no rewrite.
        let rep = report(vec![
            rec("2026-10-18T05:00:00.100Z", &[("a", 10)]),
            rec("2026-10-18T05:00:01.100Z", &[("a", 10), ("b", 5)]),
        ]);
        let text = render_text(&rep, None);
        assert!(
            text.contains("class names the measured block relation"),
            "the header carries ADR-057 §2.2's caveat with ADR-058's word (defect 3 / ADR-062 §5 A4)"
        );
        assert!(
            !text.contains("verified"),
            "§9.4.3: this surface prints no `verified` figure at all (ADR-058)"
        );
        for (word, count) in [
            ("first", "count 1"),
            ("append", "count 1"),
            ("repeat", "count 0"),
            ("shorten", "count 0"),
            ("rewrite", "count 0"),
        ] {
            assert!(
                text.lines()
                    .any(|l| l.trim_start().starts_with(word) && l.contains(count)),
                "the {word} line prints {count}"
            );
        }
        let v = report_json(&rep, None);
        for word in ["first", "append", "repeat", "shorten", "rewrite"] {
            assert!(
                v["summary"]["by_class"].get(word).is_some(),
                "the JSON by_class carries all five words ({word})"
            );
        }
        assert_eq!(v["summary"]["by_class"]["rewrite"]["count"], 0);
        assert_eq!(v["summary"]["by_class"]["first"]["count"], 1);
        assert_eq!(v["summary"]["by_class"]["append"]["count"], 1);
    }

    /// A one-fixture session exercising both forms of the re-bill
    /// surface: an append chain, then a mid-prefix break (kept `a`,
    /// diverged at `#1`), then a same-session cold re-prefill
    /// (`#0`). The `--json` row keys and the text table must carry
    /// the same values under the same names (§9.4.1's one rule).
    #[test]
    fn rebill_in_both_forms_the_same_values() {
        let mut r1 = rec_usage("2026-10-18T05:00:00.100Z", &[("a", 40), ("b", 60)], 100, 0);
        r1["decision"]["provider"] = serde_json::json!("m");
        r1["cost"]["input_miss"] = serde_json::json!(200); // 2 nano/tok
        let mut r2 = rec_usage(
            "2026-10-18T05:00:01.100Z",
            &[("a", 40), ("b", 60), ("c", 20)],
            120,
            100,
        );
        r2["decision"]["provider"] = serde_json::json!("m");
        r2["cost"]["input_miss"] = serde_json::json!(40);
        // a mid-prefix break: kept `a`, the tail diverged at #1
        let mut r3 = rec_usage(
            "2026-10-18T05:00:02.100Z",
            &[("a", 40), ("w", 55), ("c", 20)],
            115,
            40,
        );
        r3["decision"]["provider"] = serde_json::json!("m");
        r3["cost"]["input_miss"] = serde_json::json!(150); // 2 nano/tok
                                                           // a same-session cold re-prefill: nothing survived
        let mut r4 = rec_usage("2026-10-18T05:00:03.100Z", &[("z", 30), ("y", 50)], 80, 0);
        r4["decision"]["provider"] = serde_json::json!("m");
        r4["cost"]["input_miss"] = serde_json::json!(160); // 2 nano/tok
        let rep = report(vec![r1, r2, r3, r4]);
        // (1) 100/0 growth 100 rebill 0; (2) 120/100 miss 20 = growth
        // 20 rebill 0; (3) 115/40 miss 75 growth 0 rebill 75;
        // (4) 80/0 miss 80 growth 0 rebill 80.
        assert_eq!(rep.rows[2].kept_tok, 40);
        assert_eq!(rep.rows[2].rebill_tok, Some(75));
        assert_eq!(rep.rows[3].kept_tok, 0);
        assert_eq!(rep.rows[3].rebill_tok, Some(80));
        let v = report_json(&rep, None);
        let rows = v["rows"].as_array().unwrap();
        assert_eq!(rows[2]["kept_tok"], 40);
        assert_eq!(rows[2]["rebill_tok"], 75);
        assert_eq!(rows[3]["kept_tok"], 0);
        assert_eq!(rows[3]["rebill_tok"], 80);
        // key order: 18 and 19 sit between `status` and `flags`.
        let keys: Vec<&str> = rows[0]
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        let i_status = keys.iter().position(|k| *k == "status").unwrap();
        assert_eq!(
            &keys[i_status + 1..i_status + 4],
            ["kept_tok", "rebill_tok", "flags"]
        );
        // summary.rebill: the nine keys, same values the text prints.
        let rb = &v["summary"]["rebill"];
        assert_eq!(rb["tokens"], 155);
        assert_eq!(rb["rows"], 2);
        assert_eq!(rb["miss_tokens"], 275);
        assert_eq!(rb["floor_tokens"], 120);
        assert_eq!(rb["surplus_tokens"], 0);
        assert_eq!(rb["excluded_rows"], 0);
        assert_eq!(rb["unpriced_rows"], 0);
        assert_eq!(rb["plan_tokens"], 0);
        assert_eq!(rb["money_nano"]["USD"], 150 + 160);
        let text = render_text(&rep, None);
        assert!(text.contains("re-billed        155 tokens (inferred)  = Σ miss 275 − floor 120 + surplus 0   (2 rows re-billed; 0 rows without usage; 0 rows unpriced)"));
        assert!(text.contains("re-billed money  310 nano USD (inferred; each row's own measured miss rate — no price table is read here)"));
        assert!(!text.contains("re-billed plan"), "absence means zero");
        // The groupings, same values in both forms.
        assert_eq!(rb["by_break_kind"]["message"]["rows"], 2);
        assert_eq!(rb["by_break_kind"]["message"]["rebill_tok"], 155);
        assert_eq!(rb["by_break_kind"]["system"]["rows"], 0);
        assert_eq!(rb["top_rebilled"][0]["step"], 4);
        assert_eq!(rb["top_rebilled"][0]["rebill_tok"], 80);
        assert_eq!(rb["top_rebilled"][0]["at"], "#0:message");
        assert_eq!(rb["top_rebilled"][1]["at"], "#1:message");
        assert!(text
            .contains("by break kind    system 0/0 | tool 0/0 | message 2/155 | input_item 0/0"));
        assert!(text.contains(
            "top rebilled     #4 (rewrite, #0:message, 80 tok), #3 (rewrite, #1:message, 75 tok)"
        ));
    }

    /// §9.4.8.6's zero and absence cases on the printed page: a
    /// no-break session prints the arithmetic line with a zero (never
    /// an absent key), a no-usage session prints the one replacement
    /// line — and `rebill_tok` prints `—` on a row without usage.
    #[test]
    fn rebill_zero_and_absence_on_the_page() {
        let rep = report(vec![
            rec_usage("2026-10-18T05:00:00.100Z", &[("a", 10)], 100, 0),
            rec_usage("2026-10-18T05:00:01.100Z", &[("a", 10), ("b", 5)], 150, 100),
        ]);
        let text = render_text(&rep, None);
        assert!(text.contains("re-billed        0 tokens (inferred)"));
        assert!(text.contains("(0 rows re-billed; 0 rows without usage; 0 rows unpriced)"));
        assert!(text.contains("re-billed money  —"));
        let v = report_json(&rep, None);
        assert_eq!(v["summary"]["rebill"]["tokens"], 0);
        assert_eq!(
            v["summary"]["rebill"]["money_nano"]
                .as_object()
                .unwrap()
                .len(),
            0
        );

        let mut no_usage = rec("2026-10-18T05:00:00.100Z", &[("a", 7)]);
        no_usage["usage_missing"] = serde_json::json!(true);
        let rep = report(vec![no_usage]);
        let text = render_text(&rep, None);
        assert!(text.contains("re-billed        — (no step carries usage)"));
        let v = report_json(&rep, None);
        assert!(v["summary"]["rebill"]["tokens"].is_null());
        assert_eq!(v["summary"]["rebill"]["excluded_rows"], 1);
        assert_eq!(v["summary"]["rebill"]["rows"], 0);
        // the row column: `—`, never 0, on a row without usage.
        assert!(v["rows"][0]["rebill_tok"].is_null());
        assert!(text.contains("rebill_tok"));
    }

    /// `by break kind` counts rewrite rows only, always all four
    /// words, and a rewrite whose block carries no kind (`at: null`)
    /// lands in no bucket — an absence, never a guess (§9.4.5).
    #[test]
    fn by_break_kind_all_four_and_kindless_lands_nowhere() {
        let rep = report(vec![
            rec_usage("2026-10-18T05:00:00.100Z", &[("a", 10)], 100, 0),
            rec_usage("2026-10-18T05:00:01.100Z", &[("z", 10)], 100, 0),
        ]);
        assert_eq!(rep.rows[1].class, "rewrite");
        let mut kindless = rec_usage("2026-10-18T05:00:01.100Z", &[("z", 10)], 100, 0);
        kindless["prefix"]["blocks"][0]
            .as_object_mut()
            .unwrap()
            .remove("kind");
        let rep = report(vec![
            rec_usage("2026-10-18T05:00:00.100Z", &[("a", 10)], 100, 0),
            kindless,
        ]);
        assert_eq!(rep.rows[1].at, None);
        let total: u64 = rep.rebill.by_break_kind.values().map(|(c, _)| c).sum();
        assert_eq!(total, 0, "a kindless rewrite lands in no bucket");
        assert_eq!(rep.rebill.by_break_kind.len(), 4, "all four words, always");
    }
}
