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
//! representable; everything downstream is integer `Price`/`NanoUsd`.

use std::collections::BTreeMap;
use std::fmt;
use std::net::SocketAddr;

use serde::de::{self, Deserializer, Visitor};
use serde::Deserialize;

use crate::cost::{Price, PriceTable};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DurationVal(pub u64);

/// Context limit in tokens (`131072`, `200k`, `1m` = 1,048,576).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextVal(pub u64);

/// Token count; accepts underscores (`100_000_000`) because YAML does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokensVal(pub u64);

/// `provider/model`, the one route form used by `aliases` and `fallback`.
#[derive(Debug, Clone, PartialEq, Eq)]
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
/// [`Price`] only through [`PriceCfg::to_price_table`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PriceVal(pub f64);

/// A peak/breakeven multiplier scalar (2.0 → 200 pct).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MultiplierVal(pub f64);

/// `HH:MM` minutes of day (`24:00` allowed as end of day).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MinutesVal(pub u16);

/// A weekday bitmask built from a `days:` list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeekdaySet(pub u8);

/// A time zone (`UTC` or `+08:00`-style).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerCfg {
    pub addr: String,
    pub upstream_attempt_timeout: DurationVal,
    pub request_timeout: DurationVal,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCfg {
    pub key_sources: Vec<String>,
    pub ttl: DurationVal,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BreakevenCfg {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub min_remaining_turns: u32,
    #[serde(default = "default_safety_factor")]
    pub safety_factor: MultiplierVal,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheCfg {
    #[serde(default = "default_true")]
    pub sticky: bool,
    pub breakeven: BreakevenCfg,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceCfg {
    pub dir: String,
    pub rollover: Rollover,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeakCfg {
    pub multiplier: MultiplierVal,
    #[serde(default)]
    pub windows: Vec<WindowCfg>,
}

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriceCfg {
    pub input_miss: PriceVal,
    pub input_hit: PriceVal,
    pub cache_write: PriceVal,
    pub output: PriceVal,
    pub peak: PeakCfg,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelCfg {
    pub id: String,
    pub context: ContextVal,
    pub price: PriceCfg,
    pub source: String,
}

/// `window: monthly` is the only window kind (DESIGN §12.4: any other value
/// is a load error).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuotaWindowTag {
    Monthly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OverQuotaTag {
    Block,
    Spill,
}

/// `reset_day`: 1..=31 (short months clamp at use time, `quota.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotaCfg {
    pub models: Vec<String>,
    pub window: QuotaWindowTag,
    pub tokens: TokensVal,
    pub reset_day: ResetDayVal,
    pub over_quota: OverQuotaTag,
    pub source: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCfg {
    pub name: String,
    pub base_url: String,
    pub api_key_env: String,
    pub wire_api: WireApi,
    pub supports: Vec<WireApi>,
    pub models: Vec<ModelCfg>,
    #[serde(default)]
    pub quota: Option<Vec<QuotaCfg>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterceptCfg {
    pub sample: f64,
    pub shadow: bool,
}

#[derive(Debug, Clone, Deserialize)]
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
#[derive(Debug, Clone, Deserialize)]
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
    #[serde(default)]
    pub state: Option<StateKeyForbidden>,
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
    /// Convert the four tiers + peak into the integer cost-engine table.
    /// `cache_write: 0` is legal (spec §4.0: the upstream charges no separate
    /// cache-write price); a zero in any other tier is refused rather than
    /// silently serving a free model.
    pub fn to_price_table(&self) -> Result<PriceTable, String> {
        const TIERS: [(&str, f64, bool); 4] = [
            ("input_miss", 0.0, false),
            ("input_hit", 0.0, false),
            ("cache_write", 0.0, true),
            ("output", 0.0, false),
        ];
        let mut vals = [0u64; 4];
        for (i, (name, v, allow_zero)) in TIERS.iter().enumerate() {
            let value = match i {
                0 => self.input_miss.0,
                1 => self.input_hit.0,
                2 => self.cache_write.0,
                _ => self.output.0,
            };
            let _ = v;
            if !value.is_finite() || value < 0.0 {
                return Err(format!(
                    "{name}: the price must be a finite non-negative number (got {value})"
                ));
            }
            let nano = price_to_nano(value);
            if nano == 0 && !allow_zero {
                return Err(format!(
                    "{name}: converts to 0 nano-USD per 1K tokens; refusing a silently-free \
                     tier (write an explicit positive price)"
                ));
            }
            vals[i] = nano;
        }
        Ok(PriceTable {
            input_miss: Price(vals[0]),
            input_hit: Price(vals[1]),
            cache_write: Price(vals[2]),
            output: Price(vals[3]),
            peak: self.peak.to_peak_table()?,
        })
    }
}

/// Tier-A builtin kinds compiled into the product (plus the three always
/// resident ones, which take no config entries). Adding a builtin means
/// adding it here — the whitelist discipline of `ROUTER_OWNED_TOP_LEVEL_KEYS`.
pub const KNOWN_BUILTIN_KINDS: [&str; 5] = [
    "cache_guard",
    "transform_rules",
    "cost_ledger",
    "quota_guard",
    "sticky",
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

    /// The §12.10.2 load-time validation table. Every failure names the key
    /// and the reason; there is no partially-started process.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.listen_addr()?;

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
                let mpath = format!("{ppath}.models[{mi}] ({})", m.id);
                m.price
                    .to_price_table()
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
            "base_url": "https://x.example/v1",
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
        let table = cfg.providers[0].models[0].price.to_price_table().unwrap();
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
        cfg.providers[0].models[0].price.input_miss = PriceVal(0.0);
        let err = validate_err(&cfg);
        assert!(err.contains("input_miss"), "got: {err}");
        assert!(err.contains("silently-free"), "got: {err}");
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
}
