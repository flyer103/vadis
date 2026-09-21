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
///
/// The no-mix guarantee is enforced by the compiler and guarded by
/// compile-fail doctests: `+` and `sum()` on mixed currencies do not
/// compile (CONF-52's type-level half; ADR-018 §2).
///
/// ```compile_fail
/// // CONF-52 (ADR-018 §2): mixed arithmetic does not compile — there
/// // is no `impl Add<Money> for Money`, so `+` is a type error.
/// let usd = router_core::Money { nano: router_core::Nano(1), currency: router_core::Currency::Usd };
/// let cny = router_core::Money { nano: router_core::Nano(2), currency: router_core::Currency::Cny };
/// let _ = usd + cny; // E0369: cannot add `Money` to `Money`
/// ```
///
/// ```compile_fail
/// // CONF-52 (ADR-018 §2): aggregation is spelled as a per-currency
/// // map, never a `Sum` — `sum()` over mixed currencies does not
/// // compile (no `impl Sum<Money> for Money` exists at all).
/// let amounts = vec![
///     router_core::Money { nano: router_core::Nano(1), currency: router_core::Currency::Usd },
///     router_core::Money { nano: router_core::Nano(2), currency: router_core::Currency::Cny },
/// ];
/// let _total: router_core::Money = amounts.into_iter().sum(); // no Sum impl
/// ```
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

/// One band of a loaded price block (spec §4.10 / DESIGN §12.13): the
/// band's integerised [`PriceTable`] and its inclusive ceiling. `up_to` is
/// present on every band but the last; a loaded list is ascending and ends
/// with the unceiled band (the loader refuses anything else, so the
/// selection below never has to re-check those invariants).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TierTable {
    /// Inclusive upper bound on `usage.input_total` (tokens); `None` on
    /// the last band only — "no ceiling" (spec §4.10 rule 2).
    pub up_to: Option<u64>,
    pub table: PriceTable,
}

