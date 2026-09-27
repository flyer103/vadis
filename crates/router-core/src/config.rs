//! Spec §4 config types (the contract of `config.example.yaml`) and
//! load-time validation (DESIGN §12.5 / §12.10.2).
//!
//! Pure data + checks: turning bytes into [`RouterConfig`] (YAML) happens in
//! `router-cli`; every §12.5 scalar convention — duration, context size,
//! USD/1K → nano-USD price conversion, peak-multiplier precision — is
//! enforced by the `Deserialize` impls and [`RouterConfig::validate`] here,
//! so the rules have exactly one home regardless of which loader feeds the
//! types.
//!
//! Money rule (ADR-006): floats exist only at the load boundary, inside the
//! single conversion functions below, each gated on the result being exactly
//! representable; everything downstream is integer `Price`/`Nano`.

use std::collections::BTreeMap;
use std::fmt;
use std::net::SocketAddr;

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize};

use crate::cost::{Currency, Nano, Price, PriceTable, TierTable};
use crate::peak::{PeakTable, PeakWindow, Tz, Weekdays};

/// A load-time rejection with the exact key it belongs to (§12.10.2: each
/// failure names the config path and the reason — no silent defaults).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    /// Dotted path of the offending key, e.g. `providers[1].quota[0].models`.
    pub path: String,
    /// Why it is rejected, in one sentence.
    pub reason: String,
}

impl ConfigError {
    fn new(path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            reason: reason.into(),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "config error at `{}`: {}", self.path, self.reason)
    }
}

impl std::error::Error for ConfigError {}

// ---------------------------------------------------------------------------
// §12.5 scalar conventions
// ---------------------------------------------------------------------------

fn parse_duration(s: &str) -> Result<u64, String> {
    let bad = || {
        format!("invalid duration '{s}': expected stackable <integer><ms|s|m|h> segments (e.g. '90s', '1h30m'); bare numbers are not accepted")
    };
    let mut total: u64 = 0;
    let mut num: u64 = 0;
    let mut digits = 0usize;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c.is_ascii_digit() {
            num = num
                .checked_mul(10)
                .and_then(|n| n.checked_add(u64::from(c as u8 - b'0')))
                .ok_or_else(|| format!("invalid duration '{s}': the number is too large"))?;
            digits += 1;
            continue;
        }
        if !c.is_ascii_alphabetic() || digits == 0 {
            return Err(bad());
        }
        // Collect the whole run of unit letters, then match it as one unit.
        let start = i;
        let mut end = i + c.len_utf8();
        while let Some(&(j, cj)) = chars.peek() {
            if cj.is_ascii_alphabetic() {
                end = j + cj.len_utf8();
                chars.next();
            } else {
                break;
            }
        }
        let mult: u64 = match &s[start..end] {
            "ms" => 1,
            "s" => 1_000,
            "m" => 60_000,
            "h" => 3_600_000,
            unit => {
                return Err(format!(
                    "invalid duration '{s}': unknown unit '{unit}' (expected ms, s, m or h)"
                ))
            }
        };
        total = total
            .checked_add(
                num.checked_mul(mult)
                    .ok_or_else(|| format!("invalid duration '{s}': the number is too large"))?,
            )
            .ok_or_else(|| format!("invalid duration '{s}': the total overflows"))?;
        num = 0;
        digits = 0;
    }
    if s.is_empty() || digits > 0 {
        // digits > 0 means a trailing number with no unit (e.g. "1h30").
        return Err(bad());
    }
    Ok(total)
}

fn parse_context(s: &str) -> Result<u64, String> {
    let bad = || {
        format!("invalid context '{s}': expected <integer> or <integer>k|m (k=1024, m=1024x1024)")
    };
    let (num_str, mult): (&str, u64) = if let Some(v) = s.strip_suffix('k') {
        (v, 1024)
    } else if let Some(v) = s.strip_suffix('m') {
        (v, 1024 * 1024)
    } else {
        (s, 1)
    };
    if num_str.is_empty() {
        return Err(bad());
    }
    let n: u64 = num_str.parse().map_err(|_| bad())?;
    n.checked_mul(mult)
        .ok_or_else(|| format!("invalid context '{s}': the number is too large"))
}

fn parse_tokens(s: &str) -> Result<u64, String> {
    s.replace('_', "").parse().map_err(|_| {
        format!("invalid token count '{s}': expected an integer (underscores allowed)")
    })
}

fn parse_route(s: &str) -> Result<RouteSpec, String> {
    let parts: Vec<&str> = s.split('/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(format!(
            "invalid route '{s}': expected '<provider>/<model>' with both parts non-empty"
        ));
    }
    Ok(RouteSpec {
        provider: parts[0].to_string(),
        model: parts[1].to_string(),
    })
}

fn parse_hhmm(s: &str) -> Result<u16, String> {
    let (hh, mm) = s
        .split_once(':')
        .ok_or_else(|| format!("invalid time '{s}': expected HH:MM (e.g. '01:00')"))?;
    let bad = || {
        format!("invalid time '{s}': expected HH:MM with HH in 00..=24 and MM in 00..=59 ('24:00' is end of day)")
    };
    let hh: u16 = hh.parse().map_err(|_| bad())?;
    let mm: u16 = mm.parse().map_err(|_| bad())?;
    if hh > 24 || mm > 59 || (hh == 24 && mm != 0) {
        return Err(bad());
    }
    Ok(hh * 60 + mm)
}

fn parse_tz(s: &str) -> Result<Tz, String> {
    if s.eq_ignore_ascii_case("utc") || s.eq_ignore_ascii_case("z") {
        return Ok(Tz::Utc);
    }
    let (sign, rest) = if let Some(r) = s.strip_prefix('+') {
        (1i32, r)
    } else if let Some(r) = s.strip_prefix('-') {
        (-1i32, r)
    } else {
        return Err(format!(
            "invalid tz '{s}': expected 'UTC' or a numeric offset like '+08:00' / '-10:00'"
        ));
    };
    let mins = parse_hhmm(rest).map_err(|_| {
        format!("invalid tz '{s}': expected 'UTC' or a numeric offset like '+08:00' / '-10:00'")
    })?;
    if mins / 60 > 23 {
        return Err(format!("invalid tz '{s}': the offset hour is out of range"));
    }
    Ok(Tz::OffsetMin(sign * mins as i32))
}

/// Monday=bit1 … Sunday=bit0, Saturday=bit6 (the `Weekdays` convention).
fn parse_weekday(s: &str) -> Result<u8, String> {
    let bit = match s.to_ascii_lowercase().as_str() {
        "sun" => 1 << 0,
        "mon" => 1 << 1,
        "tue" => 1 << 2,
        "wed" => 1 << 3,
        "thu" => 1 << 4,
        "fri" => 1 << 5,
        "sat" => 1 << 6,
        _ => {
            return Err(format!(
                "invalid weekday '{s}': expected a three-letter day (Mon, Tue, Wed, Thu, Fri, Sat, Sun)"
            ))
        }
    };
    Ok(bit)
}

// ---------------------------------------------------------------------------
// Scalar newtypes (the §12.5 grammar, as serde types)
// ---------------------------------------------------------------------------

/// Duration in milliseconds (`60s`, `1h30m`, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DurationVal(pub u64);

/// Context limit in tokens (`131072`, `200k`, `1m` = 1,048,576).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ContextVal(pub u64);

/// Token count; accepts underscores (`100_000_000`) because YAML does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TokensVal(pub u64);

/// `provider/model`, the one route form used by `aliases` and `fallback`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RouteSpec {
    pub provider: String,
    pub model: String,
}

impl fmt::Display for RouteSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.provider, self.model)
    }
}

/// A price scalar as written (USD / 1K tokens); converted to integer
/// [`Price`] only through [`PriceCfg::to_tier_tables`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PriceVal(pub f64);

/// A band ceiling as written (`tiers[k].up_to`, spec §4.10 rule 2): a
/// plain integer number of tokens — **no** `k`/`m` suffix, unlike
/// `context`, because a ceiling is compared against a *measured* token
/// count and a reader must never guess which of 1000 or 1024 a page's "K"
/// meant. The as-written scalar is kept; the positive-integer refusal
/// (zero, negative, fractional) runs in [`PriceCfg::to_tier_tables`] so it
/// can name the band's index and the value found (DESIGN §12.13).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CeilingVal(pub f64);

impl<'de> Deserialize<'de> for CeilingVal {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(NumVisitor {
            expecting: "a plain integer token count like 200000 (no k/m suffix)",
            wrap: CeilingVal,
        })
    }
}

/// The band-count cap of spec §4.10 rule 6: no vendor publishes more than
/// three bands, and a hand-written block stays reviewable at eight.
pub const MAX_PRICE_BANDS: usize = 8;

/// A peak/breakeven multiplier scalar (2.0 → 200 pct).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct MultiplierVal(pub f64);

/// `HH:MM` minutes of day (`24:00` allowed as end of day).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct MinutesVal(pub u16);

/// A weekday bitmask built from a `days:` list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct WeekdaySet(pub u8);

/// A time zone (`UTC` or `+08:00`-style).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TzVal(pub Tz);

/// A single weekday (parsed to its `Weekdays` bit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WeekdayVal(u8);

// A string-only scalar.
struct StrVisitor<T> {
    expecting: &'static str,
    parse: fn(&str) -> Result<T, String>,
}

impl<'de, T> Visitor<'de> for StrVisitor<T> {
    type Value = T;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.expecting)
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<T, E> {
        (self.parse)(v).map_err(E::custom)
    }
}

// A scalar that is either a bare integer or a string (context: `131072` or
// `200k`; tokens: `1000000` or `100_000_000`).
struct NumStrVisitor<T> {
    expecting: &'static str,
    parse: fn(&str) -> Result<T, String>,
    direct: fn(u64) -> T,
}

impl<'de, T> Visitor<'de> for NumStrVisitor<T> {
    type Value = T;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.expecting)
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<T, E> {
        (self.parse)(v).map_err(E::custom)
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<T, E> {
        Ok((self.direct)(v))
    }
}

// A numeric scalar that may arrive as a float, an int, or a quoted string
// (prices and multipliers).
struct NumVisitor<T> {
    expecting: &'static str,
    wrap: fn(f64) -> T,
}

impl<'de, T> Visitor<'de> for NumVisitor<T> {
    type Value = T;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.expecting)
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<T, E> {
        Ok((self.wrap)(v))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<T, E> {
        Ok((self.wrap)(v as f64))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<T, E> {
        Ok((self.wrap)(v as f64))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<T, E> {
        let n: f64 = v
            .parse()
            .map_err(|_| de::Error::invalid_value(de::Unexpected::Str(v), &self))?;
        Ok((self.wrap)(n))
    }

    // serde_json's `arbitrary_precision` feature (a workspace-wide setting
    // router-core inherits) routes numbers through `deserialize_any` as a
    // private single-entry map { "$serde_json::private::Number": "literal" }
    // instead of visit_f64. Handle that representation so the same type
    // works under both serde_json and serde_yaml.
    fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<T, A::Error> {
        let entry: Option<(String, String)> = map.next_entry()?;
        if map.next_entry::<String, String>()?.is_some() {
            return Err(de::Error::custom(
                "expected a number (USD-per-1K price or multiplier)",
            ));
        }
        match entry {
            Some((tag, literal)) if tag.starts_with("$serde_json::private") => {
                let n: f64 = literal
                    .parse()
                    .map_err(|_| de::Error::custom(format!("not a number: '{literal}'")))?;
                Ok((self.wrap)(n))
            }
            _ => Err(de::Error::custom(
                "expected a number (USD-per-1K price or multiplier)",
            )),
        }
    }
}

macro_rules! de_str {
    ($ty:ty, $expecting:literal, $parse:expr) => {
        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                d.deserialize_str(StrVisitor {
                    expecting: $expecting,
                    parse: $parse,
                })
            }
        }
    };
}

de_str!(
    DurationVal,
    "a duration like '60s' or '1h30m'",
    |s: &str| parse_duration(s).map(DurationVal)
);
de_str!(RouteSpec, "a '<provider>/<model>' route", |s: &str| {
    parse_route(s)
});

impl<'de> Deserialize<'de> for ContextVal {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(NumStrVisitor {
            expecting: "a context size like '131072', '200k' or '1m'",
            parse: |s: &str| parse_context(s).map(ContextVal),
            direct: ContextVal,
        })
    }
}

impl<'de> Deserialize<'de> for TokensVal {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(NumStrVisitor {
            expecting: "a token count (underscores allowed)",
            parse: |s: &str| parse_tokens(s).map(TokensVal),
            direct: TokensVal,
        })
    }
}

impl<'de> Deserialize<'de> for PriceVal {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(NumVisitor {
            expecting: "a USD-per-1K-tokens price like 0.00066",
            wrap: PriceVal,
        })
    }
}

impl<'de> Deserialize<'de> for MultiplierVal {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(NumVisitor {
            expecting: "a multiplier like 2.0",
            wrap: MultiplierVal,
        })
    }
}

/// A USD amount as written (`overflow_monthly_cap_usd`), converted to integer
/// [`Nano`] only through [`CapUsdVal::to_nano`] — the same single load-time
/// rounding `price` uses (ADR-006: integers everywhere downstream of this
/// boundary).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CapUsdVal(pub f64);

impl CapUsdVal {
    /// The one conversion point. A negative or non-finite amount is refused
    /// rather than clamped: an unreadable guardrail must not silently become
    /// "no guardrail" or "refuse everything" (spec §4.6, DESIGN §12.5).
    #[allow(clippy::float_arithmetic)]
    pub fn to_nano(self) -> Result<Nano, String> {
        let v = self.0;
        if !v.is_finite() || v < 0.0 {
            return Err(format!(
                "the cap must be a finite USD amount >= 0 (got {v}); an absent key means no cap"
            ));
        }
        Ok(Nano((v * 1e9).round() as u64))
    }
}

impl<'de> Deserialize<'de> for CapUsdVal {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(NumVisitor {
            expecting: "a USD amount like 20.0",
            wrap: CapUsdVal,
        })
    }
}

de_str!(MinutesVal, "a HH:MM time like '01:00'", |s: &str| {
    parse_hhmm(s).map(MinutesVal)
});
de_str!(
    TzVal,
    "a time zone: 'UTC' or an offset like '+08:00'",
    |s: &str| parse_tz(s).map(TzVal)
);

