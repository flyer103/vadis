//! 五档成本引擎（DESIGN §5/§12.4）。全程整数 NanoUsd；浮点只允许出现在
//! 最终格式化输出层（本模块不含）。

use serde::Serialize;

use crate::peak::{PeakTable, Timestamp};

/// 金额定点：1 NanoUsd = 1e-9 USD（ADR-006 裁定口径）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct NanoUsd(pub u64);

impl NanoUsd {
    pub const ZERO: Self = Self(0);

    pub const fn saturating_add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }
}

/// 单价：nano-USD / 1K token。config 的 USD/1K 在装载时一次性整数化（§12.5），
/// 运行期不再出现小数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Price(pub u64);

/// tokens × price(每 1K) → NanoUsd；向下取整（§12.4：最后一次除法向下取整）。
fn tier_nano(tokens: u64, price: Price) -> NanoUsd {
    NanoUsd(saturating_div_1k(tokens as u128 * price.0 as u128))
}

/// u128 → u64 饱和 + 千分位向下取整（成本路径唯一取整点，单测锁定）。
fn saturating_div_1k(nano: u128) -> u64 {
    let q = nano / 1000;
    if q > u64::MAX as u128 {
        u64::MAX
    } else {
        q as u64
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceTable {
    pub input_miss: Price,
    pub input_hit: Price,
    pub cache_write: Price,
    pub output: Price,
    pub peak: PeakTable,
}

/// 归一化 usage（spec §6）。协议解析（R2）把它从三种线格式构造出来；本模块只认这个形状。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Usage {
    pub input_total: u64,
    pub input_cached: u64,
    pub cache_write: u64,
    pub output: u64,
    pub reasoning: u64,
}

impl Usage {
    /// input_miss 档的 token 数。
    pub const fn uncached(&self) -> u64 {
        self.input_total.saturating_sub(self.input_cached)
    }

    /// 缓存命中率（派生指标，非金额路径；允许浮点）。
    #[allow(clippy::float_arithmetic)]
    pub fn cache_hit_rate(&self) -> f32 {
        if self.input_total == 0 {
            0.0
        } else {
            self.input_cached as f32 / self.input_total as f32
        }
    }
}

/// u128 → u64 饱和 + 百分比乘（峰段倍率路径，同样向下取整）。
fn saturating_mul_pct(base: u64, pct: u32) -> u64 {
    let v = base as u128 * pct as u128 / 100;
    if v > u64::MAX as u128 {
        u64::MAX
    } else {
        v as u64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CostBreakdown {
    /// 各档金额为峰前值；`peak_applied_pct` 只作用于 `total`（§12.4 公式）。
    pub input_miss: NanoUsd,
    pub input_hit: NanoUsd,
    pub cache_write: NanoUsd,
    pub output: NanoUsd,
    pub peak_applied_pct: u32,
    pub total: NanoUsd,
}

/// 单请求成本（纯函数）：
/// `cost_nano = Σ_tier(tokens_tier × price_tier)/1000`，峰段命中再整体 `× multiplier_pct/100`。
/// output 档含 reasoning（上游普遍把 reasoning 计入 output 计费）。
pub fn cost(usage: &Usage, price: &PriceTable, at: Timestamp) -> CostBreakdown {
    let miss = tier_nano(usage.uncached(), price.input_miss);
    let hit = tier_nano(usage.input_cached, price.input_hit);
    let write = tier_nano(usage.cache_write, price.cache_write);
    let out = tier_nano(usage.output.saturating_add(usage.reasoning), price.output);

    let base = miss
        .saturating_add(hit)
        .saturating_add(write)
        .saturating_add(out);
    let (peak_applied_pct, total) = match price.peak.multiplier_at(at) {
        Some(pct) => (pct, NanoUsd(saturating_mul_pct(base.0, pct))),
        None => (100, base),
    };
    CostBreakdown {
        input_miss: miss,
        input_hit: hit,
        cache_write: write,
        output: out,
        peak_applied_pct,
        total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peak::{PeakTable, PeakWindow, Tz, Weekdays};

    // 周一 12:00 UTC（python datetime 校验）；flat 峰谷表与时间无关，用同一时刻对照。
    const MON_NOON: u64 = 1_789_992_000;
    const AT: u64 = MON_NOON;

    fn flat_peak() -> PeakTable {
        PeakTable {
            multiplier_pct: 100,
            windows: vec![],
        }
    }

    fn always_peak_200() -> PeakTable {
        PeakTable {
            multiplier_pct: 200,
            windows: vec![PeakWindow {
                days: Weekdays(0xFF),
                from_min: 0,
                to_min: 1440,
                tz: Tz::Utc,
            }],
        }
    }

    #[test]
    fn usage_uncached_and_hit_rate() {
        let u = Usage {
            input_total: 10_000,
            input_cached: 8_000,
            cache_write: 0,
            output: 500,
            reasoning: 0,
        };
        assert_eq!(u.uncached(), 2_000);
        assert!((u.cache_hit_rate() - 0.8).abs() < f32::EPSILON);
        assert_eq!(Usage::default_zero().cache_hit_rate(), 0.0);
    }

    #[test]
    fn cost_all_tiers_no_peak() {
        // miss 10_000 × 150_000/1000 = 1_500_000
        // hit  2_000  × 30_000/1000  =    60_000
        // out  1_000  × 500_000/1000 =   500_000  → total 2_060_000
        let u = Usage {
            input_total: 12_000,
            input_cached: 2_000,
            cache_write: 0,
            output: 1_000,
            reasoning: 0,
        };
        let p = PriceTable {
            input_miss: Price(150_000),
            input_hit: Price(30_000),
            cache_write: Price(0),
            output: Price(500_000),
            peak: flat_peak(),
        };
        let c = cost(&u, &p, AT);
        assert_eq!(c.input_miss, NanoUsd(1_500_000));
        assert_eq!(c.input_hit, NanoUsd(60_000));
        assert_eq!(c.output, NanoUsd(500_000));
        assert_eq!(c.total, NanoUsd(2_060_000));
        assert_eq!(c.peak_applied_pct, 100);
    }

    #[test]
    fn cost_peak_doubles_total_only() {
        let u = Usage {
            input_total: 12_000,
            input_cached: 2_000,
            cache_write: 0,
            output: 1_000,
            reasoning: 0,
        };
        let p = PriceTable {
            input_miss: Price(150_000),
            input_hit: Price(30_000),
            cache_write: Price(0),
            output: Price(500_000),
            peak: always_peak_200(),
        };
        let c = cost(&u, &p, MON_NOON);
        assert_eq!(c.peak_applied_pct, 200);
        assert_eq!(c.total, NanoUsd(4_120_000)); // 2_060_000 × 200/100
        assert_eq!(c.input_miss, NanoUsd(1_500_000)); // 档位保持峰前值
    }

    #[test]
    fn cost_tier_division_floors() {
        let u = Usage {
            input_total: 999,
            input_cached: 0,
            cache_write: 0,
            output: 0,
            reasoning: 0,
        };
        let p = PriceTable {
            input_miss: Price(1),
            input_hit: Price(0),
            cache_write: Price(0),
            output: Price(0),
            peak: flat_peak(),
        };
        assert_eq!(cost(&u, &p, AT).input_miss, NanoUsd(0)); // 999×1/1000 = 0.999 → 0
    }

    #[test]
    fn cost_output_tier_includes_reasoning() {
        let u = Usage {
            input_total: 0,
            input_cached: 0,
            cache_write: 0,
            output: 500,
            reasoning: 300,
        };
        let p = PriceTable {
            input_miss: Price(0),
            input_hit: Price(0),
            cache_write: Price(0),
            output: Price(1_000_000),
            peak: flat_peak(),
        };
        assert_eq!(cost(&u, &p, AT).output, NanoUsd(800_000)); // 800 × 1_000_000/1000
    }

    impl Usage {
        fn default_zero() -> Self {
            Self {
                input_total: 0,
                input_cached: 0,
                cache_write: 0,
                output: 0,
                reasoning: 0,
            }
        }
    }
}
