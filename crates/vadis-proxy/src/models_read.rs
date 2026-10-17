//! `GET /v1/models` (spec §4.19, ADR-055, DESIGN §12.8): the model list
//! surface — the configured alias names, and nothing else.
//!
//! Pure: no HTTP types, no clock, no RNG, no process state, no I/O — the
//! body is a function of the roster alone, so the same revision answers
//! byte-identical bodies forever (the byte-stability limb's guarantee).
//! The wiring (route, guard) is vadis-cli's, exactly as `state_read`'s is.
//!
//! The export boundary lives here too: the roster maps alias →
//! `provider/model`, and the body carries **only** the alias half. The
//! [`RouteSpec`] values are read for their type only — never serialized,
//! never stringified — so a `provider/model` string cannot reach the wire
//! through this module.

#![forbid(unsafe_code)]

use vadis_core::config::{RouteSpec, VadisConfig};

/// The response body for `GET /v1/models` (spec §4.19): one entry per
/// configured alias, ascending by id (`BTreeMap`'s own order — the map,
/// not a sort here, owns the ordering; a future switch to a `HashMap`
/// must re-establish it explicitly).
///
/// `created` is the constant `0` — never a clock read: a wall-clock
/// stamp would make the body differ call-to-call and break both the
/// byte-stability contract and every byte-level consumer. `owned_by` is
/// the constant literal `"vadis"`: the gateway owns this namespace.
pub fn models_body(aliases: &std::collections::BTreeMap<String, RouteSpec>) -> Vec<u8> {
    // serde_json does the escaping — an alias name may carry quotes,
    // backslashes or non-ASCII, and `format!` would corrupt them. The
    // key order is the contract's own order, held by serde_json's
    // preserve_order map (the same mechanism health_json relies on) —
    // pinned byte-for-byte by the unit tests below.
    let data: Vec<serde_json::Value> = aliases
        .keys()
        .map(|id| {
            serde_json::json!({
                "id": id,
                "object": "model",
                "created": 0u64,
                "owned_by": "vadis",
            })
        })
        .collect();
    serde_json::to_vec(&serde_json::json!({
        "object": "list",
        "data": data,
    }))
    .expect("a Vec of strings always serializes")
}

