//! `GET /metrics` (spec §4.16, ADR-041, DESIGN §12.21): the operator's
//! scrape surface — the last 900 seconds of this process's own
//! `trace.dir`, rendered in the Prometheus text exposition format.
//!
//! **The single-owner rule is structural here** (ADR-041 §4): every value
//! is one the §9.2 derivation already produced — `exposition` takes the
//! derivation's OUTPUT (`TraceFigures`) and admits no records, so a
//! second, parallel derivation is not expressible through this seam. The
//! module formats; it never computes a figure.
//!
//! The observation boundary (AGENTS constraint 3): the read site is the
//! config's own trace directory and nothing else — no path outside that
//! directory is opened by this module, and the exposition is a derived
//! view for operators, never a serving-path → observation channel
//! (ADR-005).

#![forbid(unsafe_code)]

use std::path::Path;

use crate::stats::TraceFigures;

/// The frozen window (spec §4.16): 900 seconds, a process constant — not
/// a config key, not a query parameter, not a header. It is stated
/// in-band on every response by `router_metrics_window_seconds`.
pub const WINDOW_MS: i64 = 900_000;

/// Pure: the derivation's output in, the exposition out (spec §4.16).
/// It takes NO records — a second value cannot be derived through this
/// seam. `read_error` is `Some(reason)` when the window read itself
/// failed: every trace-derived figure is then a hole, and each hole is
/// named by a `#` comment (§9.2's "omitted with a named reason, never a
/// 0" rule, applied to a medium with no stderr).
pub fn exposition(
    figures: &TraceFigures,
    files_read: usize,
    plan_family: Option<&str>,
    read_error: Option<&str>,
) -> String {
    let f = figures;
    let mut out = String::new();
    // The response's own bookkeeping (§3.4 #2): how many omission arms
    // fired IN THIS RESPONSE — never a figure over records.
    let mut omitted: u64 = 0;

    // #1 — the window, always: the surface's own statement of scope
    // (§9.2's "a report must state the window it covers", kept in-band).
    series(
        &mut out,
        "router_metrics_window_seconds",
        "The trailing window this exposition covers, in seconds (a process constant, spec §4.16 — not a knob).",
        &[],
        &(WINDOW_MS / 1_000).to_string(),
    );

    // The one figure this surface never carries (spec §4.16, "The one
    // figure that is not here"): the omission is a frozen constant of
    // every response, named in-band so it is never read as a zero. This
    // comment's wording is normative (spec §4.16).
    out.push_str("# router: unknown_outcome_requests omitted — the figure lives in the event log and this surface does not scan it\n");
    omitted += 1;

    if let Some(reason) = read_error {
        // The read itself failed (e.g. the live dir was removed under a
        // running process): every trace-derived figure is a hole. The
        // trace-read arm fires, and with it every arm below it (§3.5) —
        // each with its own frozen wording, never a zero series.
        out.push_str(&format!(
            "# router: trace unreadable — {reason}; every trace-derived figure is omitted\n"
        ));
        for comment in [
            "# router: cache_hit_rate omitted — no input tokens in the window",
            "# router: prefix_continuity_p50 omitted — no continuity sample in the window",
            "# router: overhead_ms_p99 omitted — no upstream-measured record in the window",
            "# router: stateful_inbound_rate omitted — the window holds no records",
            "# router: no currency in the window — no money series",
        ] {
            out.push_str(comment);
            out.push('\n');
        }
        omitted += 6;
        emit_omitted(&mut out, omitted);
        return out;
    }

    // #3 — the read's own count (the rollover files this read opened).
    series(
        &mut out,
        "router_trace_files_read",
        "The §4.1 rollover files this read opened (≤ 2 for a 900s window).",
        &[],
        &files_read.to_string(),
    );

    // #4–#6, #8 — the request counts.
    series(
        &mut out,
        "router_requests",
        "Requests in the window.",
        &[],
        &f.requests.to_string(),
    );
    series(
        &mut out,
        "router_requests_succeeded",
        "Requests in the window answered 2xx.",
        &[],
        &f.succeeded.to_string(),
    );
    series(
        &mut out,
        "router_requests_failed",
        "Requests in the window not answered 2xx.",
        &[],
        &f.failed.to_string(),
    );
    // #7 — the failed split, only for kinds the window produced (never a
    // synthetic zero for a kind nothing carried). `kind` is §8's closed
    // vocabulary, bounded by the code, never by traffic.
    if !f.failure_kinds.is_empty() {
        help_type(
            &mut out,
            "router_failures_by_kind",
            "Failed requests split by §8's errors[].kind — only kinds the window produced.",
        );
        for (kind, n) in &f.failure_kinds {
            sample(
                &mut out,
                "router_failures_by_kind",
                &[("kind", kind)],
                &n.to_string(),
            );
        }
    }
    series(
        &mut out,
        "router_requests_usage_missing",
        "Requests with usage_missing: true — excluded from every rate and every sum, never read as 0.",
        &[],
        &f.usage_missing.to_string(),
    );

    // #9 — money: per currency and never summed across them (§4.8); a
    // money series exists only for a currency the window actually holds,
    // and there is NO tier="total" series (it would double-count under a
    // PromQL sum() — the printed total is one sum() away).
    if f.currencies.is_empty() {
        out.push_str("# router: no currency in the window — no money series\n");
        omitted += 1;
    } else {
        help_type(
            &mut out,
            "router_cost_nano",
            "Cost in integer nano of the named currency — money is reported per currency and never summed across them, and no tier=\"total\" series exists (a total would double-count under sum()).",
        );
        for cur in f.currencies.keys() {
            for (tier, map) in [
                ("input_miss", &f.input_miss_nano),
                ("input_hit", &f.input_hit_nano),
                ("cache_write", &f.cache_write_nano),
                ("output", &f.output_nano),
            ] {
                sample(
                    &mut out,
                    "router_cost_nano",
                    &[
                        ("tier", tier),
                        ("currency", cur),
                        ("provenance", "verified"),
                    ],
                    &map.get(cur).copied().unwrap_or(0).to_string(),
                );
            }
        }
    }

    // #10–#11 — the hit rate's numerator and denominator, as measurements.
    series(
        &mut out,
        "router_cache_input_cached_tokens",
        "Cached input tokens in the window (the hit rate's numerator).",
        &[("provenance", "verified")],
        &f.input_cached_tokens.to_string(),
    );
    series(
        &mut out,
        "router_cache_input_tokens",
        "Input tokens in the window (the hit rate's denominator).",
        &[("provenance", "verified")],
        &f.input_total_tokens.to_string(),
    );

    // #12 — the hit rate, through the derivation's own hoisted ratio
    // (one owner; `router stats` divides in the same place).
    match crate::stats::cache_hit_rate(f) {
        Some(r) => series(
            &mut out,
            "router_cache_hit_rate",
            "Cache hit rate over the window (cached / total input tokens).",
            &[("provenance", "verified")],
            &format!("{r:.4}"),
        ),
        None => {
            out.push_str("# router: cache_hit_rate omitted — no input tokens in the window\n");
            omitted += 1;
        }
    }

    // #13 — continuity p50, the derivation's own quantile.
    match crate::stats::median(&mut f.continuity.clone()) {
        Some(c) => series(
            &mut out,
            "router_prefix_continuity_p50",
            "Prefix continuity p50 over the window (a predictor, not a measurement of what the provider did).",
            &[("provenance", "inferred")],
            &format!("{c:.4}"),
        ),
        None => {
            out.push_str(
                "# router: prefix_continuity_p50 omitted — no continuity sample in the window\n",
            );
            omitted += 1;
        }
    }

    // #14 — the two savings lines, one series per label the window
    // produced (never a synthetic zero for a verdict nothing carried;
    // the two labels are never to be summed together).
    if f.verified_savings_tokens > 0 || f.inferred_savings_tokens > 0 {
        help_type(
            &mut out,
            "router_transform_savings_tokens",
            "Transform savings in tokens, split by §7's label — the two provenance values are one metric name and are never to be summed together.",
        );
        if f.verified_savings_tokens > 0 {
            sample(
                &mut out,
                "router_transform_savings_tokens",
                &[("provenance", "verified")],
                &f.verified_savings_tokens.to_string(),
            );
        }
        if f.inferred_savings_tokens > 0 {
            sample(
                &mut out,
                "router_transform_savings_tokens",
                &[("provenance", "inferred")],
                &f.inferred_savings_tokens.to_string(),
            );
        }
    }

    // #15–#19 — the plan series, only when the loaded config declares a
    // `plan_policy` (§9.1's no-fabricated-plan-section rule, applied to
    // this surface); `family` is that policy's family verbatim.
    if let Some(fam) = plan_family {
        series(
            &mut out,
            "router_plan_switches",
            "Requests whose result.plan_switch is present (the family's spills).",
            &[("family", fam)],
            &f.switches.to_string(),
        );
        if !f.currencies.is_empty() {
            help_type(
                &mut out,
                "router_plan_switch_cost_nano",
                "The displaced requests' own measured cost.total, integer nano per currency.",
            );
            for cur in f.currencies.keys() {
                sample(
                    &mut out,
                    "router_plan_switch_cost_nano",
                    &[
                        ("family", fam),
                        ("currency", cur),
                        ("provenance", "verified"),
                    ],
                    &f.switch_cost_verified_nano
                        .get(cur)
                        .copied()
                        .unwrap_or(0)
                        .to_string(),
                );
            }
        }
        series(
            &mut out,
            "router_plan_switch_reprefill_tokens",
            "Re-prefill tokens over the switch records (the inferred column — never added into the verified sum).",
            &[("family", fam), ("provenance", "inferred")],
            &f.reprefill_tokens.to_string(),
        );
        if !f.currencies.is_empty() {
            help_type(
                &mut out,
                "router_plan_switch_reprefill_cost_nano",
                "The switch records' switch_cost_nano, integer nano per currency (inferred).",
            );
            for cur in f.currencies.keys() {
                sample(
                    &mut out,
                    "router_plan_switch_reprefill_cost_nano",
                    &[
                        ("family", fam),
                        ("currency", cur),
                        ("provenance", "inferred"),
                    ],
                    &f.reprefill_cost_nano
                        .get(cur)
                        .copied()
                        .unwrap_or(0)
                        .to_string(),
                );
            }
        }
        series(
            &mut out,
            "router_plan_switches_without_usage",
            "Switches whose displaced record carried no usage (they keep the inferred label).",
            &[("family", fam)],
            &f.switches_without_usage.to_string(),
        );
    }

    // #20 — the stateful inbound rate, through the same hoisted ratio.
    match crate::stats::stateful_inbound_rate(f) {
        Some(r) => series(
            &mut out,
            "router_stateful_inbound_rate",
            "Stateful-inbound requests as a ratio of the window's requests (always 0 in v0.1 — gap G-F).",
            &[("provenance", "count")],
            &format!("{r:.4}"),
        ),
        None => {
            out.push_str(
                "# router: stateful_inbound_rate omitted — the window holds no records\n",
            );
            omitted += 1;
        }
    }

    // #21 — overhead p99, the derivation's own quantile over its own
    // sample (`overhead_ms − upstream_ms`; a record with no upstream
    // measurement is excluded by `aggregate`, not read as 0).
    match crate::stats::p99(&mut f.overhead_ms.clone()) {
        Some(v) => series(
            &mut out,
            "router_overhead_ms_p99",
            "The router's own overhead (overhead_ms − upstream_ms), nearest-rank p99, integer milliseconds.",
            &[("provenance", "measured")],
            &v.to_string(),
        ),
        None => {
            out.push_str(
                "# router: overhead_ms_p99 omitted — no upstream-measured record in the window\n",
            );
            omitted += 1;
        }
    }

    // #2 — the response's own bookkeeping, emitted last because it
    // counts the arms above.
    emit_omitted(&mut out, omitted);
    out
}