impl<'de> Deserialize<'de> for WeekdayVal {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_str(StrVisitor {
            expecting: "a three-letter weekday like 'Mon'",
            parse: |s: &str| parse_weekday(s).map(WeekdayVal),
        })
    }
}

impl<'de> Deserialize<'de> for WeekdaySet {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = WeekdaySet;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a list of three-letter weekdays like [Mon, Tue, Fri]")
            }
            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<WeekdaySet, A::Error> {
                let mut bits = 0u8;
                while let Some(day) = seq.next_element::<WeekdayVal>()? {
                    bits |= day.0;
                }
                Ok(WeekdaySet(bits))
            }
        }
        d.deserialize_seq(V)
    }
}

// ---------------------------------------------------------------------------
// The spec §4 tree
// ---------------------------------------------------------------------------

/// The wire protocols of the 3×3 matrix (spec §4 `wire_api` / `supports`).
///
/// `Ord` is what lets `urls` be a `BTreeMap`: diagnostics and any serialized
/// view list the wires in one fixed order instead of an iteration order that
/// changes between runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum WireApi {
    Chat,
    Responses,
    Anthropic,
}

impl WireApi {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Responses => "responses",
            Self::Anthropic => "anthropic",
        }
    }
}

impl fmt::Display for WireApi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for WireApi {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        match s.as_str() {
            "chat" => Ok(Self::Chat),
            "responses" => Ok(Self::Responses),
            "anthropic" => Ok(Self::Anthropic),
            other => Err(de::Error::custom(format!(
                "unknown wire protocol '{other}' (expected chat, responses or anthropic)"
            ))),
        }
    }
}

/// v0.1 defines exactly one rollover value (spec §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Rollover {
    Hourly,
}

impl<'de> Deserialize<'de> for Rollover {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        match s.as_str() {
            "hourly" => Ok(Self::Hourly),
            other => Err(de::Error::custom(format!(
                "unknown rollover '{other}': v0.1 defines only 'hourly' (file name YYYY-MM-DDTHH.jsonl, spec §4.1)"
            ))),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_safety_factor() -> MultiplierVal {
    MultiplierVal(1.2)
}

/// Spec §4.13: the default inbound request-body limit, 2 MiB — the
/// declared form of the cap this build has always had in effect (the
/// HTTP layer's implicit one). Same value, now the router's own.
fn default_max_body_bytes() -> i64 {
    2_097_152
}

/// The smallest bound the key accepts (spec §4.13): a value below 1024
/// — including 0 and negatives — cannot bound anything and is a load
/// error, not a quiet default.
pub const MIN_MAX_BODY_BYTES: i64 = 1024;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServerCfg {
    pub addr: String,
    pub upstream_attempt_timeout: DurationVal,
    pub request_timeout: DurationVal,
    /// Spec §4.13: the largest inbound request body this process will
    /// **read** on the three protocol endpoints, an integer count of
    /// bytes (no unit suffixes). A body above it is refused at the
    /// boundary with §8's `413 request_too_large`, before the pipeline.
    /// Held as `i64` so a negative value reaches [`RouterConfig::validate`]'s
    /// own refusal (which names the key and the value) instead of a
    /// generic serde type error; converted to `usize` once, after
    /// validation, at the one consumer.
    #[serde(default = "default_max_body_bytes")]
    pub max_body_bytes: i64,
    /// Spec §4.7: the **name** of the env var whose value this process
    /// expects as the inbound token. Absent ⇒ no inbound auth (today's
    /// behaviour; the backward-compatibility clause). A plain string the
    /// parser carries through — the startup refusal on a missing/empty
    /// value is router-cli's (§12.10.2's split: router-core never reads
    /// the environment), never this parser's.
    #[serde(default)]
    pub auth_token_env: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCfg {
    pub key_sources: Vec<String>,
    pub ttl: DurationVal,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BreakevenCfg {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub min_remaining_turns: u32,
    #[serde(default = "default_safety_factor")]
    pub safety_factor: MultiplierVal,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CacheCfg {
    #[serde(default = "default_true")]
    pub sticky: bool,
    pub breakeven: BreakevenCfg,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TraceCfg {
    pub dir: String,
    pub rollover: Rollover,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PeakCfg {
    pub multiplier: MultiplierVal,
    #[serde(default)]
    pub windows: Vec<WindowCfg>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WindowCfg {
    pub days: WeekdaySet,
    pub from: MinutesVal,
    pub to: MinutesVal,
    pub tz: TzVal,
}

impl WindowCfg {
    pub fn to_peak_window(&self) -> PeakWindow {
        PeakWindow {
            days: Weekdays(self.days.0),
            from_min: self.from.0,
            to_min: self.to.0,
            tz: self.tz.0,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PriceCfg {
    #[serde(default)]
    pub input_miss: Option<PriceVal>,
    #[serde(default)]
    pub input_hit: Option<PriceVal>,
    #[serde(default)]
    pub cache_write: Option<PriceVal>,
    #[serde(default)]
    pub output: Option<PriceVal>,
    pub peak: PeakCfg,
    /// The banded shape (spec §4.10 rule 1): `price` carries either the
    /// four flat scalars above or `tiers` — never both, never neither.
    /// `Option` + a cross-field refusal (not an untagged enum) so the load
    /// error can name both keys and the reason (DESIGN §12.13).
    #[serde(default)]
    pub tiers: Option<Vec<TierCfg>>,
}

/// One band of a banded price block (spec §4.10). The four prices are the
/// same [`PriceVal`] scalars the flat shape uses, through the same
/// load-time conversion; `up_to` is the band's **inclusive** ceiling
/// (tokens, a plain integer), `None` on the last band only ("no ceiling",
/// rule 2). A band carries no `peak` and no currency: the multiplier and
/// the unit belong to the entry (rule 3, §4.8) — `deny_unknown_fields`
/// makes a `peak` inside a band a load refusal naming the key.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TierCfg {
    #[serde(default)]
    pub up_to: Option<CeilingVal>,
    #[serde(default)]
    pub input_miss: Option<PriceVal>,
    #[serde(default)]
    pub input_hit: Option<PriceVal>,
    #[serde(default)]
    pub cache_write: Option<PriceVal>,
    #[serde(default)]
    pub output: Option<PriceVal>,
}

/// spec §4.8 (ADR-018): the regional deployment a provider entry's
/// endpoint and key belong to. `cn` | `intl`, **absent ⇒ `intl`**. It is
/// declared, displayed and inert: it routes nothing, it chooses no
/// currency (a CN-region entry billed in USD is legal), and the router
/// does not check it against a host in `urls` (ADR-018, ADR-020).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Region {
    Cn,
    #[default]
    Intl,
}

impl Region {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cn => "cn",
            Self::Intl => "intl",
        }
    }
}

impl fmt::Display for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `models[i].family` (spec §4.8): the family tag pairing routes whose
/// provider-native ids differ. **Absent ⇒ the model's own `id`** (which
/// is ADR-014's rule, so every pre-§4.8 config keeps its meaning);
/// non-empty when written. The tag is a name, not an address: nothing
/// resolves a client's string to it, and `plan_policy.family` is the
/// only consumer.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FamilyTag(pub String);

impl FamilyTag {
    /// The tag a model entry carries: its own written tag, or its id when
    /// none was written (§4.8's default — the one place the fallback
    /// lives).
    pub fn or_default(id: &str) -> Self {
        Self(id.to_string())
    }
}

impl fmt::Display for FamilyTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelCfg {
    pub id: String,
    pub context: ContextVal,
    /// spec §4.8: optional non-empty string; absent ⇒ the model's own
    /// `id`. At most one model entry per provider entry may carry a
    /// given tag (a duplicate is a load error naming
    /// `providers[i].models[j].family`).
    #[serde(default)]
    pub family: Option<String>,
    pub price: PriceCfg,
    pub source: String,
}

/// `window: monthly` is the only window kind (DESIGN §12.4: any other value
/// is a load error).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum QuotaWindowTag {
    Monthly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OverQuotaTag {
    Block,
    Spill,
}

/// `reset_day`: 1..=31 (short months clamp at use time, `quota.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ResetDayVal(pub u8);

impl<'de> Deserialize<'de> for ResetDayVal {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = u64::deserialize(d)?;
        if !(1..=31).contains(&v) {
            return Err(de::Error::custom(format!(
                "invalid reset_day {v}: expected a day of month in 1..=31"
            )));
        }
        Ok(ResetDayVal(v as u8))
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QuotaCfg {
    pub models: Vec<String>,
    pub window: QuotaWindowTag,
    pub tokens: TokensVal,
    pub reset_day: ResetDayVal,
    pub over_quota: OverQuotaTag,
    pub source: String,
}

/// spec §4.6 `account`: what a provider entry **is** — the account its key, its
/// endpoint and its allowance belong to. Absent means [`AccountKind::Api`]: the
/// metered account is the ordinary case, and a roster that writes nothing means
/// exactly what today's roster means (ADR-014 item 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub enum AccountKind {
    CodingPlan,
    #[default]
    Api,
}

impl AccountKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CodingPlan => "coding_plan",
            Self::Api => "api",
        }
    }
}

impl fmt::Display for AccountKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for AccountKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        match s.as_str() {
            "coding_plan" => Ok(Self::CodingPlan),
            "api" => Ok(Self::Api),
            other => Err(de::Error::custom(format!(
                "unknown account '{other}' (spec §4.6: expected coding_plan or api; an absent \
                 key means api)"
            ))),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCfg {
    pub name: String,
    /// spec §4.8; absent ⇒ [`Region::Intl`]. Display/audit only — it
    /// routes nothing and it does not choose a currency.
    #[serde(default)]
    pub region: Region,
    /// spec §4.8; absent ⇒ [`Currency::Usd`]. The unit of every `price`
    /// tier in this entry's model table, and of nothing else. Never
    /// derived from `region`; never converted.
    #[serde(default)]
    pub currency: Currency,
    /// spec §4.9 (ADR-020): the **complete** URL for every wire protocol this
    /// entry declares in `supports` below. The router sends these values
    /// verbatim — it appends no path, trims no slash and normalizes nothing.
    /// The key set must equal the declared cells ([`RouterConfig::validate`]).
    pub urls: BTreeMap<WireApi, String>,
    pub api_key_env: String,
    pub wire_api: WireApi,
    pub supports: Vec<WireApi>,
    /// spec §4.6; absent ⇒ [`AccountKind::Api`].
    #[serde(default)]
    pub account: AccountKind,
    pub models: Vec<ModelCfg>,
    #[serde(default)]
    pub quota: Option<Vec<QuotaCfg>>,
}

impl ProviderCfg {
    /// The URL this entry POSTs to for `wire` (spec §4.9). `None` means the
    /// entry does not declare that cell at all, which
    /// [`RouterConfig::validate`] refuses at load — so a `None` here is a
    /// programming error at the call site, not a case to paper over with a
    /// fallback (ADR-020: there is no composition left to fall back to).
    pub fn url_for(&self, wire: WireApi) -> Option<&str> {
        self.urls.get(&wire).map(String::as_str)
    }
}

/// spec §4.6 `on_primary_exhausted`: what a family does once the upstream has
/// declared its subscription account exhausted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OnPrimaryExhausted {
    /// The family continues on the metered account at its real price.
    #[default]
    Spill,
    /// The request is refused with a readable reason (§8 `quota_exceeded`)
    /// instead of being served from the metered account.
    Block,
}

/// spec §4.6 `recover`: whether the family probes its `primary` again after the
/// cooldown (`probe`, the default), or never does (`none`: it returns at the
/// plan's own window boundary, or by an operator action).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RecoveryMode {
    #[default]
    Probe,
    None,
}

/// spec §4.6's `cooldown` default: a floor between the move away from
/// `primary` and the first admitted probe, not a schedule.
fn default_cooldown() -> DurationVal {
    DurationVal(15 * 60 * 1_000)
}

/// spec §4.6 `plan_policy`: one model family served by two accounts — the
/// subscription first, the metered account as the spill (ADR-014). Optional:
/// absent means no plan-first routing, and at most one policy exists in v0.1 (a
/// second family is an additive future key, never a reshaped section).
///
/// Only **syntax** is enforced here; the cross-field rules (which routes, which
/// accounts, which quota covers the family) are routing rules and live in
/// [`RouterConfig::validate`] (DESIGN §12.5 / §12.10.2).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlanPolicyCfg {
    /// The model id both routes carry — and the key of the family's state.
    pub family: String,
    /// The subscription route: a roster route whose provider is
    /// `account: coding_plan`.
    pub primary: RouteSpec,
    /// The metered route: a roster route, distinct from `primary`, whose
    /// provider is `account: api`.
    pub overflow: RouteSpec,
    #[serde(default)]
    pub on_primary_exhausted: OnPrimaryExhausted,
    #[serde(default)]
    pub recover: RecoveryMode,
    #[serde(default = "default_cooldown")]
    pub cooldown: DurationVal,
    /// Optional guardrail on the family's metered spend in a UTC calendar
    /// month; absent = no cap.
    #[serde(default)]
    pub overflow_monthly_cap_usd: Option<CapUsdVal>,
}

impl PlanPolicyCfg {
    /// The cooldown in unix **microseconds** — the single ms→µs conversion
    /// (ADR-016 §13.3 L1b). `DurationVal` is milliseconds (§12.5); the
    /// guard (`PlanFirstRule`), the `/health` probe deadline and the
    /// `plan.switched` projection all call this method, so a unit drift
    /// is impossible by construction. Saturates at `i64::MAX` (an absurd
    /// config simply never admits a probe again) — a plain `as` cast
    /// would wrap u64→i64 negative and admit every probe immediately,
    /// which is the R5-F3 failure shape in miniature.
    pub fn cooldown_us(&self) -> i64 {
        i64::try_from(self.cooldown.0)
            .unwrap_or(i64::MAX)
            .saturating_mul(1_000)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InterceptCfg {
    pub sample: f64,
    pub shadow: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginCfg {
    pub id: String,
    /// `builtin/<name>` (tier-A) or `process` (tier-B).
    pub kind: String,
    /// Free-form per-kind config; builtin kinds own their own schema.
    #[serde(default)]
    pub config: Option<serde_json::Value>,
    pub url: Option<String>,
    #[serde(default)]
    pub inject: Vec<String>,
    #[serde(default)]
    pub isolate: bool,
    pub intercept: Option<InterceptCfg>,
    #[serde(default)]
    pub disabled: bool,
}

/// `state:` is **not** a config key in v0.1 (spec §4.5): the store path is
/// fixed at `<config dir>/state/router.db`. The key is "known" to serde only
/// so that its presence produces this precise refusal instead of a generic
/// unknown-field message (DESIGN §12.10.2 load-time table).
#[derive(Debug, Clone, Copy)]
pub struct StateKeyForbidden;

impl<'de> Deserialize<'de> for StateKeyForbidden {
    fn deserialize<D>(_: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Err(de::Error::custom(
            "the `state:` section is not a config key in v0.1: the store path is fixed at \
             <config dir>/state/router.db (spec §4.5, ADR-009 item 6); a section that moves it \
             (the way trace.dir does) is an additive future key",
        ))
    }
}

/// The whole spec §4 file. Every section is required: hidden defaults are
/// the most expensive silent failure a hand-written config can have.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RouterConfig {
    pub server: ServerCfg,
    pub session: SessionCfg,
    pub cache: CacheCfg,
    pub trace: TraceCfg,
    pub providers: Vec<ProviderCfg>,
    pub aliases: BTreeMap<String, RouteSpec>,
    pub plugins: Vec<PluginCfg>,
    pub fallback: Vec<RouteSpec>,
    /// spec §4.6 / ADR-014: the plan-first policy. Absent ⇒ no plan-first
    /// routing (and a roster that declares `account: coding_plan` without a
    /// policy is a legal, ordinary entry).
    #[serde(default)]
    pub plan_policy: Option<PlanPolicyCfg>,
    // Never serializes: a present `state:` key refuses the load, so the
    // value is always None — and `StateKeyForbidden` deliberately has no
    // `Serialize` (its one job is to fail deserialization).
    #[serde(default, skip_serializing)]
    pub state: Option<StateKeyForbidden>,
}

// ---------------------------------------------------------------------------
// The roster as its own file (spec §4/§4.14; ADR-037 D1/D3/D4)
// ---------------------------------------------------------------------------

/// `providers` presence, exact (the R43 opener's 2026-09-25 ruling, STATE.md
/// "Opener rulings on R43-1b's two findings", limb (i)): the exactly-one-of
/// rule is about the key's **presence**, so a written `providers: null` is
/// refused *by `providers:`* — never collapsed into "neither written".
/// `deserialize_with` fires only when the key is present, so an absent key
/// still yields `None` while a present null fails here naming the key.
fn de_roster_key_presence<'de, D, T>(d: D, key: &str) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(d)
        .map(Some)
        .map_err(|e| de::Error::custom(format!("{key}: {e}")))
}

fn de_providers_presence<'de, D>(d: D) -> Result<Option<Vec<ProviderCfg>>, D::Error>
where
    D: Deserializer<'de>,
{
    de_roster_key_presence(d, "providers")
}

/// `providers_file` presence **and type**, exact (R43-2b F1, closed by
/// R43-4c): the key's value must be a non-empty string — a path. The
/// generic presence helper cannot see a YAML null for a `String` target:
/// serde_yaml's plain-scalar fast path hands `providers_file: null` to
/// `String::deserialize` as the *string* `"null"`, which the loader then
/// resolved as a relative path — and a file literally named `null` beside
/// the config silently loaded, a *search* where §4.14 requires a *named*
/// file. `deserialize_any` forces tag resolution, so a written null
/// arrives as `visit_unit` and is refused **by `providers_file:`** with
/// the type-shaped message, before any path resolution; an empty string
/// is refused the same way (an empty path is not a path). An explicitly
/// written string — including one spelling `"null"` — is a named path
/// and still loads.
fn de_providers_file_presence<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    struct RosterPath;

