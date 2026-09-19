//! 峰谷窗口匹配（DESIGN §12.4 `PeakWindow`）。无时钟依赖：`at` 由调用方注入。

/// UTC 时刻（epoch 秒）。管线的其余部分不得自行取当前时间（内容确定性）。
pub type Timestamp = u64;

/// 固定时区。v0.1 只需窗口声明的 UTC 偏移；IANA 数据库不引入（依赖精简）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tz {
    Utc,
    /// 相对 UTC 的偏移分钟数（例：+08:00 = 480；夏威夷 = -600）。
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

/// 一周内的天，位掩码（bit 0 = Sunday … bit 6 = Saturday）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Weekdays(pub u8);

impl Weekdays {
    /// 周一至周五 = bit1..bit5（bit0 = Sunday，bit6 = Saturday）。
    pub const MON_FRI: Self = Self(0b0011_1110);

    /// `weekday`：0 = Sunday … 6 = Saturday。
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

/// 公历日（Howard Hinnant 算法，无 chrono 依赖）。
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

/// 拆分 epoch 秒为（公历年, 月, 日, 当日分钟, weekday(0=Sun)）。
pub fn timestamp_parts(epoch_s: u64, tz: Tz) -> (i64, u32, u32, u16, u32) {
    let local = epoch_s as i64 + tz.offset_s();
    let days = local.div_euclid(86_400);
    let secs_of_day = local.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    // 1970-01-01 是 Thursday(4)。
    let weekday = (days.rem_euclid(7) + 4) as u32 % 7;
    (y, m, d, (secs_of_day / 60) as u16, weekday)
}

/// epoch 当天 00:00 UTC 的 epoch 秒。
pub fn utc_midnight_epoch(y: i64, m: u32, d: u32) -> u64 {
    (days_from_civil(y, m, d) * 86_400) as u64
}

impl PeakWindow {
    /// 区间语义：`from` 含、`to` 不含；`from > to` 表示跨午夜。
    /// `to == 0` 视为 24:00（当日收尾）。
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
    /// 峰段倍率（2.0 → 200）。100 = 无峰谷差异。
    pub multiplier_pct: u32,
    pub windows: Vec<PeakWindow>,
}

impl PeakTable {
    /// 首个命中窗口决定峰段；无窗口或全不命中 → None（平价）。
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

    // 2026-09-21 周一 UTC 零点 = 1_789_948_800；2026-09-20 周日零点 = MON − 86400（python datetime 校验）。
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
        // 同一时刻在周日：不命中
        assert!(!w.matches(SUN + 60));
    }

    #[test]
    fn window_boundaries_half_open() {
        let w = PeakWindow {
            days: Weekdays(0xFF),
            from_min: 60, // 01:00
            to_min: 240,  // 04:00（不含）
            tz: Tz::Utc,
        };
        assert!(!w.matches(mon_min(59))); // 00:59
        assert!(w.matches(mon_min(60))); // 01:00 含
        assert!(w.matches(mon_min(239))); // 03:59
        assert!(!w.matches(mon_min(240))); // 04:00 不含
    }

    #[test]
    fn window_wraparound_midnight() {
        let w = PeakWindow {
            days: Weekdays(0xFF),
            from_min: 22 * 60,
            to_min: 2 * 60,
            tz: Tz::Utc,
        };
        assert!(w.matches(mon_min(23 * 60))); // 23:00 ∈ 窗口
        assert!(!w.matches(mon_min(3 * 60))); // 03:00 ∉
        assert!(w.matches(mon_min(1 * 60))); // 01:00 ∈（跨午夜段）
    }

    #[test]
    fn window_respects_tz_offset() {
        // 窗口按 +08:00 的 09:00-10:00 声明 = UTC 01:00-02:00
        let w = PeakWindow {
            days: Weekdays(0xFF),
            from_min: 9 * 60,
            to_min: 10 * 60,
            tz: Tz::OffsetMin(480),
        };
        assert!(!w.matches(mon_min(0))); // UTC 00:00 = 本地 08:00
        assert!(w.matches(mon_min(90))); // UTC 01:30 = 本地 09:30
        assert!(!w.matches(mon_min(2 * 60))); // UTC 02:00 = 本地 10:00（不含）
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
