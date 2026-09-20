//! Five-tier cost engine (DESIGN §5/§12.4). Integer `Nano` throughout
//! (ADR-018: the fixed-point scale kept, the USD name dropped — each
//! table/breakdown carries its own `Currency`); floats are allowed only in
//! the final formatting output layer (not in this module).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::peak::{PeakTable, Timestamp};

/// spec §4.8 (ADR-018): the unit a money value is denominated in. Two
/// values in v0.1; the serialized form is the ISO-4217 code (`"USD"` /
/// `"CNY"`) in the trace, `/health` and the report, and the config
/// spelling is the same code, exact (a lowercase spelling is a load
/// error). No rate exists anywhere: router never converts one currency
/// into another (AGENTS constraint 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Default)]
#[serde(rename_all = "UPPERCASE")]
pub enum Currency {
    #[default]
    Usd,
    Cny,
}

impl Currency {
    /// The ISO-4217 code (`"USD"` / `"CNY"`), the one spelling every
    /// surface uses (§4.8).
    pub const fn as_code(self) -> &'static str {
        match self {
            Self::Usd => "USD",
            Self::Cny => "CNY",
        }
    }

    /// Parse the exact code; anything else — including a lowercase
    /// spelling — is `None` (the config parser turns that into a load
    /// error naming `providers[i].currency`, §12.5).
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "USD" => Some(Self::Usd),
            "CNY" => Some(Self::Cny),
            _ => None,
        }
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code())
    }
}

impl<'de> Deserialize<'de> for Currency {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        match Self::from_code(&s) {
            Some(c) => Ok(c),
            None => Err(serde::de::Error::custom(format!(
                "unknown currency '{s}': expected USD or CNY — the ISO-4217 code is exact, \
                 and an absent key means USD (spec 4.8)"
            ))),
        }
    }
}

/// The raw fixed-point amount: 1e-9 of **the currency carried beside
/// it** (ADR-006's scale, ADR-018's renaming — the name may not assert a
/// unit the value does not hold). It is the integer inside [`Money`];
/// an aggregate is a per-currency map, never a bare `Nano` sum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Nano(pub u64);

impl Nano {
    pub const ZERO: Self = Self(0);

    pub const fn saturating_add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }
}

/// An amount **and** its unit (spec §4.8 / DESIGN §12.4). Every money
/// value that crosses a container boundary — an aggregate, a cap, a
/// plan state, a trace field, a report figure — is a `Money`. There is
/// **no** `impl Add for Money` and no `Sum`: adding two currencies is
/// not expressible, and the one adder returns the mismatch instead of a
/// value, so a future code path that wants to mix units has to name
/// them — it cannot do it by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Money {
    pub nano: Nano,
    pub currency: Currency,
}

/// What [`Money::checked_add`] answers when the units disagree: the
/// currency it refused to add (the left operand's, then the right's —
/// both are named so the reader needs no other context).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrencyMismatch {
    Left(Currency),
    Right(Currency),
}

impl Money {
    pub const ZERO: Self = Self {
        nano: Nano::ZERO,
        currency: Currency::Usd,
    };

    /// The one adder: same-currency adds saturate; a mismatch is an
    /// error, never a value (§4.8: money is never summed across
    /// currencies — the type makes the silent form unrepresentable).
    pub fn checked_add(self, rhs: Self) -> Result<Self, (CurrencyMismatch, CurrencyMismatch)> {
        if self.currency == rhs.currency {
            Ok(Self {
                nano: self.nano.saturating_add(rhs.nano),
                currency: self.currency,
            })
        } else {
            Err((
                CurrencyMismatch::Left(self.currency),
                CurrencyMismatch::Right(rhs.currency),
            ))
        }
    }
}

/// Unit price: nano-currency / 1K tokens, in the currency of the
/// `PriceTable` it belongs to (§4.8). Values from config are integerized
/// once at load time (§12.5); no decimals appear at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Price(pub u64);

/// tokens × price (per 1K) → Nano; floored (§12.4: the final division
/// floors).
fn tier_nano(tokens: u64, price: Price) -> Nano {
    Nano(saturating_div_1k(tokens as u128 * price.0 as u128))
}