    impl<'de> Visitor<'de> for RosterPath {
        type Value = String;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a path (a non-empty string)")
        }

        fn visit_str<E>(self, v: &str) -> Result<String, E>
        where
            E: de::Error,
        {
            if v.is_empty() {
                Err(E::custom("expected a path, found an empty string"))
            } else {
                Ok(v.to_owned())
            }
        }

        fn visit_string<E>(self, v: String) -> Result<String, E>
        where
            E: de::Error,
        {
            self.visit_str(&v)
        }

        fn visit_unit<E>(self) -> Result<String, E>
        where
            E: de::Error,
        {
            Err(E::custom("expected a path, found null"))
        }
    }

    d.deserialize_any(RosterPath)
        .map(Some)
        .map_err(|e| de::Error::custom(format!("providers_file: {e}")))
}

/// The root file's own shape (spec §4/§4.14; ADR-037 D1/D4). Every section
/// of [`RouterConfig`] as it stands, with the roster carried as **exactly
/// one of two keys** — `providers:` inline or `providers_file:` naming the
/// roster file — so that presence is observable and the loader can refuse
/// both-written and neither-written *before* the join. Nothing but the
/// loader parses this: the serving path sees only the joined
/// [`RouterConfig`], whose `providers` field is always populated.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RootFile {
    pub server: ServerCfg,
    pub session: SessionCfg,
    pub cache: CacheCfg,
    pub trace: TraceCfg,
    /// The inline roster — one of the two shapes. Absent ⇒ `None`; a
    /// written `providers: []` is `Some(vec![])` (an empty roster is a
    /// decision, not a default); a written `providers: null` is a parse
    /// refusal naming `providers:` (see [`de_roster_key_presence`]).
    #[serde(default, deserialize_with = "de_providers_presence")]
    pub providers: Option<Vec<ProviderCfg>>,
    /// The roster's name — the other shape. Required when used, resolved
    /// by §4.1's rule (absolute wins, else against the config file's own
    /// directory, `~` not expanded); never discovered, never defaulted.
    /// A written `providers_file: null` (or an empty string) is a parse
    /// refusal naming `providers_file:` with the found type, before any
    /// path resolution — symmetric with `providers:` and enforced by
    /// [`de_providers_file_presence`], which exists because serde_yaml's
    /// plain-scalar fast path would otherwise coerce a written null into
    /// the string `"null"` and resolve it as a path (R43-2b F1).
    #[serde(default, deserialize_with = "de_providers_file_presence")]
    pub providers_file: Option<String>,
    pub aliases: BTreeMap<String, RouteSpec>,
    pub plugins: Vec<PluginCfg>,
    pub fallback: Vec<RouteSpec>,
    #[serde(default)]
    pub plan_policy: Option<PlanPolicyCfg>,
    // As on RouterConfig: never serialized (`StateKeyForbidden` has no
    // `Serialize`; a present key refuses the load).
    #[serde(default, skip_serializing)]
    pub state: Option<StateKeyForbidden>,
}

/// The roster file's whole shape (spec §4.14; ADR-037 D3): exactly one
/// top-level key holding the same block the inline form carries, under the
/// same `deny_unknown_fields` strictness. It is **not** a config file — no
/// `server`, no `plugins`, never discovered, never merged — so any other
/// top-level key is refused here by name, and so is a document with no
/// `providers:` key at all (an empty file, a bare sequence). A written
/// `providers: []` is legal — the empty roster, symmetric with the inline
/// form (the opener's limb (i)).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RosterFile {
    pub providers: Vec<ProviderCfg>,
}

impl RootFile {
    /// The pure join (ADR-037 D4) and the exactly-one-of rule's **one
    /// home** (spec §4: both written is a refusal, neither written is a
    /// refusal). The caller — the loader, the only code that knows a
    /// second file exists — hands the roster's parsed block only when the
    /// root named one; the result is today's [`RouterConfig`], the only
    /// representation the serving path ever sees. `validate()` runs
    /// *after* this, on the joined document, so cross-file references are
    /// checked in one pass with the existing key paths and message order.
    pub fn join(self, roster: Option<RosterFile>) -> Result<RouterConfig, ConfigError> {
        let Self {
            server,
            session,
            cache,
            trace,
            providers,
            providers_file,
            aliases,
            plugins,
            fallback,
            plan_policy,
            state,
        } = self;
        let providers = match (providers, providers_file, roster) {
            (Some(_), Some(file), _) => Err(ConfigError::new(
                "providers / providers_file",
                format!(
                    "both keys are written — the roster is inline AND named as '{file}': \
                     exactly one of `providers:` and `providers_file:` is written \
                     (spec §4/§4.14, ADR-037 D1)"
                ),
            )),
            (None, None, _) => Err(ConfigError::new(
                "providers / providers_file",
                "neither key is written — exactly one of `providers:` and \
                 `providers_file:` names the roster; an empty roster is a decision, \
                 written `providers: []` (spec §4/§4.14, ADR-037 D1)"
                    .to_string(),
            )),
            (Some(inline), None, None) => Ok(inline),
            (Some(_), None, Some(_)) => Err(ConfigError::new(
                "providers",
                "an inline roster AND a roster file's parsed block were both handed to \
                 the join — a loader bug, not a config state (the loader reads the \
                 roster only when the root names one)",
            )),
            (None, Some(_), None) => Err(ConfigError::new(
                "providers_file",
                "the root names a roster file but the join was handed no roster — the \
                 pair is read and validated together (spec §4.14; `config_load`'s \
                 `load` / `validate_pair` are the pair's entry points)",
            )),
            (None, Some(_), Some(roster)) => Ok(roster.providers),
        }?;
        Ok(RouterConfig {
            server,
            session,
            cache,
            trace,
            providers,
            aliases,
            plugins,
            fallback,
            plan_policy,
            state,
        })
    }
}

// ---------------------------------------------------------------------------
// Conversion + validation (§12.5 rules + the §12.10.2 load-time table)
// ---------------------------------------------------------------------------

/// The single load-time conversion point for prices (ADR-006). Floats live
/// only inside this function.
#[allow(clippy::float_arithmetic)]
fn price_to_nano(v: f64) -> u64 {
    (v * 1e9).round() as u64
}

/// The single load-time conversion point for multipliers (2.0 → 200); only
/// two decimal places are supported (§12.5).
#[allow(clippy::float_arithmetic)]
fn multiplier_to_pct(v: f64, what: &str) -> Result<u32, String> {
    if !v.is_finite() || v <= 0.0 {
        return Err(format!(
            "{what}: the multiplier must be a finite value > 0 (got {v})"
        ));
    }
    let scaled = v * 100.0;
    let pct = scaled.round();
    if (scaled - pct).abs() > 1e-9 {
        return Err(format!(
            "{what}: only two decimal places are supported (got {v})"
        ));
    }
    if pct > u32::MAX as f64 {
        return Err(format!("{what}: the multiplier is too large (got {v})"));
    }
    if pct as u32 == 0 {
        return Err(format!(
            "{what}: the multiplier converts to 0, which would make the tier free"
        ));
    }
    Ok(pct as u32)
}

impl PeakCfg {
    pub fn to_peak_table(&self) -> Result<PeakTable, String> {
        Ok(PeakTable {
            multiplier_pct: multiplier_to_pct(self.multiplier.0, "peak.multiplier")?,
            windows: self.windows.iter().map(WindowCfg::to_peak_window).collect(),
        })
    }
}

impl PriceCfg {
    /// The single conversion point for a price block (spec §4.10 / DESIGN
    /// §12.13, the generalisation of §12.5's `price` row): one `TierTable`
    /// per band; the flat shape yields exactly one with `up_to: None`.
    /// **Every** refusal of §4.10 rule 6 is decided in here, in the
    /// spec's row order, so a check and the conversion it guards cannot
    /// drift apart. The caller supplies the entry's `currency` (§4.8) and
    /// prefixes the config path (`providers[i].models[j].price`); the
    /// messages below name the band (`tiers[k]`) and the value found.
    pub fn to_tier_tables(&self, currency: Currency) -> Result<Vec<TierTable>, String> {
        // Rule 3: one peak table per entry, converted once here and shared
        // by every band's table — a band never carries its own multiplier.
        let peak = self.peak.to_peak_table()?;
        match &self.tiers {
            Some(tiers) => Self::banded_tables(self, tiers, currency, peak),
            None => Self::flat_table(self, currency, peak).map(|t| {
                vec![TierTable {
                    up_to: None,
                    table: t,
                }]
            }),
        }
    }