/// spec §4.10 rule 2, spelled once: the first band whose `up_to` is `None`
/// or `>= input_tokens`. Pure, O(bands ≤ 8), no clock, no estimate, no
/// config read. `n = 0` and `n = up_to` both land on the first band that
/// can hold them (§4.10's boundary table). The comparison is `<=`, so no
/// arithmetic runs on the boundary and no overflow is possible.
pub fn select_band(tiers: &[TierTable], input_tokens: u64) -> &PriceTable {
    for tier in tiers {
        match tier.up_to {
            Some(up_to) if input_tokens > up_to => continue,
            _ => return &tier.table,
        }
    }
    // Unreachable through the loader: a loaded block has 1..=8 bands and
    // its last band is unceiled, so the loop always returns.
    &tiers
        .last()
        .expect("a price block has at least one band")
        .table
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

    // -------------------------------------------------------------
    // CONF-52's runtime half (ADR-018 §2): the type-level no-mix is
    // proven by the compile_fail doctests on `Money`; these assert the
    // one adder that does exist and the shape aggregation must take.
    // -------------------------------------------------------------

    #[test]
    fn money_checked_add_refuses_a_mismatch_and_names_both_currencies() {
        let usd = Money {
            nano: Nano(1),
            currency: Currency::Usd,
        };
        let cny = Money {
            nano: Nano(2),
            currency: Currency::Cny,
        };
        // The one adder: a mismatch is an error naming BOTH operands'
        // currencies — never a value, never a silent conversion.
        let (left, right) = usd.checked_add(cny).expect_err("USD + CNY is refused");
        assert_eq!(
            (left, right),
            (
                CurrencyMismatch::Left(Currency::Usd),
                CurrencyMismatch::Right(Currency::Cny)
            )
        );
        // Both orders refuse (the error is not directional).
        assert!(cny.checked_add(usd).is_err());
        // Same-currency adds are fine and saturate (ADR-006 unchanged).
        assert_eq!(
            usd.checked_add(Money {
                nano: Nano(u64::MAX),
                currency: Currency::Usd
            })
            .unwrap()
            .nano,
            Nano(u64::MAX)
        );
    }

    #[test]
    fn aggregation_is_a_per_currency_map_never_a_bare_sum() {
        // ADR-018 §2's spelling for the one legal aggregate: a
        // BTreeMap<Currency, Money>. Asserted as the running example —
        // each currency's line sums only its own amounts, and the two
        // lines coexist without any combined total existing to read.
        let amounts = [
            Money {
                nano: Nano(1),
                currency: Currency::Usd,
            },
            Money {
                nano: Nano(2),
                currency: Currency::Usd,
            },
            Money {
                nano: Nano(5),
                currency: Currency::Cny,
            },
        ];
        let mut per_currency: std::collections::BTreeMap<Currency, Money> =
            std::collections::BTreeMap::new();
        for m in amounts {
            let slot = per_currency.entry(m.currency).or_insert(Money {
                nano: Nano::ZERO,
                currency: m.currency,
            });
            *slot = slot.checked_add(m).expect("same currency by construction");
        }
        assert_eq!(per_currency[&Currency::Usd].nano, Nano(3));
        assert_eq!(per_currency[&Currency::Cny].nano, Nano(5));
        assert_eq!(per_currency.len(), 2, "two currencies, two lines, no total");
    }

    #[test]
    fn currency_codes_round_trip_exactly_and_refuse_everything_else() {
        assert_eq!(Currency::Usd.as_code(), "USD");
        assert_eq!(Currency::Cny.as_code(), "CNY");
        assert_eq!(Currency::from_code("USD"), Some(Currency::Usd));
        assert_eq!(Currency::from_code("CNY"), Some(Currency::Cny));
        // The code is exact — case, whitespace, other ISO codes.
        for bad in ["usd", "Usd", "cny", " CNY", "CNY ", "EUR", "CNH", "", "us"] {
            assert_eq!(Currency::from_code(bad), None, "{bad:?} must not parse");
        }
        // Serialize says the same code (the trace / /health / report
        // spelling, §4.8).
        assert_eq!(serde_json::to_value(Currency::Cny).unwrap(), "CNY");
        assert_eq!(serde_json::to_value(Currency::Usd).unwrap(), "USD");
    }

    // -----------------------------------------------------------------
    // spec §4.10's selection-semantics boundary table, row by row, plus
    // peak orthogonality (rule 3) and the integer edges.
    // -----------------------------------------------------------------

    fn tier(up_to: Option<u64>, scale: u64) -> TierTable {
        TierTable {
            up_to,
            table: PriceTable {
                currency: Currency::Usd,
                input_miss: Price(150_000 * scale),
                input_hit: Price(30_000 * scale),
                cache_write: Price(0),
                output: Price(500_000 * scale),
                peak: flat_peak(),
            },
        }
    }

    /// Asserts WHICH band's prices priced the request — by comparing the
    /// produced breakdown against a hand-computed one for the expected
    /// band's `scale` (DESIGN §12.13 test 1), not merely that a table was
    /// returned.
    fn assert_band(tiers: &[TierTable], n: u64, expected_scale: u64) {
        fn sat(nano: u128) -> u64 {
            if nano > u64::MAX as u128 {
                u64::MAX
            } else {
                nano as u64
            }
        }
        let table = select_band(tiers, n);
        assert_eq!(
            table.input_miss,
            Price(150_000 * expected_scale),
            "n = {n}: expected the x{expected_scale} band"
        );
        // The money agrees with a hand-computed breakdown for that band
        // (saturating, like the engine itself at the top of the range).
        let u = Usage {
            input_total: n,
            input_cached: n / 2,
            cache_write: 0,
            output: 1_000,
            reasoning: 0,
        };
        let c = cost(&u, table, AT);
        let miss = (n - n / 2) as u128 * (150_000u64 * expected_scale) as u128 / 1000;
        let hit = (n / 2) as u128 * (30_000u64 * expected_scale) as u128 / 1000;
        let out = 1_000u128 * (500_000u64 * expected_scale) as u128 / 1000;
        assert_eq!(c.input_miss.0, sat(miss), "n = {n}: input_miss");
        assert_eq!(c.input_hit.0, sat(hit), "n = {n}: input_hit");
        assert_eq!(c.output.0, sat(out), "n = {n}: output");
        assert_eq!(
            c.total.0,
            sat(miss + hit + out),
            "n = {n}: total (no peak window)"
        );
    }

    #[test]
    fn boundary_table_flat_shape_rows() {
        // `price` flat | n ∈ {0, 200000, 10000000} → the single band.
        let flat = vec![tier(None, 1)];
        for n in [0u64, 200_000, 10_000_000] {
            assert_band(&flat, n, 1);
        }
    }

    #[test]
    fn boundary_table_two_band_rows() {
        // tiers: [{up_to: 200000}, {no ceiling}]
        let two = vec![tier(Some(200_000), 1), tier(None, 2)];
        assert_band(&two, 0, 1); // the first band starts at 0
        assert_band(&two, 200_000, 1); // the boundary belongs to the declaring band
        assert_band(&two, 200_001, 2); // one token above the ceiling
        assert_band(&two, 10_000_000, 2); // beyond every ceiling ⇒ the unceiled band
    }

    #[test]
    fn boundary_table_three_band_rows() {
        // tiers: [{200000}, {1000000}, {no ceiling}]
        let three = vec![
            tier(Some(200_000), 1),
            tier(Some(1_000_000), 2),
            tier(None, 3),
        ];
        assert_band(&three, 200_000, 1); // boundary, inclusive
        assert_band(&three, 200_001, 2);
        assert_band(&three, 1_000_000, 2); // boundary, inclusive
        assert_band(&three, 1_000_001, 3);
    }

    #[test]
    fn peak_windows_and_bands_are_orthogonal() {
        // Rule 3: the same n selects the same band inside and outside a
        // peak window; the total differs by exactly multiplier_pct, and
        // changing the band at a fixed instant does not change
        // peak_applied_pct.
        let mut two = vec![tier(Some(200_000), 1), tier(None, 2)];
        two[0].table.peak = always_peak_200();
        two[1].table.peak = always_peak_200();
        let n = 5_000u64;
        let u = Usage {
            input_total: n,
            input_cached: 0,
            cache_write: 0,
            output: 1_000,
            reasoning: 0,
        };
        let inside = cost(&u, select_band(&two, n), AT);
        assert_eq!(inside.peak_applied_pct, 200, "the window applied");
        // The same request with no window: same band (checked by money),
        // total differs by exactly the multiplier.
        let flat = vec![tier(Some(200_000), 1), tier(None, 2)];
        let outside = cost(&u, select_band(&flat, n), AT);
        assert_eq!(outside.peak_applied_pct, 100);
        assert_eq!(
            inside.input_miss, outside.input_miss,
            "same band, same buckets"
        );
        assert_eq!(
            inside.total.0 as u128,
            outside.total.0 as u128 * 200 / 100,
            "the window multiplies the band's sum once"
        );
        // A different n (the other band) at the same instant still applies
        // the same pct — the multiplier is never re-read per band.
        let n2 = 300_000u64;
        let u2 = Usage {
            input_total: n2,
            input_cached: 0,
            cache_write: 0,
            output: 1_000,
            reasoning: 0,
        };
        let other_band = cost(&u2, select_band(&two, n2), AT);
        assert_eq!(other_band.peak_applied_pct, 200, "the pct is per entry");
        // miss = 300_000 × (150_000×2)/1000 = 90_000_000 nano.
        assert_eq!(
            other_band.input_miss,
            Nano(90_000_000),
            "the x2 band priced it"
        );
    }

    #[test]
    fn integer_edges_select_the_last_band_without_overflow() {
        // An up_to of u64::MAX-adjacent magnitude and an n at the top of
        // the range select the last band; the comparison is `<=`, so no
        // arithmetic runs on the boundary at all.
        let tiers = vec![tier(Some(u64::MAX - 1), 1), tier(None, 2)];
        assert_band(&tiers, u64::MAX - 1, 1);
        assert_band(&tiers, u64::MAX, 2);
        // And a ceiling exactly at u64::MAX on a one-ceilless list.
        let two = vec![tier(Some(u64::MAX), 1), tier(None, 2)];
        assert_band(&two, u64::MAX, 1);
    }
}