/// The read + the call above: the whole of the handler's body (DESIGN
/// §12.21). Reads the process's own resolved `trace.dir` (the reload
/// refuses to move it, ADR-040 D5 — no revision coupling needed) and the
/// plan family off the revision in force (a reload MAY repoint the
/// policy); touches no request byte, no store, and no file outside the
/// process's own trace directory.
pub fn snapshot(state: &router_proxy::AppState) -> String {
    let plan_family = state
        .revision
        .capture()
        .forwarder
        .config
        .plan_policy
        .as_ref()
        .map(|p| p.family.clone());
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let start_ms = now_ms.saturating_sub(WINDOW_MS);
    match crate::stats::read_window_records(Path::new(&state.trace_dir), start_ms, now_ms) {
        Ok((records, files_read)) => {
            let figures = crate::stats::aggregate(&records);
            exposition(&figures, files_read, plan_family.as_deref(), None)
        }
        Err(reason) => exposition(
            &TraceFigures::default(),
            0,
            plan_family.as_deref(),
            Some(&reason),
        ),
    }
}

fn emit_omitted(out: &mut String, omitted: u64) {
    series(
        out,
        "router_metrics_omitted_figures",
        "How many figures this response omitted with a named reason (the # router: comments) — the response's own bookkeeping, never a figure over records.",
        &[],
        &omitted.to_string(),
    );
}