/// The same bytes from a whole config — the convenience seam the serving
/// path uses (`revision.forwarder.config`). Delegates to [`models_body`];
/// it exists so the handler reads one field, not the map, and so the
/// config-level spelling is unit-tested against the same expectations.
pub fn models_body_from_config(config: &VadisConfig) -> Vec<u8> {
    models_body(&config.aliases)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn roster(pairs: &[(&str, &str)]) -> BTreeMap<String, RouteSpec> {
        pairs
            .iter()
            .map(|(alias, route)| {
                (
                    alias.to_string(),
                    serde_json::from_value(serde_json::Value::String(route.to_string()))
                        .expect("a route parses"),
                )
            })
            .collect()
    }

    /// The body of a three-alias roster, byte for byte: the exact key
    /// order, the exact constants, and the exact entry order.
    #[test]
    fn three_aliases_exact_bytes() {
        let body = models_body(&roster(&[
            ("coding-fast", "p/m1"),
            ("chat-fast", "p/m0"),
            ("coding-bulk", "p/m2"),
        ]));
        assert_eq!(
            String::from_utf8(body).expect("utf-8"),
            "{\"object\":\"list\",\"data\":[\
             {\"id\":\"chat-fast\",\"object\":\"model\",\"created\":0,\"owned_by\":\"vadis\"},\
             {\"id\":\"coding-bulk\",\"object\":\"model\",\"created\":0,\"owned_by\":\"vadis\"},\
             {\"id\":\"coding-fast\",\"object\":\"model\",\"created\":0,\"owned_by\":\"vadis\"}]}",
            "the key order, the constants and the ascending id order are the contract"
        );
    }

    /// Four keys per entry, exactly — parsed, so a key change anywhere
    /// breaks this even if the byte string above was edited to match.
    #[test]
    fn every_entry_has_exactly_the_four_keys() {
        let body = models_body(&roster(&[("a", "p/m")]));
        let v: serde_json::Value = serde_json::from_slice(&body).expect("json");
        let entry = v["data"][0].as_object().expect("an object");
        let mut keys: Vec<&str> = entry.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["created", "id", "object", "owned_by"]);
        assert_eq!(entry["object"], "model");
        assert_eq!(entry["created"].as_u64(), Some(0), "the constant 0");
        assert_eq!(entry["owned_by"], "vadis", "the constant literal");
        let outer = v.as_object().expect("an object");
        let mut outer_keys: Vec<&str> = outer.keys().map(String::as_str).collect();
        outer_keys.sort_unstable();
        assert_eq!(outer_keys, vec!["data", "object"]);
        assert_eq!(outer["object"], "list");
    }

    /// Ascending **byte** order, not lexical-case-insensitive or
    /// insertion order: `Z` (0x5A) sorts before `a` (0x61), and the
    /// insertion order here is the reverse of the answer's.
    #[test]
    fn order_is_byte_ascending_not_insertion() {
        let body = models_body(&roster(&[("b", "p/m"), ("a", "p/m"), ("Z", "p/m")]));
        let v: serde_json::Value = serde_json::from_slice(&body).expect("json");
        let ids: Vec<&str> = v["data"]
            .as_array()
            .expect("data[]")
            .iter()
            .map(|e| e["id"].as_str().expect("a string id"))
            .collect();
        assert_eq!(ids, vec!["Z", "a", "b"], "byte-ascending (BTreeMap order)");
    }

    /// The empty roster: `200`'s body is the empty list — not an error,
    /// not `null`, not an absent `data`.
    #[test]
    fn empty_roster_is_the_empty_list() {
        let body = models_body(&BTreeMap::new());
        assert_eq!(
            String::from_utf8(body).expect("utf-8"),
            "{\"object\":\"list\",\"data\":[]}"
        );
    }

    /// Alias names that need JSON escaping come out correctly escaped —
    /// the reason the body is built by serde_json, never `format!`.
    #[test]
    fn escaping_is_serde_jsons() {
        let tricky = "quote\"back\\slash\ttab";
        let body = models_body(&roster(&[(tricky, "p/m")]));
        let v: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(
            v["data"][0]["id"].as_str().expect("round-trips"),
            tricky,
            "the name survives the wire escape/unescape round trip"
        );
        let text = String::from_utf8(body).expect("utf-8");
        assert!(
            !text.contains("\"quote\""),
            "the raw quote is escaped: {text}"
        );
    }

    /// The route half of the roster never reaches the body — not as an
    /// id, not as a member, not anywhere.
    #[test]
    fn no_route_string_in_the_body() {
        let body = models_body(&roster(&[("alias", "secret-provider/secret-model")]));
        let text = String::from_utf8(body).expect("utf-8");
        assert!(!text.contains("secret-provider"));
        assert!(!text.contains("secret-model"));
        assert!(!text.contains('/'), "no id is a provider/model route");
    }

    /// Purity, observed as bytes: two calls over one roster are
    /// byte-identical, and a cloned roster (equal, different object)
    /// answers the same bytes too.
    #[test]
    fn two_calls_are_byte_identical() {
        let roster = roster(&[("x", "p/m"), ("y", "p/m2")]);
        let first = models_body(&roster);
        let second = models_body(&roster);
        assert_eq!(first, second);
        assert_eq!(first, models_body(&roster.clone()));
    }

    /// The config-level seam answers what the map-level one does.
    #[test]
    fn config_seam_matches_map_body() {
        let config: VadisConfig = serde_json::from_value(serde_json::json!({
            "server": {"addr": "127.0.0.1:1", "upstream_attempt_timeout": "1s", "request_timeout": "1s"},
            "session": {"key_sources": ["prompt_cache_key"], "ttl": "11h"},
            "cache": {"sticky": true, "breakeven": {"enabled": true, "min_remaining_turns": 2, "safety_factor": 1.1}},
            "trace": {"dir": "./t", "rollover": "hourly"},
            "providers": [],
            "aliases": {"fast": "p/m1"},
            "plugins": [],
            "fallback": []
        }))
        .expect("a minimal config parses");
        assert_eq!(
            models_body_from_config(&config),
            models_body(&config.aliases)
        );
    }
}
