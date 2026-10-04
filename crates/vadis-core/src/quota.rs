//! Quota model (DESIGN §5/§12.4). Pure functions + explicit state; the
//! accounting basis = `input_total + output` (GAP-Q1 default).

use serde::Serialize;

use crate::cost::Usage;
use crate::peak::utc_midnight_epoch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaWindow {
    Monthly { reset_day: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum OverQuota {
    Block,
    Spill,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaPlan {
    pub models: Vec<String>,
    pub window: QuotaWindow,
    pub tokens: u64,
    pub over_quota: OverQuota,
    pub source: String,
}

/// Mutable state, one per plan. `window_start_epoch_s` is the start of the
/// current metering window (aligned to UTC midnight).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaState {
    pub plan_idx: usize,
    pub window_start_epoch_s: u64,
    pub tokens_used: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaVerdict {
    Inside {
        remaining_before: u64,
    },
    /// The overflow portion is billed at input_miss (DESIGN §5);
    /// `billable_tokens` = the number of tokens beyond the allowance.
    Spill {
        billable_tokens: u64,
    },
    /// `over_quota=block` → Guard Reject(quota_exceeded).
    Blocked {
        remaining: u64,
    },
}

fn is_leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(y) => 29,
        2 => 28,
        _ => 0,
    }
}

fn prev_month(y: i64, m: u32) -> (i64, u32) {
    if m == 1 {
        (y - 1, 12)
    } else {
        (y, m - 1)
    }
}

fn next_month(y: i64, m: u32) -> (i64, u32) {
    if m == 12 {
        (y + 1, 1)
    } else {
        (y, m + 1)
    }
}

/// Start (UTC midnight) of the metering window containing the given instant.
/// When `reset_day` exceeds the current month's day count, the last day of
/// the month is used (a day-31 reset lands on month-end in 30-/28-day months,
/// keeping the unit consistent).
pub fn window_start_for(now_epoch_s: u64, reset_day: u8) -> u64 {
    let (y, m, d, _, _) = crate::peak::timestamp_parts(now_epoch_s, crate::peak::Tz::Utc);
    let rd = reset_day as u32;
    let (wy, wm) = if d >= rd { (y, m) } else { prev_month(y, m) };
    utc_midnight_epoch(wy, wm, rd.min(days_in_month(wy, wm)))
}

/// The next reset point after `window_start` (public: the plan policy's
/// probe-deferral rule reads it — spec §4.6 rule 3).
pub fn next_reset(window_start: u64, reset_day: u8) -> u64 {
    let (y, m, d, _, _) = crate::peak::timestamp_parts(window_start, crate::peak::Tz::Utc);
    let rd = reset_day as u32;
    debug_assert_eq!(d, rd.min(days_in_month(y, m)));
    let (ny, nm) = next_month(y, m);
    utc_midnight_epoch(ny, nm, rd.min(days_in_month(ny, nm)))
}

/// Metering basis: `usage.input_total + usage.output` (GAP-Q1 blueprint
/// default; if the spec changes after write-back, this single point changes).
fn chargeable(usage: &Usage) -> u64 {
    usage.input_total.saturating_add(usage.output)
}

/// The single state-write point: align the window first (crossed window →
/// reset to zero), then classify, then book.
pub fn charge(
    plan: &QuotaPlan,
    st: &mut QuotaState,
    usage: &Usage,
    now_epoch_s: u64,
) -> QuotaVerdict {
    let QuotaWindow::Monthly { reset_day } = plan.window;
    let start = window_start_for(now_epoch_s, reset_day);
    if st.window_start_epoch_s != start {
        st.window_start_epoch_s = start;
        st.tokens_used = 0;
    }
    let _ = next_reset(start, reset_day); // reset-point derivation is already covered by window_start_for
    let charged = chargeable(usage);
    let remaining = plan.tokens.saturating_sub(st.tokens_used);
    let verdict = if st.tokens_used >= plan.tokens {
        match plan.over_quota {
            OverQuota::Block => QuotaVerdict::Blocked { remaining: 0 },
            OverQuota::Spill => QuotaVerdict::Spill {
                billable_tokens: charged,
            },
        }
    } else if charged <= remaining {
        QuotaVerdict::Inside {
            remaining_before: remaining,
        }
    } else {
        match plan.over_quota {
            OverQuota::Block => QuotaVerdict::Blocked { remaining },
            OverQuota::Spill => QuotaVerdict::Spill {
                billable_tokens: charged - remaining,
            },
        }
    };

    st.tokens_used = match verdict {
        QuotaVerdict::Inside { .. } => st.tokens_used.saturating_add(charged),
        // After a spill the allowance is exhausted; the overflow does not
        // consume plan allowance (billed separately at input_miss, DESIGN §5).
        QuotaVerdict::Spill { .. } => plan.tokens,
        QuotaVerdict::Blocked { .. } => st.tokens_used,
    };
    verdict
}

#[cfg(test)]
mod tests {
    use super::*;

    // Anchors computed with a tool (UTC midnight, verified with python
    // datetime):
    const SEP_1_2026: u64 = 1_788_220_800; // 2026-09-01
    const AUG_1_2026: u64 = 1_785_542_400; // 2026-08-01
    const SEP_15_2026: u64 = 1_789_430_400; // 2026-09-15
    const JAN_31_2026: u64 = 1_769_817_600; // 2026-01-31
    const FEB_15_2026: u64 = 1_771_113_600; // 2026-02-15

    fn plan(tokens: u64, over: OverQuota) -> QuotaPlan {
        QuotaPlan {
            models: vec!["glm-5.3".into()],
            window: QuotaWindow::Monthly { reset_day: 1 },
            tokens,
            over_quota: over,
            source: "test".into(),
        }
    }

    fn usage(input: u64, output: u64) -> Usage {
        Usage {
            input_total: input,
            input_cached: 0,
            cache_write: 0,
            output,
            reasoning: 0,
        }
    }

    #[test]
    fn inside_charges_input_plus_output() {
        let p = plan(1_000, OverQuota::Block);
        let mut st = QuotaState {
            plan_idx: 0,
            window_start_epoch_s: SEP_1_2026,
            tokens_used: 100,
        };
        let v = charge(&p, &mut st, &usage(300, 200), SEP_15_2026);
        assert_eq!(
            v,
            QuotaVerdict::Inside {
                remaining_before: 900
            }
        );
        assert_eq!(st.tokens_used, 600);
    }

    #[test]
    fn window_rollover_resets_usage() {
        let p = plan(1_000, OverQuota::Block);
        let mut st = QuotaState {
            plan_idx: 0,
            window_start_epoch_s: AUG_1_2026,
            tokens_used: 900,
        };
        let v = charge(&p, &mut st, &usage(100, 0), SEP_15_2026);
        assert_eq!(st.window_start_epoch_s, SEP_1_2026); // September → the new window starts 9/1
        assert_eq!(st.tokens_used, 100); // the old window's 900 is zeroed, then re-counted
        assert_eq!(
            v,
            QuotaVerdict::Inside {
                remaining_before: 1_000
            }
        );
    }

    #[test]
    fn spill_reports_overflow_tokens() {
        let p = plan(1_000, OverQuota::Spill);
        let mut st = QuotaState {
            plan_idx: 0,
            window_start_epoch_s: SEP_1_2026,
            tokens_used: 900,
        };
        let v = charge(&p, &mut st, &usage(300, 0), SEP_15_2026);
        assert_eq!(
            v,
            QuotaVerdict::Spill {
                billable_tokens: 200
            }
        );
        assert_eq!(st.tokens_used, 1_000); // plan allowance exhausted; the overflow does not consume allowance
    }

    #[test]
    fn block_rejects_without_mutating_usage() {
        let p = plan(1_000, OverQuota::Block);
        let mut st = QuotaState {
            plan_idx: 0,
            window_start_epoch_s: SEP_1_2026,
            tokens_used: 900,
        };
        let v = charge(&p, &mut st, &usage(300, 0), SEP_15_2026);
        assert_eq!(v, QuotaVerdict::Blocked { remaining: 100 });
        assert_eq!(st.tokens_used, 900); // rejected → nothing booked
    }

    #[test]
    fn exhausted_block_reports_zero_remaining() {
        let p = plan(1_000, OverQuota::Block);
        let mut st = QuotaState {
            plan_idx: 0,
            window_start_epoch_s: SEP_1_2026,
            tokens_used: 1_000,
        };
        assert_eq!(
            charge(&p, &mut st, &usage(1, 0), SEP_15_2026),
            QuotaVerdict::Blocked { remaining: 0 }
        );
    }

    #[test]
    fn reset_day_clamps_in_short_months() {
        // reset_day=31: February 15 belongs to the window opened 1/31
        let (y, m, d, _, _) = crate::peak::timestamp_parts(FEB_15_2026, crate::peak::Tz::Utc);
        assert_eq!((y, m, d), (2026, 2, 15));
        assert_eq!(window_start_for(FEB_15_2026, 31), JAN_31_2026);
        // The other direction: January 15 belongs to the window opened 12/31
        // of the previous year
        let dec31_prev = JAN_31_2026 - 31 * 86_400; // 2025-12-31
        assert_eq!(window_start_for(JAN_31_2026 - 16 * 86_400, 31), dec31_prev);
    }
}
