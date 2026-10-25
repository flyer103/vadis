//! `GET /v1/models` (spec §4.19, ADR-055, ADR-061 D1, DESIGN §12.8): the
//! model list surface — the configured alias names, each with the context
//! window of the route it resolves to, and nothing else.
//!
//! Pure: no HTTP types, no clock, no RNG, no process state, no I/O — the
//! body is a function of the roster alone, so the same revision answers
//! byte-identical bodies forever (the byte-stability limb's guarantee).
//! The wiring (route, guard) is vadis-cli's, exactly as `state_read`'s is.
//!
//! The export boundary lives here too: the roster maps alias →
//! `provider/model`, and the body carries **only** the alias half plus one
//! number. The [`RouteSpec`] values are read to reach the route's roster
//! model entry (whose declared `context` is the entry's window, ADR-061
//! §2.2) — never serialized, never stringified — so a `provider/model`
//! string cannot reach the wire through this module.

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
///
/// `context_length` (ADR-061 D1, the fifth key, appended after
/// `owned_by`) is **the route's** declared window in tokens — the
/// `context` of the roster model entry the alias's route names, read
/// through [`route_context`]. An alias may not declare a window of its
/// own (§4.0's single-source discipline: one number, one place), so
/// repointing an alias changes the number a client reads, which is the
/// coupling the ruling wants.
pub fn models_body(config: &VadisConfig) -> Vec<u8> {
    // serde_json does the escaping — an alias name may carry quotes,
    // backslashes or non-ASCII, and `format!` would corrupt them. The
    // key order is the contract's own order, held by serde_json's
    // preserve_order map (the same mechanism health_json relies on) —
    // pinned byte-for-byte by the unit tests below.
    let data: Vec<serde_json::Value> = config
        .aliases
        .iter()
        .map(|(id, route)| {
            serde_json::json!({
                "id": id,
                "object": "model",
                "created": 0u64,
                "owned_by": "vadis",
                "context_length": route_context(config, route),
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
/// it exists so the handler reads one field, and so the config-level
/// spelling is unit-tested against the same expectations.
pub fn models_body_from_config(config: &VadisConfig) -> Vec<u8> {
    models_body(config)
}

/// The context window the route's own roster model entry declares
/// (`providers[i].models[j].context`, §12.5's scalar convention already
/// parsed it to a token count at load).
///
/// The `expect` is the load invariant, not a fallback: a configuration
/// that reaches the serving path has passed `VadisConfig::validate`,
/// which refuses an alias naming a route the roster does not hold
/// (ADR-061 §2's fact F2) — so the lookup can never miss here, and the
/// contract's "always present, never a default" cardinality holds by
/// construction rather than by a guess published as a number.
fn route_context(config: &VadisConfig, route: &RouteSpec) -> u64 {
    config
        .providers
        .iter()
        .find(|p| p.name == route.provider)
        .and_then(|p| p.models.iter().find(|m| m.id == route.model))
        .map(|m| m.context.0)
        .expect("a validated config resolves every alias's route (ADR-061 F2)")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A config with one provider `secret-provider` (a string no test's
    /// client-facing assertion names, so the export boundary's provider
    /// half is witnessed, not vacuous) holding the given models (id,
    /// context spelling) and the given aliases (name → route). The price
    /// block is inert here — `models_body` never reads it — but the
    /// parser requires it, so it is the minimal shape the roster's own
    /// tests use.
    fn config(models: &[(&str, &str)], aliases: &[(&str, &str)]) -> VadisConfig {
        let models: Vec<serde_json::Value> = models
            .iter()
            .map(|(id, context)| {
                serde_json::json!({
                    "id": id,
                    "context": context,
                    "price": {
                        "input_miss": 0.00066, "input_hit": 0.000022,
                        "cache_write": 0.0, "output": 0.00198,
                        "peak": {"multiplier": 2.0}
                    },
                    "source": "test",
                })
            })
            .collect();
        let aliases: serde_json::Map<String, serde_json::Value> = aliases
            .iter()
            .map(|(name, route)| (name.to_string(), serde_json::json!(route)))
            .collect();
        let cfg: VadisConfig = serde_json::from_value(serde_json::json!({
            "server": {"addr": "127.0.0.1:1", "upstream_attempt_timeout": "1s", "request_timeout": "1s"},
            "session": {"key_sources": ["prompt_cache_key"], "ttl": "11h"},
            "cache": {"sticky": true, "breakeven": {"enabled": true, "min_remaining_turns": 2, "safety_factor": 1.1}},
            "trace": {"dir": "./t", "rollover": "hourly"},
            "providers": [{
                "name": "secret-provider",
                "urls": {"chat": "https://x.example/v1/chat/completions"},
                "api_key_env": "P_KEY",
                "wire_api": "chat",
                "supports": ["chat"],
                "models": models,
            }],
            "aliases": aliases,
            "plugins": [],
            "fallback": [],
        }))
        .expect("a minimal config parses");
        cfg.validate().expect("a minimal config validates");
        cfg
    }

    /// The body of a three-alias roster, byte for byte: the exact key
    /// order (with `context_length` appended after `owned_by`), the
    /// exact constants, and the exact entry order.
    #[test]
    fn three_aliases_exact_bytes() {
        let cfg = config(
            &[("m0", "128k"), ("m1", "200k"), ("m2", "1m")],
            &[
                ("coding-fast", "secret-provider/m2"),
                ("chat-fast", "secret-provider/m0"),
                ("coding-bulk", "secret-provider/m1"),
            ],
        );
        let body = models_body(&cfg);
        assert_eq!(
            String::from_utf8(body).expect("utf-8"),
            "{\"object\":\"list\",\"data\":[\
             {\"id\":\"chat-fast\",\"object\":\"model\",\"created\":0,\"owned_by\":\"vadis\",\"context_length\":131072},\
             {\"id\":\"coding-bulk\",\"object\":\"model\",\"created\":0,\"owned_by\":\"vadis\",\"context_length\":204800},\
             {\"id\":\"coding-fast\",\"object\":\"model\",\"created\":0,\"owned_by\":\"vadis\",\"context_length\":1048576}]}",
            "the key order, the constants, the windows and the ascending id order are the contract"
        );
    }

    /// Five keys per entry, exactly — parsed, so a key change anywhere
    /// breaks this even if the byte string above was edited to match.
    #[test]
    fn every_entry_has_exactly_the_five_keys() {
        let cfg = config(&[("m", "200k")], &[("a", "secret-provider/m")]);
        let body = models_body(&cfg);
        let v: serde_json::Value = serde_json::from_slice(&body).expect("json");
        let entry = v["data"][0].as_object().expect("an object");
        let mut keys: Vec<&str> = entry.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["context_length", "created", "id", "object", "owned_by"]
        );
        assert_eq!(entry["object"], "model");
        assert_eq!(entry["created"].as_u64(), Some(0), "the constant 0");
        assert_eq!(entry["owned_by"], "vadis", "the constant literal");
        assert_eq!(
            entry["context_length"].as_u64(),
            Some(204800),
            "the route's declared context, as an integer token count"
        );
        assert!(
            entry["context_length"].is_u64(),
            "an integer on the wire — never a \"200k\" string"
        );
        let outer = v.as_object().expect("an object");
        let mut outer_keys: Vec<&str> = outer.keys().map(String::as_str).collect();
        outer_keys.sort_unstable();
        assert_eq!(outer_keys, vec!["data", "object"]);
        assert_eq!(outer["object"], "list");
    }

    /// The entire point of the fifth key (ADR-061 §2.3): an alias
    /// publishes **its route's** window, so two aliases on different
    /// routes answer different numbers, and the number follows the route
    /// — never a per-alias declaration (there is no such key).
    #[test]
    fn the_alias_publishes_the_routes_window() {
        let cfg = config(
            &[("small", "200k"), ("big", "1m")],
            &[
                ("cheap", "secret-provider/small"),
                ("roomy", "secret-provider/big"),
            ],
        );
        let v: serde_json::Value = serde_json::from_slice(&models_body(&cfg)).expect("json");
        let by_id = |id: &str| {
            v["data"]
                .as_array()
                .expect("data[]")
                .iter()
                .find(|e| e["id"] == id)
                .expect("the entry exists")
                .clone()
        };
        assert_eq!(by_id("cheap")["context_length"].as_u64(), Some(204800));
        assert_eq!(by_id("roomy")["context_length"].as_u64(), Some(1048576));
    }

    /// Ascending **byte** order, not lexical-case-insensitive or
    /// insertion order: `Z` (0x5A) sorts before `a` (0x61), and the
    /// insertion order here is the reverse of the answer's.
    #[test]
    fn order_is_byte_ascending_not_insertion() {
        let cfg = config(
            &[("m", "1")],
            &[
                ("b", "secret-provider/m"),
                ("a", "secret-provider/m"),
                ("Z", "secret-provider/m"),
            ],
        );
        let v: serde_json::Value = serde_json::from_slice(&models_body(&cfg)).expect("json");
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
        let cfg = config(&[], &[]);
        let body = models_body(&cfg);
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
        let cfg = config(&[("m", "1")], &[(tricky, "secret-provider/m")]);
        let body = models_body(&cfg);
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
    /// id, not as a member, not anywhere. The window is the only fact
    /// the route contributes, and it arrives as a number.
    #[test]
    fn no_route_string_in_the_body() {
        let cfg = config(
            &[("secret-model", "1")],
            &[("alias", "secret-provider/secret-model")],
        );
        let text = String::from_utf8(models_body(&cfg)).expect("utf-8");
        assert!(!text.contains("secret-provider"));
        assert!(
            !text.contains("secret-model"),
            "the native id travels only as the route lookup, never as a string"
        );
        assert!(
            !text.contains('/'),
            "no member is a provider/model route: {text}"
        );
    }

    /// Purity, observed as bytes: two calls over one roster are
    /// byte-identical, and a config whose maps are equal but distinct
    /// objects answers the same bytes too — the helper is pure, so the
    /// same inputs build a second config whose maps are equal to (and
    /// distinct from) the first's.
    #[test]
    fn two_calls_are_byte_identical() {
        let models = &[("m1", "1"), ("m2", "1k")];
        let aliases = &[("x", "secret-provider/m1"), ("y", "secret-provider/m2")];
        let cfg = config(models, aliases);
        let first = models_body(&cfg);
        let second = models_body(&cfg);
        assert_eq!(first, second);
        let other = config(models, aliases);
        assert!(
            !std::ptr::eq(&cfg.aliases, &other.aliases),
            "the two configs are distinct objects"
        );
        assert_eq!(first, models_body(&other));
        assert_eq!(first, models_body_from_config(&cfg));
    }

    /// The config-level seam answers what the builder does.
    #[test]
    fn config_seam_matches_builder() {
        let cfg = config(&[("m1", "1")], &[("fast", "secret-provider/m1")]);
        assert_eq!(models_body_from_config(&cfg), models_body(&cfg));
    }
}