/// u128 → u64 saturating + floor at the thousandths step (the single rounding
/// point on the cost path, pinned by unit tests).
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
    /// The unit of every tier in this table — copied from the provider
    /// entry at load time (§4.8: one table, one currency; the parser
    /// never converts).
    pub currency: Currency,
    pub input_miss: Price,
    pub input_hit: Price,
    pub cache_write: Price,
    pub output: Price,
    pub peak: PeakTable,
}

/// Normalized usage (spec §6). Protocol parsing constructs it from the
/// three wire formats; this module only knows this shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
pub struct Usage {
    pub input_total: u64,
    pub input_cached: u64,
    pub cache_write: u64,
    pub output: u64,
    pub reasoning: u64,
}

impl Usage {
    /// Token count of the input_miss tier.
    pub const fn uncached(&self) -> u64 {
        self.input_total.saturating_sub(self.input_cached)
    }

    /// Cache hit rate (a derived metric, not a money path; floats allowed).
    #[allow(clippy::float_arithmetic)]
    pub fn cache_hit_rate(&self) -> f32 {
        if self.input_total == 0 {
            0.0
        } else {
            self.input_cached as f32 / self.input_total as f32
        }
    }
}

/// u128 → u64 saturating + percentage multiply (the peak multiplier path,
/// also floored).
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
    /// Tier amounts are pre-peak values; `peak_applied_pct` applies only to
    /// `total` (§12.4 formula).
    pub input_miss: Nano,
    pub input_hit: Nano,
    pub cache_write: Nano,
    pub output: Nano,
    pub peak_applied_pct: u32,
    pub total: Nano,
    /// The unit every amount here is denominated in — copied from the
    /// price table that produced this breakdown (§4.8), so the pure
    /// function's output is self-describing.
    pub currency: Currency,
}

/// Per-request cost (pure function):
/// `cost_nano = Σ_tier(tokens_tier × price_tier)/1000`, then the whole thing
/// `× multiplier_pct/100` on a peak-window hit. The output tier includes
/// reasoning (upstreams generally bill reasoning as output).
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
        Some(pct) => (pct, Nano(saturating_mul_pct(base.0, pct))),
        None => (100, base),
    };
    CostBreakdown {
        input_miss: miss,
        input_hit: hit,
        cache_write: write,
        output: out,
        peak_applied_pct,
        total,
        currency: price.currency,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peak::{PeakTable, PeakWindow, Tz, Weekdays};

    // Monday 12:00 UTC (verified with python datetime); the flat peak table is
    // time-independent, so the same instant is used as the control.
    const MON_NOON: u64 = 1_789_992_000;
    const AT: u64 = MON_NOON;

    fn table(currency: Currency) -> PriceTable {
        PriceTable {
            currency,
            input_miss: Price(150_000),
            input_hit: Price(30_000),
            cache_write: Price(0),
            output: Price(500_000),
            peak: flat_peak(),
        }
    }

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
        let p = table(Currency::Usd);
        let c = cost(&u, &p, AT);
        assert_eq!(c.input_miss, Nano(1_500_000));
        assert_eq!(c.input_hit, Nano(60_000));
        assert_eq!(c.output, Nano(500_000));
        assert_eq!(c.total, Nano(2_060_000));
        assert_eq!(c.peak_applied_pct, 100);
        assert_eq!(c.currency, Currency::Usd); // carried from the table (§4.8)
        assert_eq!(
            cost(&u, &table(Currency::Cny), AT).currency,
            Currency::Cny,
            "a CNY table prices a CNY breakdown — same arithmetic, its own unit"
        );
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
            peak: always_peak_200(),
            ..table(Currency::Usd)
        };
        let c = cost(&u, &p, MON_NOON);
        assert_eq!(c.peak_applied_pct, 200);
        assert_eq!(c.total, Nano(4_120_000)); // 2_060_000 × 200/100
        assert_eq!(c.input_miss, Nano(1_500_000)); // tiers keep pre-peak values
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
            ..table(Currency::Usd)
        };
        assert_eq!(cost(&u, &p, AT).input_miss, Nano(0)); // 999×1/1000 = 0.999 → 0
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
            output: Price(1_000_000),
            ..table(Currency::Usd)
        };
        assert_eq!(cost(&u, &p, AT).output, Nano(800_000)); // 800 × 1_000_000/1000
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