    /// Rule 1, the flat arm: the four flat scalars are one band with no
    /// ceiling. A missing scalar is the "neither shape" refusal naming the
    /// missing keys.
    fn flat_table(&self, currency: Currency, peak: PeakTable) -> Result<PriceTable, String> {
        const NAMES: [&str; 4] = ["input_miss", "input_hit", "cache_write", "output"];
        let given = [
            self.input_miss,
            self.input_hit,
            self.cache_write,
            self.output,
        ];
        let missing: Vec<&str> = NAMES
            .iter()
            .zip(given)
            .filter_map(|(name, v)| v.is_none().then_some(*name))
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "{}: missing — a price block is exactly one shape: the four flat \
                 prices or tiers (spec 4.10 rule 1)",
                missing.join(", ")
            ));
        }
        quad_to_table(
            "",
            self.input_miss,
            self.input_hit,
            self.cache_write,
            self.output,
            currency,
            peak,
        )
    }

    /// Rule 1's banded arm plus every band-level refusal of rule 6, in the
    /// spec table's row order: count → per-band ceilings → the unceiled
    /// band's cardinality and position → ascending order → per-band prices.
    fn banded_tables(
        &self,
        tiers: &[TierCfg],
        currency: Currency,
        peak: PeakTable,
    ) -> Result<Vec<TierTable>, String> {
        const NAMES: [&str; 4] = ["input_miss", "input_hit", "cache_write", "output"];
        // Row 1: both shapes written is refused before anything else.
        let flat_written: Vec<&str> = NAMES
            .iter()
            .zip([
                self.input_miss,
                self.input_hit,
                self.cache_write,
                self.output,
            ])
            .filter_map(|(name, v)| v.is_some().then_some(*name))
            .collect();
        if !flat_written.is_empty() {
            return Err(format!(
                "carries both the flat shape ({}) and tiers — a price block is \
                 exactly one shape (spec 4.10 rule 1)",
                flat_written.join(", ")
            ));
        }
        // Row 2: an empty list states no price; 8 is the cap.
        if tiers.is_empty() {
            return Err(
                "tiers: the band list is empty — a price block with no band states no \
                 price (spec 4.10 rule 6)"
                    .to_string(),
            );
        }
        if tiers.len() > MAX_PRICE_BANDS {
            return Err(format!(
                "tiers: {} bands — the cap is {} (no vendor publishes more than three \
                 bands, and a hand-written block stays reviewable; spec 4.10 rule 6)",
                tiers.len(),
                MAX_PRICE_BANDS
            ));
        }
        // Row 3: a ceiling is a positive integer of tokens — zero covers
        // nothing (n = 0 is priced by the first band), and a fractional or
        // negative value is not a token count the router can compare
        // against a measured figure.
        let mut ceilings: Vec<Option<u64>> = Vec::with_capacity(tiers.len());
        for (k, band) in tiers.iter().enumerate() {
            match band.up_to {
                None => ceilings.push(None),
                Some(CeilingVal(v)) => {
                    if !v.is_finite() || v < 1.0 || v.fract() != 0.0 {
                        return Err(format!(
                            "tiers[{k}].up_to: {v} — a ceiling is a positive integer of \
                             tokens (n = 0 is priced by the first band, so a ceiling below \
                             1 covers nothing; spec 4.10 rule 6)"
                        ));
                    }
                    if v > u64::MAX as f64 {
                        return Err(format!(
                            "tiers[{k}].up_to: {v} — the ceiling exceeds the token \
                             counter's range (u64)"
                        ));
                    }
                    ceilings.push(Some(v as u64));
                }
            }
        }
        // Rows 4–5: exactly one band omits `up_to`, and it is the last.
        let unceiled: Vec<usize> = tiers
            .iter()
            .enumerate()
            .filter_map(|(k, b)| b.up_to.is_none().then_some(k))
            .collect();
        let last = tiers.len() - 1;
        match unceiled.as_slice() {
            [] => {
                return Err(
                    "tiers: every band declares up_to — exactly one band, the last, \
                     must omit it (spec 4.10 rule 2)"
                        .to_string(),
                )
            }
            [k] if *k != last => {
                return Err(format!(
                    "tiers[{k}] omits up_to but is not the last band (band {last} is) — \
                     bands ascend and the unceiled band is the top one (spec 4.10 rule 2)"
                ))
            }
            [..] if unceiled.len() > 1 => {
                return Err(format!(
                    "tiers[{}] and tiers[{}] both omit up_to — exactly one band, the \
                     last, has no ceiling (spec 4.10 rule 2)",
                    unceiled[0], unceiled[1]
                ))
            }
            _ => {}
        }
        // Row 6: equal or descending ceilings would overlap, so one input
        // size would carry two prices.
        for i in 0..tiers.len().saturating_sub(1) {
            if let (Some(a), Some(b)) = (ceilings[i], ceilings[i + 1]) {
                if b <= a {
                    return Err(format!(
                        "tiers[{}].up_to: {} after tiers[{}].up_to: {} — bands must \
                         ascend (two equal or descending ceilings overlap, so one input \
                         size would carry two prices; spec 4.10 rule 6)",
                        i + 1,
                        b,
                        i,
                        a
                    ));
                }
            }
        }
        // Rows 7–8: every band states all four prices, and only
        // `cache_write` may convert to 0 — the §4.0 rule, per band.
        let mut out = Vec::with_capacity(tiers.len());
        for (k, band) in tiers.iter().enumerate() {
            let table = quad_to_table(
                &format!("tiers[{k}]."),
                band.input_miss,
                band.input_hit,
                band.cache_write,
                band.output,
                currency,
                peak.clone(),
            )?;
            out.push(TierTable {
                up_to: ceilings[k],
                table,
            });
        }
        Ok(out)
    }
}

/// The four-scalar conversion shared by the flat shape and every band (the
/// body of the former `to_price_table`, unchanged in its checks and
/// messages): `cache_write: 0` is legal (spec §4.0: the upstream charges no
/// separate cache-write price); a zero in any other tier is refused rather
/// than silently serving a free model. `prefix` is `""` for the flat shape
/// and `tiers[k].` for a band, so a band's refusal names its index.
fn quad_to_table(
    prefix: &str,
    input_miss: Option<PriceVal>,
    input_hit: Option<PriceVal>,
    cache_write: Option<PriceVal>,
    output: Option<PriceVal>,
    currency: Currency,
    peak: PeakTable,
) -> Result<PriceTable, String> {
    const TIERS: [(&str, bool); 4] = [
        ("input_miss", false),
        ("input_hit", false),
        ("cache_write", true),
        ("output", false),
    ];
    let given = [input_miss, input_hit, cache_write, output];
    let mut vals = [0u64; 4];
    for (i, (name, allow_zero)) in TIERS.iter().enumerate() {
        let Some(PriceVal(value)) = given[i] else {
            return Err(format!(
                "{prefix}{name}: the price is missing — every band states all four \
                 prices; a band that inherits a price from another band is a number the \
                 page did not publish for it (spec 4.10 rule 6)"
            ));
        };
        if !value.is_finite() || value < 0.0 {
            return Err(format!(
                "{prefix}{name}: the price must be a finite non-negative number (got {value})"
            ));
        }
        let nano = price_to_nano(value);
        if nano == 0 && !allow_zero {
            return Err(format!(
                "{prefix}{name}: converts to 0 nano-USD per 1K tokens; refusing a \
                 silently-free tier (write an explicit positive price)"
            ));
        }
        vals[i] = nano;
    }
    Ok(PriceTable {
        currency,
        input_miss: Price(vals[0]),
        input_hit: Price(vals[1]),
        cache_write: Price(vals[2]),
        output: Price(vals[3]),
        peak,
    })
}

/// Tier-A builtin kinds compiled into the product (plus the three always
/// resident ones, which take no config entries). Adding a builtin means
/// adding it here — the whitelist discipline of `ROUTER_OWNED_TOP_LEVEL_KEYS`.
pub const KNOWN_BUILTIN_KINDS: [&str; 6] = [
    "cache_guard",
    "transform_rules",
    "cost_ledger",
    "quota_guard",
    "sticky",
    // spec §4.17's exact-match response cache (ADR-042): a tier-A plugin
    // mounted from the `plugins:` list, off unless `config.enabled: true`.
    "response_cache",
];

/// Product-defined typed service-slot names usable in `inject` (spec §4.3).
pub const KNOWN_SERVICE_SLOTS: [&str; 2] = ["cache_ledger", "session_table"];

impl RouterConfig {
    /// Parse and return the listen address (the same check `validate` runs).
    pub fn listen_addr(&self) -> Result<SocketAddr, ConfigError> {
        self.server.addr.parse().map_err(|_| {
            ConfigError::new(
                "server.addr",
                format!(
                    "invalid listen address '{}' (expected host:port)",
                    self.server.addr
                ),
            )
        })
    }

    fn has_route(&self, r: &RouteSpec) -> bool {
        self.providers
            .iter()
            .any(|p| p.name == r.provider && p.models.iter().any(|m| m.id == r.model))
    }

    fn provider(&self, name: &str) -> Option<&ProviderCfg> {
        self.providers.iter().find(|p| p.name == name)
    }

    /// The family tag a route's model entry carries (§4.8): the written
    /// tag, or the model's own id when none was written — the one place
    /// the default lives. `None` when the route is not on the roster.
    fn family_tag_of(&self, route: &RouteSpec) -> Option<String> {
        let provider = self.provider(&route.provider)?;
        let model = provider.models.iter().find(|m| m.id == route.model)?;
        Some(model.family.clone().unwrap_or_else(|| model.id.clone()))
    }

    /// The spec §4.6 checks on `plan_policy` (DESIGN §12.10.2's row). They are
    /// routing rules, not syntax, so serde cannot police them — and the order
    /// below is the order a reader gets their message in: the routes exist →
    /// they differ → each sits on the right kind of account → the family is
    /// a tag both routes' model entries carry → the primary's declared plan
    /// covers the family → the cap is a readable amount in a comparable unit.
    fn validate_plan_policy(&self) -> Result<(), ConfigError> {
        let Some(policy) = &self.plan_policy else {
            return Ok(());
        };

        // A route is a plan route only if the roster has it: `provider/model`
        // with the model actually declared by that provider.
        let resolve = |key: &str, route: &RouteSpec| -> Result<&ProviderCfg, ConfigError> {
            self.provider(&route.provider)
                .filter(|p| p.models.iter().any(|m| m.id == route.model))
                .ok_or_else(|| {
                    ConfigError::new(
                        format!("plan_policy.{key}"),
                        format!(
                            "unknown route '{route}': plan_policy.{key} must be a \
                             <provider>/<model> route in the roster (spec §4.6)"
                        ),
                    )
                })
        };
        let primary = resolve("primary", &policy.primary)?;
        let overflow = resolve("overflow", &policy.overflow)?;

        if policy.primary == policy.overflow {
            return Err(ConfigError::new(
                "plan_policy.overflow",
                format!(
                    "must differ from plan_policy.primary ('{}'): the two accounts are distinct \
                     routes (spec §4.6)",
                    policy.primary
                ),
            ));
        }

        if primary.account != AccountKind::CodingPlan {
            return Err(ConfigError::new(
                "plan_policy.primary",
                format!(
                    "provider '{}' is account: {}; plan_policy.primary must be a route of a \
                     provider declaring account: coding_plan (spec §4.6)",
                    primary.name, primary.account
                ),
            ));
        }

        if overflow.account != AccountKind::Api {
            return Err(ConfigError::new(
                "plan_policy.overflow",
                format!(
                    "provider '{}' is account: {}; plan_policy.overflow must be a route of a \
                     provider declaring account: api (spec §4.6)",
                    overflow.name, overflow.account
                ),
            ));
        }

        for (key, tag) in [
            ("primary", self.family_tag_of(&policy.primary)),
            ("overflow", self.family_tag_of(&policy.overflow)),
        ] {
            match tag {
                Some(tag) if tag == policy.family => {}
                Some(tag) => {
                    return Err(ConfigError::new(
                        "plan_policy.family",
                        format!(
                            "family '{}' must equal the family tag plan_policy.{key}'s model entry \
                             carries ('{tag}'): both routes serve the family (spec §4.6/§4.8)",
                            policy.family
                        ),
                    ));
                }
                None => unreachable!("resolve() already established the route is on the roster"),
            }
        }

        // A provider that declares a plan must cover the family (spec §4.6,
        // restated by §4.8 in terms of ids): the check names **the model id
        // the primary route resolves to** — `quota.models` names ids, and the
        // two routes' ids may differ. A provider that declares no plan is
        // legal: GAP-Q1 — a plan whose allowance is not published is still
        // a plan.
        let plans = primary.quota.as_deref().unwrap_or(&[]);
        let covered = plans
            .iter()
            .any(|q| q.models.iter().any(|m| m == &policy.primary.model));
        if !plans.is_empty() && !covered {
            let declared: Vec<&str> = plans
                .iter()
                .flat_map(|q| q.models.iter().map(String::as_str))
                .collect();
            return Err(ConfigError::new(
                "plan_policy.family",
                format!(
                    "family '{}' is not covered by provider '{}'s quota.models {declared:?}: a \
                     provider that declares a plan must include the model id the primary route \
                     resolves to ('{}') (spec §4.6/§4.8)",
                    policy.family, primary.name, policy.primary.model
                ),
            ));
        }

        // The one scalar comparison that could mix units (spec §4.6/§4.8):
        // `overflow_monthly_cap_usd` is USD by name, so writing it over an
        // overflow route of another currency is refused before the process
        // serves — a load error, never a silent cross-currency compare.
        if let Some(cap) = policy.overflow_monthly_cap_usd {
            cap.to_nano().map_err(|reason| {
                ConfigError::new("plan_policy.overflow_monthly_cap_usd", reason)
            })?;
            if overflow.currency != Currency::Usd {
                return Err(ConfigError::new(
                    "plan_policy.overflow_monthly_cap_usd",
                    format!(
                        "the cap is denominated in USD by its own name, but the overflow route \
                         '{}''s provider '{}' is currency {}: comparing a USD ceiling with a \
                         spend in another currency is the silent mixing spec 4.8 forbids — \
                         remove the cap or point the overflow at a USD entry (spec 4.6/4.8)",
                        policy.overflow, overflow.name, overflow.currency
                    ),
                ));
            }
        }

        Ok(())
    }

    /// The §12.10.2 load-time validation table. Every failure names the key
    /// and the reason; there is no partially-started process.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.listen_addr()?;

        // Spec §4.13: the body bound is a load-time rule like any other
        // scalar — a value below 1024 (including 0 and negatives) cannot
        // bound anything and names its key in the refusal.
        if self.server.max_body_bytes < MIN_MAX_BODY_BYTES {
            return Err(ConfigError::new(
                "server.max_body_bytes",
                format!(
                    "{} — the inbound body limit must be an integer of bytes ≥ {} \
                     (there is deliberately no 'unlimited' spelling; write a large \
                     number where a reader can see it)",
                    self.server.max_body_bytes, MIN_MAX_BODY_BYTES
                ),
            ));
        }

        for (i, src) in self.session.key_sources.iter().enumerate() {
            if src != "prompt_cache_key" {
                let ok = src
                    .strip_prefix("header:")
                    .is_some_and(|name| !name.is_empty());
                if !ok {
                    return Err(ConfigError::new(
                        format!("session.key_sources[{i}]"),
                        format!(
                            "unknown key source '{src}': expected 'prompt_cache_key' or \
                             'header:<name>' with a non-empty header name"
                        ),
                    ));
                }
            }
        }

        multiplier_to_pct(
            self.cache.breakeven.safety_factor.0,
            "cache.breakeven.safety_factor",
        )
        .map_err(|reason| ConfigError::new("cache.breakeven.safety_factor", reason))?;

