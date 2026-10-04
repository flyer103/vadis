//! CONF-53 (§12.8; spec §4.8 / ADR-018 §1): **the currency and region
//! keys parse exactly, default exactly, and refuse at load — not at
//! runtime.**
//!
//! Numbering note: DESIGN §12.8's ADR-018 row set allocates CONF-46…49,
//! but 46/47 landed on the R6 branch and 48–51 were reserved for the
//! parallel R8 cards; this round's cases therefore start at CONF-52
//! (the operator's allocation, recorded in §12.8 with this file).
//!
//! Asserted through the real `config_load::load` (YAML bytes →
//! validated config) and, for the illegal-currency half, through the
//! real `vadis_cli::serve` exit code:
//!
//! - (a) an entry that omits `currency` loads as USD and one that omits
//!   `region` loads as intl — the defaults are per-entry, never global;
//! - (b) `currency: CNY` and `region: cn` load, and **cn + USD (the
//!   currency default) loads too**: neither field derives the other
//!   (ADR-018 §1 — a CN-region entry billed in USD is legal);
//! - (c) an illegal `currency` (wrong case, unknown code, non-string)
//!   is a **load error** naming `providers[i].currency` with the legal
//!   spellings — and `serve` exits 2, nothing serves;
//! - (d) an illegal `region` is a load error the same way.
//!
//! The unit-level twins of these rules live in `vadis-core`'s config
//! tests; this case proves them through the file-and-YAML boundary the
//! operator actually writes.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// A minimal otherwise-valid config (the CONF-23 shape) with `{patch}`
/// spliced into the single provider entry.
fn config_with(patch: &str) -> String {
    format!(
        r#"
server:   {{ addr: "127.0.0.1:39723", upstream_attempt_timeout: 45s, request_timeout: 9m }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: only-provider
    {patch}
    urls:
      chat: https://only.example/v1/chat/completions
    api_key_env: CONF53_ONLY_KEY
    wire_api: chat
    supports: [chat]
    models:
      - id: only-model
        context: 128k
        price:
          input_miss: 0.001
          input_hit: 0.0001
          cache_write: 0.0
          output: 0.002
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "mock upstream (no price; test fixture)"

aliases: {{}}
plugins: []
fallback: []
"#
    )
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "conf53-{}-{}-{tag}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn load_with(tag: &str, patch: &str) -> Result<vadis_core::config::VadisConfig, String> {
    let dir = tempdir(tag);
    let path = dir.join("config.yaml");
    std::fs::write(&path, config_with(patch)).unwrap();
    vadis_cli::config_load::load(&path).map(|rc| rc.vadis)
}

// (a) absent ⇒ USD / intl, per entry.
#[test]
fn conf_53a_absent_keys_default_to_usd_and_intl() {
    let rc = load_with("default", "").expect("an entry with neither key loads");
    assert_eq!(rc.providers[0].currency, vadis_core::Currency::Usd);
    assert_eq!(rc.providers[0].region, vadis_core::config::Region::Intl);
}

// (b) written values load; cn + USD is legal (no derivation either way).
#[test]
fn conf_53b_written_values_load_and_cn_usd_is_legal() {
    let rc = load_with("cny", "currency: CNY").expect("currency: CNY loads");
    assert_eq!(rc.providers[0].currency, vadis_core::Currency::Cny);

    let rc = load_with("cn", "region: cn").expect("region: cn loads");
    assert_eq!(rc.providers[0].region, vadis_core::config::Region::Cn);
    // The load succeeded with no currency key: a CN-region entry billed
    // in USD (the default) is a legal configuration (ADR-018 §1).
    assert_eq!(rc.providers[0].currency, vadis_core::Currency::Usd);

    let rc = load_with("both", "region: cn\n    currency: CNY").expect("cn + CNY loads");
    assert_eq!(rc.providers[0].region, vadis_core::config::Region::Cn);
    assert_eq!(rc.providers[0].currency, vadis_core::Currency::Cny);
}

// (c) an illegal currency is a load error naming the key and the legal
// spellings.
#[test]
fn conf_53c_illegal_currency_is_a_load_error() {
    for bad in ["usd", "Cny", "EUR", "RMB", ""] {
        let err = load_with("bad-cur", &format!("currency: \"{bad}\""))
            .expect_err("an illegal currency must refuse the load");
        assert!(
            err.contains("unknown currency"),
            "value {bad:?} refused with the reason, got: {err}"
        );
        assert!(
            err.contains("USD or CNY"),
            "the reason names the legal spellings, got: {err}"
        );
    }
    // A non-string value is refused, not coerced.
    assert!(load_with("bad-type", "currency: 4").is_err());
}

// (c, serve half) Through the real serve assembly: exit code 2 (the
// config class, DESIGN §12.10.2) — the same refusal class CONF-23
// asserts for the store — and nothing serves.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_53c_illegal_currency_exits_2_nothing_serves() {
    let dir = tempdir("serve");
    let path = dir.join("config.yaml");
    std::fs::write(&path, config_with("currency: \"usd\"")).unwrap();
    let cfg = path.to_string_lossy().into_owned();
    let code = vadis_cli::serve(&cfg).await;
    assert_eq!(
        code, 2,
        "an illegal currency must exit 2 (2=config, 3=bind, 4=env), got {code}"
    );
}

// (d) an illegal region is a load error the same way.
#[test]
fn conf_53d_illegal_region_is_a_load_error() {
    for bad in ["CN", "INTL", "eu", "japan"] {
        let err = load_with("bad-reg", &format!("region: \"{bad}\""))
            .expect_err("an illegal region must refuse the load");
        assert!(
            err.contains("unknown variant") && err.contains(bad),
            "region {bad:?} refused with the value named, got: {err}"
        );
    }
}