fn series(out: &mut String, name: &str, help: &str, labels: &[(&str, &str)], value: &str) {
    help_type(out, name, help);
    sample(out, name, labels, value);
}

fn help_type(out: &mut String, name: &str, help: &str) {
    // Every series is a gauge (ADR-041 §3.6): the figures are
    // window-scoped and may go down, so no name ends in `_total` and no
    // consumer may be invited to rate() a non-monotonic value.
    out.push_str(&format!("# HELP {name} {}\n", help_escape(help)));
    out.push_str(&format!("# TYPE {name} gauge\n"));
}

fn sample(out: &mut String, name: &str, labels: &[(&str, &str)], value: &str) {
    out.push_str(name);
    if !labels.is_empty() {
        out.push('{');
        let parts: Vec<String> = labels
            .iter()
            .map(|(k, v)| format!("{k}=\"{}\"", label_escape(v)))
            .collect();
        out.push_str(&parts.join(","));
        out.push('}');
    }
    out.push(' ');
    out.push_str(value);
    out.push('\n');
}

/// Prometheus label-value escaping: backslash, double-quote, newline.
fn label_escape(v: &str) -> String {
    v.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// HELP-text escaping: backslash and newline.
fn help_escape(v: &str) -> String {
    v.replace('\\', "\\\\").replace('\n', "\\n")
}