        for (pi, p) in self.providers.iter().enumerate() {
            let ppath = format!("providers[{pi}] ({})", p.name);

            if !p.supports.contains(&p.wire_api) {
                let supports: Vec<&str> = p.supports.iter().map(|w| w.as_str()).collect();
                return Err(ConfigError::new(
                    format!("{ppath}.wire_api"),
                    format!(
                        "wire_api '{}' is not declared in this provider's supports {:?} \
                         (the native format must be a declared cell of the matrix, spec §4)",
                        p.wire_api.as_str(),
                        supports
                    ),
                ));
            }

            // spec §4.9 / ADR-020: the declaration and the reachability cannot
            // disagree. Both directions are refused here, so "declared but
            // unreachable" (the defect ADR-020 removes) and "reachable but
            // undeclared" (a capability nobody reads) are equally impossible.
            // `urls` is a BTreeMap, so the reported key is deterministic.
            for (wi, w) in p.supports.iter().enumerate() {
                if !p.urls.contains_key(w) {
                    return Err(ConfigError::new(
                        format!("{ppath}.urls"),
                        format!(
                            "supports[{wi}] '{}' is declared but this entry carries no `urls.{}` — \
                             every declared cell needs its complete URL, and the router composes \
                             no path (spec §4.9, ADR-020)",
                            w.as_str(),
                            w.as_str()
                        ),
                    ));
                }
            }
            for (w, url) in &p.urls {
                if !p.supports.contains(w) {
                    let supports: Vec<&str> = p.supports.iter().map(|s| s.as_str()).collect();
                    return Err(ConfigError::new(
                        format!("{ppath}.urls.{}", w.as_str()),
                        format!(
                            "a URL for a cell this entry does not declare: add '{}' to supports {:?} \
                             or drop the URL (spec §4.9, ADR-020)",
                            w.as_str(),
                            supports
                        ),
                    ));
                }
                let absolute = url.starts_with("http://") || url.starts_with("https://");
                if !absolute || url.chars().any(char::is_whitespace) {
                    return Err(ConfigError::new(
                        format!("{ppath}.urls.{}", w.as_str()),
                        format!(
                            "'{url}' is not an absolute http(s) URL free of whitespace — this value \
                             is sent verbatim, so a relative or malformed one has no repair path \
                             (spec §4.9, ADR-020)"
                        ),
                    ));
                }
            }

            for (mi, m) in p.models.iter().enumerate() {
                let dup = p.models[..mi].iter().any(|prev| prev.id == m.id);
                if dup {
                    return Err(ConfigError::new(
                        format!("{ppath}.models[{mi}]"),
                        format!(
                            "duplicate model id '{}' within provider '{}' (ambiguous roster)",
                            m.id, p.name
                        ),
                    ));
                }
                // spec §4.8: a tag is a non-empty string unique within its
                // provider entry (it must resolve to at most one model —
                // `plan_policy.family` matches it). Uniqueness is checked
                // on the EFFECTIVE tag — written, or defaulted to the
                // entry's own id — in both orders: a later written tag
                // against an earlier entry, and a later DEFAULTED tag
                // against an earlier written one (the order that only
                // appears once family tags exist at all).
                let mpath = format!("{ppath}.models[{mi}] ({})", m.id);
                if let Some(tag) = &m.family {
                    if tag.is_empty() {
                        return Err(ConfigError::new(
                            format!("{mpath}.family"),
                            "family must be a non-empty string when written; an absent key \
                             means the model's own id (spec 4.8)"
                                .to_string(),
                        ));
                    }
                }
                let effective = m.family.as_deref().unwrap_or(&m.id);
                let dup_written_or_default = p.models[..mi]
                    .iter()
                    .any(|prev| prev.family.as_deref().unwrap_or(&prev.id) == effective);
                if dup_written_or_default {
                    return Err(ConfigError::new(
                        format!("{mpath}.family"),
                        format!(
                            "family tag '{effective}' is already carried by another model entry of \
                             provider '{}': a tag resolves to at most one model per provider \
                             (spec 4.8)",
                            p.name
                        ),
                    ));
                }
                m.price
                    .to_tier_tables(p.currency)
                    .map_err(|reason| ConfigError::new(format!("{mpath}.price"), reason))?;
            }

            for (qi, q) in p.quota.as_deref().unwrap_or(&[]).iter().enumerate() {
                for model in &q.models {
                    if !p.models.iter().any(|m| &m.id == model) {
                        return Err(ConfigError::new(
                            format!("{ppath}.quota[{qi}].models"),
                            format!(
                                "'{model}' is not a model of provider '{}' (quota may only \
                                 reference its own provider's models, spec §4.0)",
                                p.name
                            ),
                        ));
                    }
                }
            }

            let dup_provider = self.providers[..pi].iter().any(|prev| prev.name == p.name);
            if dup_provider {
                return Err(ConfigError::new(
                    format!("providers[{pi}]"),
                    format!(
                        "duplicate provider name '{}' (the roster must be unambiguous)",
                        p.name
                    ),
                ));
            }
        }

        for (alias, route) in &self.aliases {
            if !self.has_route(route) {
                return Err(ConfigError::new(
                    format!("aliases.{alias}"),
                    format!("unknown route '{route}': no such provider/model in the roster"),
                ));
            }
        }

        for (i, route) in self.fallback.iter().enumerate() {
            if !self.has_route(route) {
                return Err(ConfigError::new(
                    format!("fallback[{i}]"),
                    format!(
                        "unknown route '{route}': fallback entries must be roster \
                         provider/model routes (aliases do not take part, spec §4.2)"
                    ),
                ));
            }
        }

        for (i, plug) in self.plugins.iter().enumerate() {
            let dup = self.plugins[..i].iter().any(|prev| prev.id == plug.id);
            if dup {
                return Err(ConfigError::new(
                    format!("plugins[{i}]"),
                    format!(
                        "duplicate plugin id '{}' (ambiguous fiber identity)",
                        plug.id
                    ),
                ));
            }
            let ppath = format!("plugins[{i}] ({})", plug.id);
            if let Some(builtin) = plug.kind.strip_prefix("builtin/") {
                if !KNOWN_BUILTIN_KINDS.contains(&builtin) {
                    return Err(ConfigError::new(
                        format!("{ppath}.kind"),
                        format!(
                            "unknown builtin '{builtin}' (known builtin kinds: {})",
                            KNOWN_BUILTIN_KINDS.join(", ")
                        ),
                    ));
                }
            } else if plug.kind == "process" {
                if plug.url.is_none() {
                    return Err(ConfigError::new(
                        ppath,
                        "kind 'process' requires 'url' (the tier-B socket to connect to)",
                    ));
                }
            } else {
                return Err(ConfigError::new(
                    format!("{ppath}.kind"),
                    format!(
                        "unknown kind '{}' (expected 'builtin/<name>' or 'process')",
                        plug.kind
                    ),
                ));
            }
            if let Some(ic) = &plug.intercept {
                let in_range = (0.0..=1.0).contains(&ic.sample);
                // Comparison only; no float arithmetic on this path.
                if !in_range {
                    return Err(ConfigError::new(
                        format!("{ppath}.intercept.sample"),
                        format!("sample must be within [0, 1] (got {})", ic.sample),
                    ));
                }
            }
            for (si, slot) in plug.inject.iter().enumerate() {
                if !KNOWN_SERVICE_SLOTS.contains(&slot.as_str()) {
                    return Err(ConfigError::new(
                        format!("{ppath}.inject[{si}]"),
                        format!(
                            "unknown service slot '{slot}' (inject names product-defined \
                             typed slots, spec §4.3; known: {})",
                            KNOWN_SERVICE_SLOTS.join(", ")
                        ),
                    ));
                }
            }
        }

