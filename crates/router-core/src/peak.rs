//! Peak/off-peak window matching (DESIGN §12.4 `PeakWindow`). No clock
//! dependency: `at` is injected by the caller.

/// A UTC instant (epoch seconds). The rest of the pipeline must never read the
/// current time on its own (content determinism).
pub type Timestamp = u64;

/// Fixed time zone. v0.1 only needs UTC offsets declared by windows; the IANA
/// database is deliberately not pulled in (dependency discipline).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tz {
    Utc,
    /// Offset in minutes relative to UTC (e.g. +08:00 = 480; Hawaii = -600).
    OffsetMin(i32),
}

impl Tz {
    const fn offset_s(self) -> i64 {
        match self {
            Self::Utc => 0,
            Self::OffsetMin(m) => m as i64 * 60,
        }
    }
}

/// Days of the week as a bitmask (bit 0 = Sunday … bit 6 = Saturday).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Weekdays(pub u8);

impl Weekdays {
    /// Monday through Friday = bit1..bit5 (bit0 = Sunday, bit6 = Saturday).
    pub const MON_FRI: Self = Self(0b0011_1110);

    /// `weekday`: 0 = Sunday … 6 = Saturday.
    pub const fn contains(self, weekday: u32) -> bool {
        self.0 & (1 << weekday) != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeakWindow {
    pub days: Weekdays,
    pub from_min: u16,
    pub to_min: u16,
    pub tz: Tz,
}

/// Gregorian calendar date (Howard Hinnant's algorithm, no chrono dependency).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = if m > 2 { m - 3 } else { m + 9 } as u64;
    let doy = (153 * mp + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

/// Split epoch seconds into (Gregorian year, month, day, minute of day,
/// weekday (0=Sun)).
pub fn timestamp_parts(epoch_s: u64, tz: Tz) -> (i64, u32, u32, u16, u32) {
    let local = epoch_s as i64 + tz.offset_s();
    let days = local.div_euclid(86_400);
    let secs_of_day = local.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    // 1970-01-01 was a Thursday(4).
    let weekday = (days.rem_euclid(7) + 4) as u32 % 7;
    (y, m, d, (secs_of_day / 60) as u16, weekday)
}

/// Epoch seconds of 00:00 UTC on the given day.
pub fn utc_midnight_epoch(y: i64, m: u32, d: u32) -> u64 {
    (days_from_civil(y, m, d) * 86_400) as u64
}

impl PeakWindow {
    /// Interval semantics: `from` inclusive, `to` exclusive; `from > to` means
    /// the window spans midnight. `to == 0` is treated as 24:00 (end of day).
    pub fn matches(&self, epoch_s: u64) -> bool {
        let (_, _, _, minute, weekday) = timestamp_parts(epoch_s, self.tz);
        if !self.days.contains(weekday) {
            return false;
        }
        let m = minute;
        let (from, to) = (
            self.from_min,
            if self.to_min == 0 { 1440 } else { self.to_min },
        );
        if from <= to {
            m >= from && m < to
        } else {
            m >= from || m < to
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeakTable {
    /// Peak multiplier (2.0 → 200). 100 = no peak/off-peak difference.
    pub multiplier_pct: u32,
    pub windows: Vec<PeakWindow>,
}

impl PeakTable {
    /// The first matching window decides the peak; no windows or none
    /// matching → None (flat price).
    pub fn multiplier_at(&self, epoch_s: u64) -> Option<u32> {
        self.windows
            .iter()
            .find(|w| w.matches(epoch_s))
            .map(|_| self.multiplier_pct)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-09-21 Monday 00:00 UTC = 1_789_948_800; 2026-09-20 Sunday midnight
    // = MON − 86400 (verified with python datetime).
    const MON: u64 = 1_789_948_800;
    const SUN: u64 = MON - 86_400;
    const NOON_UTC: u16 = 12 * 60;

    fn mon_min(offset_min: u64) -> u64 {
        MON + offset_min * 60
    }

    #[test]
    fn weekday_membership() {
        let mf = Weekdays::MON_FRI;
        assert!(mf.contains(1) && mf.contains(5));
        assert!(!mf.contains(0) && !mf.contains(6));
    }

    #[test]
    fn window_hits_weekday_and_misses_weekend() {
        let w = PeakWindow {
            days: Weekdays::MON_FRI,
            from_min: 0,
            to_min: NOON_UTC,
            tz: Tz::Utc,
        };
        assert!(w.matches(mon_min(1)));
        // The same instant on Sunday: no match
        assert!(!w.matches(SUN + 60));
    }

    #[test]
    fn window_boundaries_half_open() {
        let w = PeakWindow {
            days: Weekdays(0xFF),
            from_min: 60, // 01:00
            to_min: 240,  // 04:00 (exclusive)
            tz: Tz::Utc,
        };
        assert!(!w.matches(mon_min(59))); // 00:59
        assert!(w.matches(mon_min(60))); // 01:00 inclusive
        assert!(w.matches(mon_min(239))); // 03:59
        assert!(!w.matches(mon_min(240))); // 04:00 exclusive
    }

    #[test]
    fn window_wraparound_midnight() {
        let w = PeakWindow {
            days: Weekdays(0xFF),
            from_min: 22 * 60,
            to_min: 2 * 60,
            tz: Tz::Utc,
        };
        assert!(w.matches(mon_min(23 * 60))); // 23:00 ∈ window
        assert!(!w.matches(mon_min(3 * 60))); // 03:00 ∉
        assert!(w.matches(mon_min(1 * 60))); // 01:00 ∈ (post-midnight leg)
    }

    #[test]
    fn window_respects_tz_offset() {
        // A window declared as 09:00-10:00 at +08:00 = 01:00-02:00 UTC
        let w = PeakWindow {
            days: Weekdays(0xFF),
            from_min: 9 * 60,
            to_min: 10 * 60,
            tz: Tz::OffsetMin(480),
        };
        assert!(!w.matches(mon_min(0))); // UTC 00:00 = local 08:00
        assert!(w.matches(mon_min(90))); // UTC 01:30 = local 09:30
        assert!(!w.matches(mon_min(2 * 60))); // UTC 02:00 = local 10:00 (exclusive)
    }

    #[test]
    fn peak_table_first_hit_and_none() {
        let table = PeakTable {
            multiplier_pct: 200,
            windows: vec![PeakWindow {
                days: Weekdays(0xFF),
                from_min: 60,
                to_min: 120,
                tz: Tz::Utc,
            }],
        };
        assert_eq!(table.multiplier_at(mon_min(90)), Some(200));
        assert_eq!(table.multiplier_at(mon_min(180)), None);
        let empty = PeakTable {
            multiplier_pct: 200,
            windows: vec![],
        };
        assert_eq!(empty.multiplier_at(MON), None);
    }
}