        self.validate_plan_policy()?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The full happy path, shaped like config.example.yaml. Built through
    // serde_json so router-core needs no YAML dependency: the scalar
    // conventions under test (durations, context suffixes, prices, routes)
    // are format-independent by construction.
    const HAPPY: &str = r#"{
        "server": {"addr": "127.0.0.1:8790", "upstream_attempt_timeout": "60s", "request_timeout": "10m"},
        "session": {"key_sources": ["prompt_cache_key", "header:session-id"], "ttl": "12h"},
        "cache": {"sticky": true, "breakeven": {"enabled": true, "min_remaining_turns": 3, "safety_factor": 1.2}},
        "trace": {"dir": "./state/traces", "rollover": "hourly"},
        "providers": [{
            "name": "p",
            "urls": {"chat": "https://x.example/v1/chat/completions", "anthropic": "https://x.example/v1/messages"},
            "api_key_env": "P_KEY",
            "wire_api": "chat",
            "supports": ["chat", "anthropic"],
            "models": [{
                "id": "m1",
                "context": "200k",
                "price": {
                    "input_miss": 0.00066, "input_hit": 0.000022,
                    "cache_write": 0.0, "output": 0.00198,
                    "peak": {"multiplier": 2.0, "windows": [
                        {"days": ["Mon", "Tue", "Wed", "Thu", "Fri"], "from": "01:00", "to": "04:00", "tz": "UTC"},
                        {"days": ["Sat"], "from": "22:00", "to": "02:00", "tz": "+08:00"}
                    ]}
                },
                "source": "https://x.example/pricing @2026-09-19"
            }],
            "quota": [{"models": ["m1"], "window": "monthly", "tokens": 100000000, "reset_day": 1, "over_quota": "block", "source": "s"}]
        }],
        "aliases": {"fast": "p/m1"},
        "plugins": [
            {"id": "g", "kind": "builtin/cache_guard", "config": {"strict_prefix": true}},
            {"id": "off", "kind": "process", "url": "unix:///tmp/x.sock", "inject": ["cache_ledger"],
             "intercept": {"sample": 0.05, "shadow": true}, "disabled": true}
        ],
        "fallback": ["p/m1"]
    }"#;

    fn happy() -> RouterConfig {
        let cfg: RouterConfig = serde_json::from_str(HAPPY).expect("happy config parses");
        cfg.validate().expect("happy config validates");
        cfg
    }

    fn deser_err(s: &str) -> String {
        serde_json::from_str::<RouterConfig>(s)
            .expect_err("expected a parse error")
            .to_string()
    }

    fn validate_err(cfg: &RouterConfig) -> String {
        cfg.validate()
            .expect_err("expected a validation error")
            .to_string()
    }

    #[test]
    fn happy_path_parses_and_validates() {
        let cfg = happy();
        assert_eq!(cfg.listen_addr().unwrap().to_string(), "127.0.0.1:8790");
        assert_eq!(cfg.server.upstream_attempt_timeout, DurationVal(60_000));
        assert_eq!(cfg.server.request_timeout, DurationVal(600_000));
        assert_eq!(cfg.session.ttl, DurationVal(43_200_000));
        assert_eq!(cfg.providers[0].models[0].context, ContextVal(200 * 1024));
    }

    #[test]
    fn price_conversion_is_exact_at_load_time() {
        let cfg = happy();
        let tables = cfg.providers[0].models[0]
            .price
            .to_tier_tables(cfg.providers[0].currency)
            .unwrap();
        assert_eq!(
            tables.len(),
            1,
            "the flat shape is one band with no ceiling"
        );
        assert_eq!(tables[0].up_to, None);
        let table = &tables[0].table;
        assert_eq!(table.input_miss, Price(660_000));
        assert_eq!(table.input_hit, Price(22_000));
        assert_eq!(table.cache_write, Price(0)); // legal: no separate cache-write billing
        assert_eq!(table.output, Price(1_980_000));
        assert_eq!(table.peak.multiplier_pct, 200);
        let w = &table.peak.windows[0];
        assert_eq!((w.from_min, w.to_min), (60, 240));
        assert_eq!(w.days, Weekdays(Weekdays::MON_FRI.0));
        let wrap = &table.peak.windows[1];
        assert_eq!((wrap.from_min, wrap.to_min), (22 * 60, 2 * 60)); // spans midnight
        assert_eq!(wrap.tz, Tz::OffsetMin(480));
    }

    #[test]
    fn durations_stack_and_reject_bare_numbers() {
        assert_eq!(parse_duration("500ms").unwrap(), 500);
        assert_eq!(parse_duration("1h30m").unwrap(), 3_600_000 + 1_800_000);
        assert!(parse_duration("100").is_err()); // bare seconds unsupported (§12.5)
        assert!(parse_duration("1h30").is_err()); // trailing number, no unit
        assert!(parse_duration("1x").is_err());
        assert!(parse_duration("").is_err());
    }

    #[test]
    fn context_suffixes_and_rejection() {
        assert_eq!(parse_context("131072").unwrap(), 131_072);
        assert_eq!(parse_context("1m").unwrap(), 1_048_576); // 1m = mebi, not one minute
        assert_eq!(parse_context("200K").map_err(|_| ()), Err(())); // uppercase not a YAML suffix form; reject
        assert!(parse_context("1g").is_err());
        assert!(parse_context("k").is_err());
    }

    #[test]
    fn tokens_accept_underscores() {
        assert_eq!(parse_tokens("100_000_000").unwrap(), 100_000_000);
        assert_eq!(parse_tokens("42").unwrap(), 42);
        assert!(parse_tokens("1.5").is_err());
        assert!(parse_tokens("abc").is_err());
    }

    #[test]
    fn missing_section_is_an_error() {
        // Drop the session section entirely (renaming would trip
        // deny_unknown_fields first).
        let s = HAPPY.replace(
            "\"session\": {\"key_sources\": [\"prompt_cache_key\", \"header:session-id\"], \"ttl\": \"12h\"},",
            "",
        );
        let err = deser_err(&s);
        assert!(err.contains("missing field"), "got: {err}");
        assert!(err.contains("session"), "got: {err}");
    }

    #[test]
    fn wrong_scalar_type_is_an_error() {
        let s = HAPPY.replace("\"ttl\": \"12h\"", "\"ttl\": 12");
        let err = deser_err(&s);
        assert!(err.contains("invalid type"), "got: {err}");
    }

    #[test]
    fn unknown_field_is_refused_not_ignored() {
        let s = HAPPY.replace("\"fallback\":", "\"experimental\": true, \"fallback\":");
        let err = deser_err(&s);
        assert!(err.contains("unknown field"), "got: {err}");
        assert!(err.contains("experimental"), "got: {err}");
    }

    #[test]
    fn state_section_names_its_future_status() {
        let s = HAPPY.replace(
            "\"fallback\":",
            "\"state\": {\"dir\": \"x\"}, \"fallback\":",
        );
        let err = deser_err(&s);
        assert!(err.contains("state"), "got: {err}");
        assert!(err.contains("not a config key in v0.1"), "got: {err}");
    }

    #[test]
    fn alias_to_unknown_route_is_rejected_with_key_and_route() {
        let mut cfg = happy();
        cfg.aliases
            .insert("coding-fast".into(), parse_route("zai/glm-9").unwrap());
        let err = validate_err(&cfg);
        assert!(err.contains("aliases.coding-fast"), "got: {err}");
        assert!(err.contains("zai/glm-9"), "got: {err}");
    }

    #[test]
    fn fallback_entry_must_be_a_roster_route() {
        let mut cfg = happy();
        cfg.fallback.push(parse_route("nope/m").unwrap());
        let err = validate_err(&cfg);
        assert!(err.contains("fallback[1]"), "got: {err}");
    }

    #[test]
    fn quota_referencing_a_foreign_model_is_rejected() {
        let mut cfg = happy();
        cfg.providers[0].quota.as_mut().unwrap()[0]
            .models
            .push("someone-elses-model".into());
        let err = validate_err(&cfg);
        assert!(err.contains("quota[0].models"), "got: {err}");
        assert!(err.contains("someone-elses-model"), "got: {err}");
        assert!(err.contains("its own provider"), "got: {err}");
    }

    #[test]
    fn wire_api_must_be_declared_in_supports() {
        let mut cfg = happy();
        cfg.providers[0].supports = vec![WireApi::Anthropic];
        let err = validate_err(&cfg);
        assert!(err.contains("wire_api"), "got: {err}");
        assert!(err.contains("supports"), "got: {err}");
    }

    #[test]
    fn a_declared_cell_without_a_url_is_refused() {
        let mut cfg = happy();
        cfg.providers[0].urls.remove(&WireApi::Anthropic);
        let err = validate_err(&cfg);
        // the path carries the provider name (DESIGN §12.10.2's convention)
        assert!(err.contains("providers[0] (p).urls"), "got: {err}");
        assert!(err.contains("no `urls.anthropic`"), "got: {err}");
    }

    #[test]
    fn a_url_for_an_undeclared_cell_is_refused() {
        let mut cfg = happy();
        cfg.providers[0].supports = vec![WireApi::Chat];
        let err = validate_err(&cfg);
        assert!(
            err.contains("providers[0] (p).urls.anthropic"),
            "got: {err}"
        );
        assert!(err.contains("does not declare"), "got: {err}");
    }

    #[test]
    fn a_url_must_be_absolute_and_free_of_whitespace() {
        for bad in [
            "api.example.com/chat/completions",
            "ftp://api.example.com/chat/completions",
            "https://api.example.com/chat completions",
            "",
        ] {
            let mut cfg = happy();
            cfg.providers[0].urls.insert(WireApi::Chat, bad.into());
            let err = validate_err(&cfg);
            assert!(
                err.contains("providers[0] (p).urls.chat"),
                "bad={bad:?} got: {err}"
            );
            assert!(err.contains("absolute http(s)"), "bad={bad:?} got: {err}");
        }
    }

    #[test]
    fn an_unknown_wire_key_in_urls_is_refused_by_the_key_type() {
        let s = HAPPY.replace(
            "\"chat\": \"https://x.example/v1/chat/completions\"",
            "\"chta\": \"https://x.example/v1/chat/completions\"",
        );
        assert_ne!(s, HAPPY, "the fixture must contain the key being replaced");
        let err = deser_err(&s);
        assert!(err.contains("chta"), "got: {err}");
        assert!(err.contains("chat, responses or anthropic"), "got: {err}");
    }

    #[test]
    fn url_for_returns_the_configured_string_untouched() {
        let cfg = happy();
        assert_eq!(
            cfg.providers[0].url_for(WireApi::Chat),
            Some("https://x.example/v1/chat/completions")
        );
        assert_eq!(
            cfg.providers[0].url_for(WireApi::Anthropic),
            Some("https://x.example/v1/messages")
        );
        // A trailing slash is NOT trimmed: the router composes nothing, so it
        // must not silently fix anything either (ADR-020 item 1).
        let mut cfg = happy();
        cfg.providers[0].urls.insert(
            WireApi::Chat,
            "https://x.example/v1/chat/completions/".into(),
        );
        assert_eq!(
            cfg.providers[0].url_for(WireApi::Chat),
            Some("https://x.example/v1/chat/completions/")
        );
    }

    #[test]
    fn duplicate_provider_and_model_names_are_rejected() {
        let mut cfg = happy();
        let mut clone = cfg.providers[0].clone();
        clone.models.clear();
        clone.quota = None; // same name, different content — still ambiguous
        cfg.providers.push(clone);
        assert!(validate_err(&cfg).contains("duplicate provider name"));

        let mut cfg = happy();
        let mut m = cfg.providers[0].models[0].clone();
        m.id = cfg.providers[0].models[0].id.clone();
        cfg.providers[0].models.push(m);
        assert!(validate_err(&cfg).contains("duplicate model id"));
    }

    #[test]
    fn zero_price_outside_cache_write_is_refused() {
        let mut cfg = happy();
        cfg.providers[0].models[0].price.input_miss = Some(PriceVal(0.0));
        let err = validate_err(&cfg);
        assert!(err.contains("input_miss"), "got: {err}");
        assert!(err.contains("silently-free"), "got: {err}");
    }

    // -----------------------------------------------------------------
    // spec §4.10 (banded pricing): fixtures, the two-spellings witness
    // and one negative case per rule-6 refusal row.
    // -----------------------------------------------------------------

    fn no_peak() -> PeakCfg {
        PeakCfg {
            multiplier: MultiplierVal(1.0),
            windows: Vec::new(),
        }
    }

    fn flat_price() -> PriceCfg {
        PriceCfg {
            input_miss: Some(PriceVal(0.00066)),
            input_hit: Some(PriceVal(0.000022)),
            cache_write: Some(PriceVal(0.0)),
            output: Some(PriceVal(0.00198)),
            peak: no_peak(),
            tiers: None,
        }
    }

    /// One band: explicit prices (no float arithmetic — the crate denies
    /// it), `x2` carries exactly double the flat prices so a selected band
    /// is identifiable from the money it produces.
    fn band(up_to: Option<u64>, x2: bool) -> TierCfg {
        TierCfg {
            up_to: up_to.map(|v| CeilingVal(v as f64)),
            input_miss: Some(PriceVal(if x2 { 0.00132 } else { 0.00066 })),
            input_hit: Some(PriceVal(if x2 { 0.000044 } else { 0.000022 })),
            cache_write: Some(PriceVal(0.0)),
            output: Some(PriceVal(if x2 { 0.00396 } else { 0.00198 })),
        }
    }

    fn banded_price(tiers: Vec<TierCfg>) -> PriceCfg {
        PriceCfg {
            input_miss: None,
            input_hit: None,
            cache_write: None,
            output: None,
            peak: no_peak(),
            tiers: Some(tiers),
        }
    }

    fn cfg_with(price: PriceCfg) -> RouterConfig {
        let mut cfg = happy();
        cfg.providers[0].models[0].price = price;
        cfg
    }

    #[test]
    fn tiered_happy_path_loads_and_ascending_bands_are_kept() {
        let cfg = cfg_with(banded_price(vec![
            band(Some(200_000), false),
            band(Some(1_000_000), false),
            band(None, true),
        ]));
        cfg.validate().expect("a legal banded block validates");
        let tables = cfg.providers[0].models[0]
            .price
            .to_tier_tables(cfg.providers[0].currency)
            .unwrap();
        assert_eq!(
            tables.iter().map(|t| t.up_to).collect::<Vec<_>>(),
            vec![Some(200_000), Some(1_000_000), None]
        );
        assert_eq!(tables[0].table.input_miss, Price(660_000));
        assert_eq!(tables[2].table.input_miss, Price(1_320_000), "the x2 band");
        // Rule 3: every band carries the entry's single peak table.
        assert_eq!(tables[0].table.peak, tables[2].table.peak);
    }

    /// The backward-compatibility witness (DESIGN §12.13 test 2): the same
    /// four prices written flat and as a one-band `tiers:` list produce
    /// identical tables and identical money for the same usage.
    #[test]
    fn flat_and_one_band_tiers_are_one_table() {
        use crate::cost::{cost, select_band, Usage};
        const AT: u64 = 1_789_992_000;

        let flat = flat_price().to_tier_tables(Currency::Usd).unwrap();
        let one_band = banded_price(vec![band(None, false)])
            .to_tier_tables(Currency::Usd)
            .unwrap();
        assert_eq!(flat, one_band, "the two spellings load to one table");

        let usage = Usage {
            input_total: 123_456,
            input_cached: 100_000,
            cache_write: 500,
            output: 789,
            reasoning: 10,
        };
        assert_eq!(
            cost(&usage, select_band(&flat, usage.input_total), AT),
            cost(&usage, select_band(&one_band, usage.input_total), AT),
            "the two spellings produce identical money"
        );
        // And the validate() path (the load check) accepts both.
        cfg_with(flat_price()).validate().unwrap();
        cfg_with(banded_price(vec![band(None, false)]))
            .validate()
            .unwrap();
    }

    #[test]
    fn both_shapes_written_is_refused() {
        let mut price = banded_price(vec![band(None, false)]);
        price.input_miss = Some(PriceVal(0.00066));
        let err = validate_err(&cfg_with(price));
        assert!(err.contains("price"), "names the price path: {err}");
        assert!(err.contains("input_miss"), "names the flat key: {err}");
        assert!(err.contains("exactly one shape"), "got: {err}");
    }

    #[test]
    fn neither_shape_written_is_refused() {
        let mut price = flat_price();
        price.output = None;
        let err = validate_err(&cfg_with(price));
        assert!(err.contains("output"), "names the missing key: {err}");
        assert!(err.contains("exactly one shape"), "got: {err}");
    }

    #[test]
    fn empty_tiers_list_is_refused() {
        let err = validate_err(&cfg_with(banded_price(Vec::new())));
        assert!(err.contains("tiers"), "got: {err}");
        assert!(err.contains("empty"), "got: {err}");
    }

    #[test]
    fn band_count_cap_is_eight() {
        let eight: Vec<TierCfg> = (0..7)
            .map(|i| band(Some(200_000 + i * 1_000), false))
            .chain(std::iter::once(band(None, false)))
            .collect();
        cfg_with(banded_price(eight))
            .validate()
            .expect("8 bands load");
        let nine: Vec<TierCfg> = (0..8)
            .map(|i| band(Some(200_000 + i * 1_000), false))
            .chain(std::iter::once(band(None, false)))
            .collect();
        let err = validate_err(&cfg_with(banded_price(nine)));
        assert!(err.contains("9 bands"), "names the count found: {err}");
        assert!(err.contains("cap is 8"), "names the cap: {err}");
    }

    #[test]
    fn ceiling_below_one_or_fractional_is_refused_per_band() {
        for bad in [0.0, -5.0] {
            let mut price = banded_price(vec![band(Some(200_000), false), band(None, false)]);
            price.tiers.as_mut().unwrap()[0].up_to = Some(CeilingVal(bad));
            let err = validate_err(&cfg_with(price));
            assert!(err.contains("tiers[0].up_to"), "names the band: {err}");
            assert!(err.contains("positive integer"), "got: {err}");
        }
        // Fractional: refused with the value found, named per band.
        let mut price = banded_price(vec![band(Some(200_000), false), band(None, false)]);
        price.tiers.as_mut().unwrap()[0].up_to = Some(CeilingVal(200_000.5));
        let err = validate_err(&cfg_with(price));
        assert!(err.contains("tiers[0].up_to"), "got: {err}");
        assert!(err.contains("200000.5"), "names the value found: {err}");
    }

    #[test]
    fn ceiling_rejects_suffix_spellings_at_parse() {
        // A `k`/`m` suffix is a parse refusal, not a multiplier (§4.10
        // rule 2: a ceiling is a plain integer, unlike `context`).
        let err = serde_json::from_str::<PriceCfg>(
            r#"{"tiers": [{"up_to": "200k", "input_miss": 0.1, "input_hit": 0.01,
                "cache_write": 0.0, "output": 0.2}, {"input_miss": 0.1, "input_hit": 0.01,
                "cache_write": 0.0, "output": 0.2}],
                "peak": {"multiplier": 1.0, "windows": []}}"#,
        )
        .expect_err("expected a parse error")
        .to_string();
        assert!(err.contains("plain integer token count"), "got: {err}");
    }

    #[test]
    fn no_ceilless_band_is_refused() {
        let err = validate_err(&cfg_with(banded_price(vec![
            band(Some(200_000), false),
            band(Some(1_000_000), false),
        ])));
        assert!(err.contains("every band declares up_to"), "got: {err}");
    }

    #[test]
    fn two_ceilless_bands_are_refused() {
        let err = validate_err(&cfg_with(banded_price(vec![
            band(Some(200_000), false),
            band(None, false),
            band(None, false),
        ])));
        assert!(err.contains("tiers[1]"), "names the first offender: {err}");
        assert!(err.contains("tiers[2]"), "names the second: {err}");
        assert!(err.contains("exactly one band"), "got: {err}");
    }

    #[test]
    fn ceilless_band_not_last_is_refused() {
        // The only unceiled band sits at index 0 while index 2 is last.
        let err = validate_err(&cfg_with(banded_price(vec![
            band(None, false),
            band(Some(1_000_000), false),
            band(Some(2_000_000), false),
        ])));
        assert!(err.contains("tiers[0] omits up_to"), "got: {err}");
        assert!(err.contains("not the last band"), "got: {err}");
    }

    #[test]
    fn equal_ceilings_are_refused() {
        let err = validate_err(&cfg_with(banded_price(vec![
            band(Some(200_000), false),
            band(Some(200_000), false),
            band(None, false),
        ])));
        assert!(err.contains("tiers[1].up_to: 200000"), "got: {err}");
        assert!(err.contains("tiers[0].up_to: 200000"), "got: {err}");
        assert!(err.contains("overlap"), "got: {err}");
    }

    #[test]
    fn descending_ceilings_are_refused() {
        let err = validate_err(&cfg_with(banded_price(vec![
            band(Some(1_000_000), false),
            band(Some(200_000), false),
            band(None, false),
        ])));
        assert!(err.contains("ascend"), "got: {err}");
    }

    #[test]
    fn band_missing_a_price_is_refused() {
        let mut price = banded_price(vec![band(Some(200_000), false), band(None, false)]);
        price.tiers.as_mut().unwrap()[1].input_hit = None;
        let err = validate_err(&cfg_with(price));
        assert!(err.contains("tiers[1].input_hit"), "got: {err}");
        assert!(err.contains("all four"), "got: {err}");
    }

    #[test]
    fn band_zero_price_outside_cache_write_is_refused_per_band() {
        let mut price = banded_price(vec![band(Some(200_000), false), band(None, false)]);
        price.tiers.as_mut().unwrap()[1].output = Some(PriceVal(0.0));
        let err = validate_err(&cfg_with(price.clone()));
        assert!(err.contains("tiers[1].output"), "got: {err}");
        assert!(err.contains("silently-free"), "got: {err}");
        // cache_write: 0 stays legal per band.
        price.tiers.as_mut().unwrap()[1].output = Some(PriceVal(0.00198));
        price.tiers.as_mut().unwrap()[1].cache_write = Some(PriceVal(0.0));
        cfg_with(price).validate().expect("cache_write 0 is legal");
    }

    #[test]
    fn peak_inside_a_band_is_refused_at_parse() {
        let err = serde_json::from_str::<TierCfg>(
            r#"{"up_to": 200000, "input_miss": 0.1, "input_hit": 0.01,
                "cache_write": 0.0, "output": 0.2,
                "peak": {"multiplier": 1.0, "windows": []}}"#,
        )
        .expect_err("expected a parse error")
        .to_string();
        assert!(
            err.contains("unknown field") && err.contains("peak"),
            "names the refused key: {err}"
        );
    }

    #[test]
    fn peak_multiplier_precision_is_two_decimals() {
        let mut cfg = happy();
        cfg.providers[0].models[0].price.peak.multiplier = MultiplierVal(1.234);
        assert!(validate_err(&cfg).contains("two decimal places"));
        // 1.25 is fine: 125 pct.
        cfg.providers[0].models[0].price.peak.multiplier = MultiplierVal(1.25);
        cfg.validate().expect("1.25 is representable");
    }

    #[test]
    fn invalid_listen_address_is_rejected() {
        let mut cfg = happy();
        cfg.server.addr = "not an address".into();
        assert!(validate_err(&cfg).contains("server.addr"));
    }

    #[test]
    fn unknown_key_source_is_rejected() {
        let mut cfg = happy();
        cfg.session.key_sources.push("cookie:session".into());
        let err = validate_err(&cfg);
        assert!(err.contains("session.key_sources[2]"), "got: {err}");
    }

    #[test]
    fn unknown_rollover_is_rejected() {
        let s = HAPPY.replace("\"rollover\": \"hourly\"", "\"rollover\": \"daily\"");
        let err = deser_err(&s);
        assert!(err.contains("rollover"), "got: {err}");
        assert!(err.contains("hourly"), "got: {err}");
    }

    #[test]
    fn process_plugin_requires_url() {
        let mut cfg = happy();
        cfg.plugins[1].url = None;
        let err = validate_err(&cfg);
        assert!(err.contains("process"), "got: {err}");
        assert!(err.contains("url"), "got: {err}");
    }

    #[test]
    fn unknown_builtin_and_service_slot_are_rejected() {
        let mut cfg = happy();
        cfg.plugins[0].kind = "builtin/typo_guard".into();
        assert!(validate_err(&cfg).contains("unknown builtin"));

        let mut cfg = happy();
        cfg.plugins[1].inject.push("someone_elses_table".into());
        assert!(validate_err(&cfg).contains("unknown service slot"));
    }

    #[test]
    fn route_strings_must_be_provider_slash_model() {
        assert!(parse_route("p/m").is_ok());
        assert!(parse_route("p").is_err());
        assert!(parse_route("p/").is_err());
        assert!(parse_route("a/b/c").is_err());
    }

    // -----------------------------------------------------------------------
    // spec §4.6: `account` + `plan_policy` (ADR-014)
    // -----------------------------------------------------------------------

    // The plan-first shape: one model family (`glm-5.3`) served by two accounts,
    // each account its own provider entry, plus a second subscription account
    // that no policy names. Built through serde_json like HAPPY, so router-core
    // still needs no YAML dependency (the shipped example's own parse is
    // asserted by router-cli, which owns file I/O and YAML).
    const PLANNED: &str = r#"{
        "server": {"addr": "127.0.0.1:8790", "upstream_attempt_timeout": "60s", "request_timeout": "10m"},
        "session": {"key_sources": ["prompt_cache_key"], "ttl": "12h"},
        "cache": {"sticky": true, "breakeven": {"enabled": true, "min_remaining_turns": 3, "safety_factor": 1.2}},
        "trace": {"dir": "./state/traces", "rollover": "hourly"},
        "providers": [
            {
                "name": "zai-plan",
                "urls": {"anthropic": "https://api.z.ai/api/anthropic/v1/messages"},
                "api_key_env": "ZAI_CODING_API_KEY",
                "wire_api": "anthropic",
                "supports": ["anthropic"],
                "account": "coding_plan",
                "models": [
                    {"id": "glm-5.3", "context": "200k",
                     "price": {"input_miss": 0.0014, "input_hit": 0.00026, "cache_write": 0.0,
                               "output": 0.0044, "peak": {"multiplier": 1.0, "windows": []}},
                     "source": "s"},
                    {"id": "glm-5.3-flash", "context": "200k",
                     "price": {"input_miss": 0.00015, "input_hit": 0.00003, "cache_write": 0.0,
                               "output": 0.0005, "peak": {"multiplier": 1.0, "windows": []}},
                     "source": "s"}
                ],
                "quota": [{"models": ["glm-5.3"], "window": "monthly", "tokens": 100000000,
                           "reset_day": 1, "over_quota": "block", "source": "s"}]
            },
            {
                "name": "zai",
                "urls": {"chat": "https://api.z.ai/api/paas/v4/chat/completions"},
                "api_key_env": "ZAI_API_KEY",
                "wire_api": "chat",
                "supports": ["chat"],
                "models": [
                    {"id": "glm-5.3", "context": "200k",
                     "price": {"input_miss": 0.0014, "input_hit": 0.00026, "cache_write": 0.0,
                               "output": 0.0044, "peak": {"multiplier": 1.0, "windows": []}},
                     "source": "s"},
                    {"id": "glm-5.3-flash", "context": "200k",
                     "price": {"input_miss": 0.00015, "input_hit": 0.00003, "cache_write": 0.0,
                               "output": 0.0005, "peak": {"multiplier": 1.0, "windows": []}},
                     "source": "s"}
                ]
            },
            {
                "name": "moonshot-plan",
                "urls": {"anthropic": "https://api.moonshot.ai/anthropic/v1/messages"},
                "api_key_env": "KIMI_CODING_API_KEY",
                "wire_api": "anthropic",
                "supports": ["anthropic"],
                "account": "coding_plan",
                "models": [
                    {"id": "kimi-k3", "context": "1m",
                     "price": {"input_miss": 0.003, "input_hit": 0.0003, "cache_write": 0.0,
                               "output": 0.015, "peak": {"multiplier": 1.0, "windows": []}},
                     "source": "s"}
                ],
                "quota": [{"models": ["kimi-k3"], "window": "monthly", "tokens": 50000000,
                           "reset_day": 1, "over_quota": "spill", "source": "s"}]
            },
            {
                "name": "moonshot",
                "urls": {"chat": "https://api.moonshot.ai/v1/chat/completions"},
                "api_key_env": "KIMI_API_KEY",
                "wire_api": "chat",
                "supports": ["chat"],
                "models": [
                    {"id": "kimi-k3", "context": "1m",
                     "price": {"input_miss": 0.003, "input_hit": 0.0003, "cache_write": 0.0,
                               "output": 0.015, "peak": {"multiplier": 1.0, "windows": []}},
                     "source": "s"}
                ]
            }
        ],
        "aliases": {},
        "plugins": [],
        "plan_policy": {"family": "glm-5.3", "primary": "zai-plan/glm-5.3", "overflow": "zai/glm-5.3"},
        "fallback": ["zai/glm-5.3"]
    }"#;

    fn planned() -> RouterConfig {
        let cfg: RouterConfig = serde_json::from_str(PLANNED).expect("planned config parses");
        cfg.validate().expect("planned config validates");
        cfg
    }

    /// Mutate the parsed policy (the cross-field checks are a function of the
    /// typed config, so a mutation states the illegal combination directly).
    fn planned_with(f: impl FnOnce(&mut PlanPolicyCfg)) -> RouterConfig {
        let mut cfg = planned();
        f(cfg.plan_policy.as_mut().expect("a policy"));
        cfg
    }

    // -------------------------------------------------------------
    // spec §4.8 (ADR-018): region / currency / family — the parsing
    // rules and the load-time checks, per surface and per default.
    // -------------------------------------------------------------

    /// Replace only the `n`-th occurrence of `from` (PLANNED has two
    /// providers serving the same model ids, so a global replace would
    /// patch both entries at once).
    fn replace_nth(s: &str, from: &str, to: &str, n: usize) -> String {
        let mut out = String::with_capacity(s.len());
        let mut rest = s;
        let mut seen = 0;
        while let Some(pos) = rest.find(from) {
            out.push_str(&rest[..pos]);
            out.push_str(if seen == n { to } else { from });
            rest = &rest[pos + from.len()..];
            seen += 1;
        }
        out.push_str(rest);
        assert!(
            seen > n,
            "occurrence {n} of {from:?} not found ({seen} seen)"
        );
        out
    }

    /// `"id": "glm-5.3", "context"` — occurs once per provider (0 =
    /// zai-plan, 1 = zai), the splice anchor for the model-level
    /// `family` patches.
    const GLM: &str = "\"id\": \"glm-5.3\", \"context\"";
    const FLASH: &str = "\"id\": \"glm-5.3-flash\", \"context\"";

    /// PLANNED with `patch` spliced into the zai provider ENTRY (the
    /// `"name": "zai",` line — `currency`/`region` are provider keys,
    /// §4.8), exercising the real deserializer + validate(). The anchor
    /// cannot match `"name": "zai-plan",` (the next char differs).
    fn zai_entry_patched(patch: &str) -> String {
        let anchor = "\"name\": \"zai\",";
        assert_eq!(
            PLANNED.matches(anchor).count(),
            1,
            "the zai anchor is unique"
        );
        PLANNED.replacen(anchor, &format!("{anchor} {patch},"), 1)
    }

    /// Drop the plan_policy line (the `both_keys_stay_optional` filter),
    /// so a provider-level check is asserted on its own message.
    fn without_policy(s: &str) -> String {
        s.lines()
            .filter(|l| !l.contains("\"plan_policy\""))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn currency_absent_means_usd_and_written_cny_is_per_entry() {
        // Absent ⇒ USD (§4.8): PLANNED writes no `currency` key anywhere.
        let cfg = planned();
        assert!(cfg.providers.iter().all(|p| p.currency == Currency::Usd));

        let cfg: RouterConfig =
            serde_json::from_str(&zai_entry_patched("\"currency\": \"CNY\"")).expect("parses");
        cfg.validate().expect("a CNY entry validates");
        assert_eq!(cfg.providers[1].currency, Currency::Cny);
        assert_eq!(
            cfg.providers[0].currency,
            Currency::Usd,
            "per-entry, not global"
        );
        let table = cfg.providers[1].models[0]
            .price
            .to_tier_tables(cfg.providers[1].currency)
            .unwrap();
        let table = &table[0].table;
        assert_eq!(table.currency, Currency::Cny, "the table carries its unit");
    }

    #[test]
    fn currency_is_exact_and_unknown_values_are_load_errors() {
        // Not a runtime fallback to USD: the load itself refuses. The
        // ISO-4217 code is exact — case included (§4.8).
        for bad in ["usd", "cny", "EUR", "Usd", "", "RMB"] {
            let err = deser_err(&zai_entry_patched(&format!("\"currency\": \"{bad}\"")));
            assert!(
                err.contains("unknown currency"),
                "value {bad:?} must be refused with 'unknown currency', got: {err}"
            );
        }
        // A non-string value is refused, not coerced.
        assert!(
            serde_json::from_str::<RouterConfig>(&zai_entry_patched("\"currency\": 4")).is_err()
        );
        // The message names the expected spellings (the operator's fix).
        assert!(
            deser_err(&zai_entry_patched("\"currency\": \"eur\"")).contains("USD or CNY"),
            "the refusal names the legal spellings"
        );
    }

    #[test]
    fn region_absent_means_intl_and_cn_does_not_derive_currency() {
        let cfg = planned();
        assert!(cfg.providers.iter().all(|p| p.region == Region::Intl));

        // cn + USD (the currency default) is legal: neither field
        // implies the other (§4.8 / ADR-018 §1). Constructs it and
        // proves it loads.
        let cfg: RouterConfig =
            serde_json::from_str(&zai_entry_patched("\"region\": \"cn\"")).expect("parses");
        cfg.validate().expect("cn region + USD billing is legal");
        assert_eq!(cfg.providers[1].region, Region::Cn);
        assert_eq!(cfg.providers[1].currency, Currency::Usd);
    }

    #[test]
    fn region_is_exact_and_unknown_values_are_load_errors() {
        for bad in ["CN", "Intl", "eu", "usa"] {
            let err = deser_err(&zai_entry_patched(&format!("\"region\": \"{bad}\"")));
            assert!(
                err.contains("unknown variant") && err.contains(bad),
                "region {bad:?} must be refused at load, got: {err}"
            );
        }
    }

    #[test]
    fn family_absent_means_the_models_own_id() {
        // §4.8's default is ADR-014's rule verbatim: PLANNED writes no
        // family key, and both routes' tags equal their model ids.
        let cfg = planned();
        let policy = cfg.plan_policy.as_ref().unwrap();
        assert_eq!(cfg.family_tag_of(&policy.primary).unwrap(), "glm-5.3");
        assert_eq!(cfg.family_tag_of(&policy.overflow).unwrap(), "glm-5.3");
    }

    #[test]
    fn family_tag_pairs_two_routes_and_the_policy_matches_the_tag() {
        // ADR-018 §4's motivating shape, shrunk: both entries carry tag
        // "fam" while their ids stay the route addresses, and the policy
        // names the tag. The quota coverage check is restated in terms
        // of the id the primary resolves to (zai-plan's quota.models
        // still names "glm-5.3" — and the config validates).
        let tagged = "\"id\": \"glm-5.3\", \"family\": \"fam\", \"context\"";
        let s = replace_nth(&replace_nth(PLANNED, GLM, tagged, 0), GLM, tagged, 0);
        let s = s.replace("\"family\": \"glm-5.3\"", "\"family\": \"fam\"");
        let cfg: RouterConfig = serde_json::from_str(&s).expect("tagged pair parses");
        cfg.validate()
            .expect("the policy matches the tag both entries carry");
        let policy = cfg.plan_policy.as_ref().unwrap();
        assert_eq!(cfg.family_tag_of(&policy.primary).unwrap(), "fam");
        assert_eq!(cfg.family_tag_of(&policy.overflow).unwrap(), "fam");
    }

    #[test]
    fn family_tags_may_not_collide_within_a_provider() {
        // Two WRITTEN tags colliding inside zai-plan (the order
        // 48d4341 already guarded): refused naming the key.
        let s = replace_nth(
            PLANNED,
            GLM,
            "\"id\": \"glm-5.3\", \"family\": \"fam\", \"context\"",
            0,
        );
        let s = replace_nth(
            &s,
            FLASH,
            "\"id\": \"glm-5.3-flash\", \"family\": \"fam\", \"context\"",
            0,
        );
        let cfg: RouterConfig = serde_json::from_str(&without_policy(&s)).expect("parses");
        let err = validate_err(&cfg);
        assert!(err.contains("family tag 'fam'"), "got: {err}");
        assert!(
            err.contains("models[1] (glm-5.3-flash).family"),
            "the error names the second entry's key, got: {err}"
        );
    }

    #[test]
    fn a_written_tag_may_not_shadow_a_later_entrys_defaulted_tag() {
        // Entry 0 writes family "glm-5.3-flash"; entry 1 writes nothing,
        // so its tag defaults to its id — the same string. The tag would
        // resolve to TWO models: a load error (§4.8). Found unguarded
        // while auditing 48d4341 (the check only ran for a written tag).
        let s = replace_nth(
            PLANNED,
            GLM,
            "\"id\": \"glm-5.3\", \"family\": \"glm-5.3-flash\", \"context\"",
            0,
        );
        let cfg: RouterConfig = serde_json::from_str(&without_policy(&s)).expect("parses");
        let err = validate_err(&cfg);
        assert!(
            err.contains("family tag 'glm-5.3-flash' is already carried"),
            "got: {err}"
        );
    }

    #[test]
    fn an_empty_family_tag_is_a_load_error() {
        // §4.8: the key is "optional non-empty string" — an empty tag
        // is neither the default (an absent key is) nor a usable name,
        // and must be refused at load naming the entry's key.
        let s = replace_nth(
            PLANNED,
            GLM,
            "\"id\": \"glm-5.3\", \"family\": \"\", \"context\"",
            0,
        );
        let cfg: RouterConfig = serde_json::from_str(&without_policy(&s)).expect("parses");
        let err = validate_err(&cfg);
        assert!(
            err.contains("family must be a non-empty string"),
            "got: {err}"
        );
        assert!(
            err.contains("models[0] (glm-5.3).family"),
            "the error names the entry's key, got: {err}"
        );
    }

    #[test]
    fn the_policys_family_must_equal_the_tag_both_routes_carry() {
        // Only the primary is tagged "fam" while the policy still names
        // the id "glm-5.3": a half-tagged family is not a family — the
        // load refuses (spec §4.6/§4.8) rather than route a policy
        // between routes it cannot recognize. The quota coverage check
        // stays in terms of the primary's model ID, so quota alone
        // would NOT have caught this.
        let s = replace_nth(
            PLANNED,
            GLM,
            "\"id\": \"glm-5.3\", \"family\": \"fam\", \"context\"",
            0,
        );
        let cfg: RouterConfig = serde_json::from_str(&s).expect("parses");
        let err = validate_err(&cfg);
        assert!(
            err.contains(
                "must equal the family tag plan_policy.primary's model entry carries ('fam')"
            ),
            "got: {err}"
        );
        assert!(
            err.contains("plan_policy.family"),
            "the error names the policy key, got: {err}"
        );

        // And the mirror: the overflow alone tagged is the same refusal
        // with the overflow's name in the message.
        let s = replace_nth(
            PLANNED,
            GLM,
            "\"id\": \"glm-5.3\", \"family\": \"fam\", \"context\"",
            1,
        );
        let cfg: RouterConfig = serde_json::from_str(&s).expect("parses");
        let err = validate_err(&cfg);
        assert!(
            err.contains(
                "must equal the family tag plan_policy.overflow's model entry carries ('fam')"
            ),
            "got: {err}"
        );
    }

    #[test]
    fn a_defaulted_tag_may_not_collide_with_a_later_written_tag() {
        // The remaining order of §4.8's uniqueness rule: entry 0 writes
        // nothing (tag = "glm-5.3"), entry 1 — a different model —
        // writes family "glm-5.3". The tag resolves to two models: a
        // load error, refused by the effective-tag comparison at the
        // second entry (the mirror of the shadow test above).
        let s = replace_nth(
            PLANNED,
            FLASH,
            "\"id\": \"glm-5.3-flash\", \"family\": \"glm-5.3\", \"context\"",
            0,
        );
        let cfg: RouterConfig = serde_json::from_str(&without_policy(&s)).expect("parses");
        let err = validate_err(&cfg);
        assert!(
            err.contains("family tag 'glm-5.3' is already carried"),
            "got: {err}"
        );
        assert!(
            err.contains("models[1] (glm-5.3-flash).family"),
            "the error names the second entry's key, got: {err}"
        );
    }

    #[test]
    fn plan_policy_parses_with_the_spec_defaults_and_the_account_flag() {
        let cfg = planned();
        let policy = cfg.plan_policy.as_ref().expect("the policy parsed");
        assert_eq!(policy.family, "glm-5.3");
        assert_eq!(policy.primary.to_string(), "zai-plan/glm-5.3");
        assert_eq!(policy.overflow.to_string(), "zai/glm-5.3");
        assert_eq!(policy.on_primary_exhausted, OnPrimaryExhausted::Spill); // §4.6 default
        assert_eq!(policy.recover, RecoveryMode::Probe); // §4.6 default
        assert_eq!(policy.cooldown, DurationVal(900_000)); // §4.6 default: 15m
        assert!(policy.overflow_monthly_cap_usd.is_none()); // absent = no cap

        assert_eq!(cfg.providers[0].account, AccountKind::CodingPlan);
        // An `api` entry written either way is the same account: declared, or
        // absent (the default — what today's roster already means).
        assert_eq!(cfg.providers[1].account, AccountKind::Api);
        assert_eq!(happy().providers[0].account, AccountKind::Api);
    }

    #[test]
    fn plan_policy_accepts_every_key_it_defines() {
        let s = PLANNED.replace(
            r#""overflow": "zai/glm-5.3"}"#,
            r#""overflow": "zai/glm-5.3", "on_primary_exhausted": "block", "recover": "none",
               "cooldown": "1h30m", "overflow_monthly_cap_usd": 20.0}"#,
        );
        let cfg: RouterConfig = serde_json::from_str(&s).expect("the full key set parses");
        cfg.validate().expect("the full key set validates");
        let policy = cfg.plan_policy.as_ref().unwrap();
        assert_eq!(policy.on_primary_exhausted, OnPrimaryExhausted::Block);
        assert_eq!(policy.recover, RecoveryMode::None);
        assert_eq!(policy.cooldown, DurationVal(5_400_000)); // 1h30m stacks
        assert_eq!(
            policy.overflow_monthly_cap_usd.unwrap().to_nano().unwrap(),
            Nano(20_000_000_000)
        );
    }

    #[test]
    fn both_keys_stay_optional() {
        // The same roster with the policy line removed: the keys land together
        // (GAP-Q15) but neither is required, and a subscription account no
        // policy names is a legal entry (§4.6).
        let s: String = PLANNED
            .lines()
            .filter(|l| !l.contains("\"plan_policy\""))
            .collect::<Vec<_>>()
            .join("\n");
        let cfg: RouterConfig = serde_json::from_str(&s).expect("parses without a policy");
        cfg.validate().expect("validates without a policy");
        assert!(cfg.plan_policy.is_none());
        assert_eq!(cfg.providers[2].account, AccountKind::CodingPlan);
    }

    #[test]
    fn unknown_account_value_is_refused_with_the_key_and_the_expected_words() {
        let s = PLANNED.replace(
            r#""account": "coding_plan""#,
            r#""account": "subscription""#,
        );
        let err = deser_err(&s);
        assert!(err.contains("subscription"), "got: {err}");
        assert!(err.contains("coding_plan"), "got: {err}");
        assert!(err.contains("account"), "got: {err}");
    }

    #[test]
    fn plan_policy_refuses_an_unknown_key() {
        let s = PLANNED.replace(
            r#""plan_policy": {"family""#,
            r#""plan_policy": {"prefer": "plan", "family""#,
        );
        let err = deser_err(&s);
        assert!(err.contains("unknown field"), "got: {err}");
        assert!(err.contains("prefer"), "got: {err}");
    }

    #[test]
    fn plan_policy_scalars_are_parsed_by_the_one_grammar() {
        // A duration is a duration everywhere: `15` is not one (§12.5).
        let s = PLANNED.replace(
            r#""overflow": "zai/glm-5.3"}"#,
            r#""overflow": "zai/glm-5.3", "cooldown": "15"}"#,
        );
        let err = deser_err(&s);
        assert!(err.contains("duration"), "got: {err}");

        // Two enums, each with exactly the words §4.6 lists.
        let s = PLANNED.replace(
            r#""overflow": "zai/glm-5.3"}"#,
            r#""overflow": "zai/glm-5.3", "on_primary_exhausted": "maybe"}"#,
        );
        let err = deser_err(&s);
        assert!(err.contains("maybe"), "got: {err}");

        let s = PLANNED.replace(
            r#""overflow": "zai/glm-5.3"}"#,
            r#""overflow": "zai/glm-5.3", "recover": "always"}"#,
        );
        let err = deser_err(&s);
        assert!(err.contains("always"), "got: {err}");
    }

    #[test]
    fn plan_routes_must_be_roster_routes() {
        let cfg = planned_with(|p| p.primary = parse_route("zai-plan/glm-9").unwrap());
        let err = validate_err(&cfg);
        assert!(err.contains("plan_policy.primary"), "got: {err}");
        assert!(err.contains("zai-plan/glm-9"), "got: {err}");

        let cfg = planned_with(|p| p.overflow = parse_route("nope/glm-5.3").unwrap());
        let err = validate_err(&cfg);
        assert!(err.contains("plan_policy.overflow"), "got: {err}");
        assert!(err.contains("nope/glm-5.3"), "got: {err}");
    }

    #[test]
    fn plan_routes_must_differ() {
        let cfg = planned_with(|p| p.overflow = p.primary.clone());
        let err = validate_err(&cfg);
        assert!(err.contains("plan_policy.overflow"), "got: {err}");
        assert!(err.contains("differ"), "got: {err}");
    }

    #[test]
    fn plan_primary_must_sit_on_a_coding_plan_account() {
        // A metered route as `primary`. The family checks are deliberately after
        // the account checks, so this is the message a reader gets.
        let cfg = planned_with(|p| p.primary = parse_route("moonshot/kimi-k3").unwrap());
        let err = validate_err(&cfg);
        assert!(err.contains("plan_policy.primary"), "got: {err}");
        assert!(err.contains("moonshot"), "got: {err}");
        assert!(err.contains("coding_plan"), "got: {err}");
    }

    #[test]
    fn plan_overflow_must_sit_on_an_api_account() {
        let cfg = planned_with(|p| p.overflow = parse_route("moonshot-plan/kimi-k3").unwrap());
        let err = validate_err(&cfg);
        assert!(err.contains("plan_policy.overflow"), "got: {err}");
        assert!(err.contains("moonshot-plan"), "got: {err}");
        assert!(err.contains("account: api"), "got: {err}");
    }

    #[test]
    fn plan_family_must_be_the_model_id_of_both_routes() {
        let cfg = planned_with(|p| p.family = "glm-5.3-flash".into());
        let err = validate_err(&cfg);
        assert!(err.contains("plan_policy.family"), "got: {err}");
        assert!(err.contains("plan_policy.primary"), "got: {err}");

        let cfg = planned_with(|p| p.overflow = parse_route("zai/glm-5.3-flash").unwrap());
        let err = validate_err(&cfg);
        assert!(err.contains("plan_policy.family"), "got: {err}");
        assert!(err.contains("plan_policy.overflow"), "got: {err}");
    }

    #[test]
    fn plan_family_must_be_covered_by_the_plans_own_quota() {
        // glm-5.3-flash is a model of the primary provider (so the route is
        // legal) but is not one of the plan's models.
        let cfg = planned_with(|p| {
            p.family = "glm-5.3-flash".into();
            p.primary = parse_route("zai-plan/glm-5.3-flash").unwrap();
            p.overflow = parse_route("zai/glm-5.3-flash").unwrap();
        });
        let err = validate_err(&cfg);
        assert!(err.contains("plan_policy.family"), "got: {err}");
        assert!(err.contains("quota.models"), "got: {err}");
        assert!(err.contains("glm-5.3"), "got: {err}");
    }

    #[test]
    fn plan_primary_needs_no_quota_at_all() {
        // GAP-Q1: the plan's allowance is often unpublished, so a subscription
        // account without a `quota` block is a legal primary — and it is the
        // case the policy exists for.
        let cfg = planned_with(|p| p.primary = parse_route("moonshot-plan/kimi-k3").unwrap());
        assert!(validate_err(&cfg).contains("plan_policy.primary")); // account mismatch first

        let mut cfg = planned();
        cfg.providers[0].quota = None; // zai-plan declares no plan any more
        cfg.validate()
            .expect("a plan without a quota block is legal");
    }

    #[test]
    fn plan_overflow_cap_must_be_a_readable_amount() {
        let cfg = planned_with(|p| p.overflow_monthly_cap_usd = Some(CapUsdVal(-1.0)));
        let err = validate_err(&cfg);
        assert!(
            err.contains("plan_policy.overflow_monthly_cap_usd"),
            "got: {err}"
        );
        assert!(err.contains(">= 0"), "got: {err}");

        let cfg = planned_with(|p| p.overflow_monthly_cap_usd = Some(CapUsdVal(f64::NAN)));
        let err = validate_err(&cfg);
        assert!(
            err.contains("plan_policy.overflow_monthly_cap_usd"),
            "got: {err}"
        );

        // 0 is a legal cap (a family that must never be served metered), and the
        // conversion is the one rounding, in nano-USD (ADR-006).
        let cfg = planned_with(|p| p.overflow_monthly_cap_usd = Some(CapUsdVal(0.0)));
        cfg.validate().expect("a zero cap is legal");
        assert_eq!(CapUsdVal(0.5).to_nano().unwrap(), Nano(500_000_000));
    }
}
