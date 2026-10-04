//! The reload's `changed_keys` value diff (ADR-040 D10; DESIGN §12.10.5
//! note R10): the diff runs over the **validated, joined** configuration —
//! the value the process serves, `VadisConfig::providers` staying the only
//! representation (ADR-037 D4) — **never over file text**. A comment-only
//! edit moves the byte digest (spec §4.14) and produces an **empty** list
//! here: the digest is a byte measurement, this list is a value
//! measurement, and the comment edit is the case that separates them
//! (RV-8).
//!
//! The mechanics: both revisions are serialized to `serde_json::Value` and
//! diffed structurally. Only `{path, change}` pairs are ever emitted —
//! **values never leave this module** — so the serialization form of a
//! leaf (a duration as milliseconds, an enum as its tag) is irrelevant:
//! what matters is structure and equality alone. This is not a second
//! serializer of anything served (ADR-025's refusal concerns the outbound
//! request bytes; nothing here reaches the wire, and D7's no-normalization
//! rule concerns what is *published*, which stays the operator's own
//! bytes).
//!
//! The path grammar (D10): object members join with `.` (`session.ttl`);
//! the three arrays the loader guarantees a unique identity for are named
//! by it — `providers` by `name`, `providers[x].models` by `id`, `plugins`
//! by `id`; every other array is named by 0-based index (`fallback[0]`).
//! The descent stops at the deepest path where the two values differ: an
//! added or removed subtree is one entry, its leaves not enumerated; a
//! renamed model id is therefore two entries (one `removed`, one
//! `added`). The list is sorted by the path string, so two processes
//! applying the same pair write byte-identical payloads (AGENTS 2 binds
//! observation as much as content).

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::Value;

use crate::config::VadisConfig;

/// The payload's own vocabulary (D10): deliberately not row 12's
/// `loaded`/`unloaded`, which name an operation the runtime performed on a
/// plugin — this diff names a fact about two values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    /// Present in the applied revision, absent from its predecessor.
    Added,
    /// Present in both, unequal.
    Changed,
    /// Present in the predecessor, absent from the applied revision.
    Removed,
}

/// One entry of a row 13 `changed_keys` member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeyChange {
    pub path: String,
    pub change: ChangeKind,
}

/// The value diff between the revision being replaced and the revision
/// being applied (D10). `Err` only if a config value cannot be serialized
/// for comparison (a non-finite float is the one reachable case) — the
/// caller treats that as a refusal of the switch, never a panic and never
/// a silently empty list.
pub fn changed_keys(old: &VadisConfig, new: &VadisConfig) -> Result<Vec<KeyChange>, String> {
    let old_v = serde_json::to_value(old)
        .map_err(|e| format!("the serving revision cannot be compared: {e}"))?;
    let new_v =
        serde_json::to_value(new).map_err(|e| format!("the candidate cannot be compared: {e}"))?;
    let mut out = Vec::new();
    diff_value("", &old_v, &new_v, &mut out);
    // Sorted by the path string, lexicographically (D10): the payload is a
    // pure function of (predecessor, applied), so an assertion may compare
    // arrays without sorting them first.
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// Object-member join: `"" + "server"` is `server`, `"providers[p]" +
/// "models"` is `providers[p].models`.
fn join_member(path: &str, member: &str) -> String {
    if path.is_empty() {
        member.to_string()
    } else {
        format!("{path}.{member}")
    }
}

fn push(out: &mut Vec<KeyChange>, path: String, change: ChangeKind) {
    out.push(KeyChange { path, change });
}

fn diff_value(path: &str, old: &Value, new: &Value, out: &mut Vec<KeyChange>) {
    if old == new {
        return;
    }
    match (old, new) {
        (Value::Object(o), Value::Object(n)) => {
            // The union key space, ordered (BTreeSet) so the walk itself
            // is deterministic before the final sort.
            let keys: BTreeSet<&String> = o.keys().chain(n.keys()).collect();
            for k in keys {
                let sub = join_member(path, k);
                match (o.get(k), n.get(k)) {
                    (None, Some(_)) => push(out, sub, ChangeKind::Added),
                    (Some(_), None) => push(out, sub, ChangeKind::Removed),
                    (Some(a), Some(b)) => diff_value(&sub, a, b, out),
                    (None, None) => unreachable!("the key came from the union"),
                }
            }
        }
        (Value::Array(o), Value::Array(n)) => diff_array(path, o, n, out),
        // A scalar difference, or a type change: the descent stops here
        // (D10 — a path whose subtree changed type is carried whole).
        _ => push(out, path.to_string(), ChangeKind::Changed),
    }
}

fn diff_array(path: &str, old: &[Value], new: &[Value], out: &mut Vec<KeyChange>) {
    match identity_key_for(path).and_then(|key| keyed_arrays(key, old, new)) {
        Some((old_map, new_map)) => {
            let keys: BTreeSet<&String> = old_map.keys().chain(new_map.keys()).collect();
            for k in keys {
                let sub = format!("{path}[{k}]");
                match (old_map.get(k), new_map.get(k)) {
                    (None, Some(_)) => push(out, sub, ChangeKind::Added),
                    (Some(_), None) => push(out, sub, ChangeKind::Removed),
                    (Some(a), Some(b)) => diff_value(&sub, a, b, out),
                    (None, None) => unreachable!("the key came from the union"),
                }
            }
        }
        // Positional: every other array is named by 0-based index (D10).
        None => {
            for i in 0..old.len().max(new.len()) {
                let sub = format!("{path}[{i}]");
                match (old.get(i), new.get(i)) {
                    (None, Some(_)) => push(out, sub, ChangeKind::Added),
                    (Some(_), None) => push(out, sub, ChangeKind::Removed),
                    (Some(a), Some(b)) => diff_value(&sub, a, b, out),
                    (None, None) => unreachable!("the index came from the union"),
                }
            }
        }
    }
}

/// The identity key an array is diffed by, where its elements carry one
/// the loader guarantees unique (§12.10.2's duplicate checks; D10's three
/// named identities). `providers[x].models` matches **exactly** one
/// bracket pair — `providers[x].quota[i].models` (a `Vec<String>`) is
/// positional like every other array.
fn identity_key_for(path: &str) -> Option<&'static str> {
    if path == "providers" {
        return Some("name");
    }
    if path == "plugins" {
        return Some("id");
    }
    if let Some(mid) = path
        .strip_prefix("providers[")
        .and_then(|r| r.strip_suffix("].models"))
    {
        if !mid.contains(['[', ']']) {
            return Some("id");
        }
    }
    None
}

/// One array keyed by an identity field: the element's own value for the
/// key, beside the element.
type KeyedArray<'a> = BTreeMap<String, &'a Value>;

/// Both arrays keyed by `key` — or `None` (the caller falls back to the
/// positional walk) when any element lacks a string value for the key or a
/// key repeats. A validated config cannot produce either (the loader
/// refuses duplicates), so the fallback is belt-and-braces, never the plan.
fn keyed_arrays<'a>(
    key: &str,
    old: &'a [Value],
    new: &'a [Value],
) -> Option<(KeyedArray<'a>, KeyedArray<'a>)> {
    let collect = |arr: &'a [Value]| {
        let mut map = BTreeMap::new();
        for v in arr {
            let k = v.get(key)?.as_str()?.to_string();
            if map.insert(k.clone(), v).is_some() {
                return None; // a duplicate identity: not the keyed shape
            }
        }
        Some(map)
    };
    Some((collect(old)?, collect(new)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixtures are JSON text parsed through the same `Deserialize` impls
    // the YAML boundary uses (the scalar conventions are
    // format-independent by construction — config.rs's own test module's
    // pattern), so vadis-core needs no YAML dependency.
    const BASE: &str = r#"{
        "server": {"addr": "127.0.0.1:8790", "upstream_attempt_timeout": "60s", "request_timeout": "10m"},
        "session": {"key_sources": ["prompt_cache_key"], "ttl": "12h"},
        "cache": {"sticky": true, "breakeven": {"enabled": true, "min_remaining_turns": 3, "safety_factor": 1.2}},
        "trace": {"dir": "./state/traces", "rollover": "hourly"},
        "providers": [{
            "name": "p",
            "urls": {"chat": "https://x.example/v1/chat/completions"},
            "api_key_env": "P_KEY",
            "wire_api": "chat",
            "supports": ["chat"],
            "models": [{
                "id": "m1", "context": "200k",
                "price": {"input_miss": 0.00066, "input_hit": 0.000022, "cache_write": 0.0, "output": 0.00198,
                          "peak": {"multiplier": 1.0, "windows": []}},
                "source": "https://x.example/pricing @2026-09-19"
            }]
        }],
        "aliases": {},
        "plugins": [],
        "fallback": []
    }"#;

    fn cfg(json: &str) -> VadisConfig {
        let v: Value = serde_json::from_str(json).unwrap();
        let c: VadisConfig = serde_json::from_value(v).expect("the fixture parses");
        c.validate().expect("the fixture validates");
        c
    }

    fn paths(changes: &[KeyChange]) -> Vec<(&str, ChangeKind)> {
        changes
            .iter()
            .map(|c| (c.path.as_str(), c.change))
            .collect()
    }

    /// RV-8's middle state at the value level: two loads whose **values**
    /// are equal produce the empty list — the comment-only case separates
    /// the byte digest (which moves) from the value diff (which does not).
    /// The end-to-end half (the byte digest actually moving on a comment
    /// edit) is the loader's own test and the reload's rig.
    #[test]
    fn equal_values_diff_to_nothing() {
        let a = cfg(BASE);
        let b = cfg(BASE);
        assert_eq!(changed_keys(&a, &b).unwrap(), vec![]);
        // And through the serialized boundary explicitly: a reordered-key
        // document of the same values is still no change (the diff is over
        // values, never over text layout).
        let reordered = BASE.replace("\"ttl\": \"12h\"}", "\"ttl\": \"12h\"}");
        assert_eq!(changed_keys(&a, &cfg(&reordered)).unwrap(), vec![]);
    }

    /// RV-8's third state: a revision that moves exactly one key lists
    /// exactly that path.
    #[test]
    fn a_single_scalar_change_lists_exactly_its_path() {
        let a = cfg(BASE);
        let b = cfg(&BASE.replace("\"ttl\": \"12h\"", "\"ttl\": \"6h\""));
        let changes = changed_keys(&a, &b).unwrap();
        assert_eq!(
            paths(&changes),
            vec![("session.ttl", ChangeKind::Changed)],
            "got: {changes:?}"
        );
    }

    /// The identity-keyed grammar (D10): a model's price member is named
    /// by the provider's `name` and the model's `id`, never by index.
    #[test]
    fn provider_and_model_arrays_are_keyed_by_identity() {
        let a = cfg(BASE);
        let b = cfg(&BASE.replace("\"input_miss\": 0.00066", "\"input_miss\": 0.00077"));
        let changes = changed_keys(&a, &b).unwrap();
        assert_eq!(
            paths(&changes),
            vec![(
                "providers[p].models[m1].price.input_miss",
                ChangeKind::Changed
            )],
            "got: {changes:?}"
        );
    }

    /// An added provider is ONE entry at the subtree root — its leaves
    /// are not enumerated (D10's stop rule); a rename is one removal and
    /// one addition, honest and deterministic.
    #[test]
    fn an_added_subtree_is_one_entry_and_a_rename_is_two() {
        let a = cfg(BASE);
        let with_second = BASE.replace(
            "\"id\": \"m1\", \"context\": \"200k\"",
            "\"id\": \"m2\", \"context\": \"200k\"",
        );
        let changes = changed_keys(&a, &cfg(&with_second)).unwrap();
        assert_eq!(
            paths(&changes),
            vec![
                ("providers[p].models[m1]", ChangeKind::Removed),
                ("providers[p].models[m2]", ChangeKind::Added),
            ],
            "a rename is the removed id and the added id, sorted: {changes:?}"
        );
    }

    /// Every other array is positional: `fallback` (a `RouteSpec` list)
    /// names its elements by 0-based index, and `quota`'s own `models`
    /// string list is NOT the identity-keyed `providers[x].models`.
    #[test]
    fn non_identity_arrays_are_positional_including_quota_models() {
        let a = cfg(BASE);
        let with_fallback = BASE.replace("\"fallback\": []", "\"fallback\": [\"p/m1\"]");
        let changes = changed_keys(&a, &cfg(&with_fallback)).unwrap();
        assert_eq!(
            paths(&changes),
            vec![("fallback[0]", ChangeKind::Added)],
            "got: {changes:?}"
        );

        // A provider carrying both m1 and m2, so the quota may name
        // either (the loader refuses a quota reference to a model the
        // provider does not carry).
        let two_models = BASE.replace(
            "\"source\": \"https://x.example/pricing @2026-09-19\"\n            }]",
            "\"source\": \"https://x.example/pricing @2026-09-19\"}, {
                \"id\": \"m2\", \"context\": \"200k\",
                \"price\": {\"input_miss\": 0.00066, \"input_hit\": 0.000022, \"cache_write\": 0.0, \"output\": 0.00198,
                          \"peak\": {\"multiplier\": 1.0, \"windows\": []}},
                \"source\": \"https://x.example/pricing @2026-09-19\"
            }]",
        );
        let quota = |models: &str| {
            two_models.replace(
                "\"api_key_env\": \"P_KEY\",",
                &format!(
                    "\"api_key_env\": \"P_KEY\",\n            \"quota\": [{{\"models\": {models}, \"window\": \"monthly\", \"tokens\": 1000, \"reset_day\": 1, \"over_quota\": \"block\", \"source\": \"https://x.example/quota @2026-09-19\"}}],"
                ),
            )
        };
        // The positional arm that matters: quota's own `models` list is a
        // Vec<String> — the SAME two members reordered are reported
        // positionally, where the identity-keyed `providers[x].models`
        // walk would have reported nothing (order is content in a Vec).
        let c = cfg(&quota("[\"m1\", \"m2\"]"));
        let d = cfg(&quota("[\"m2\", \"m1\"]"));
        let changes = changed_keys(&c, &d).unwrap();
        assert_eq!(
            paths(&changes),
            vec![
                ("providers[p].quota[0].models[0]", ChangeKind::Changed),
                ("providers[p].quota[0].models[1]", ChangeKind::Changed),
            ],
            "quota's models list is positional, not identity-keyed: {changes:?}"
        );
    }

    /// The list is sorted by path string, lexicographically, however many
    /// members moved (D10: a pure function of the two revisions).
    #[test]
    fn the_list_is_sorted_by_path() {
        let a = cfg(BASE);
        let b = cfg(&BASE
            .replace("\"ttl\": \"12h\"", "\"ttl\": \"6h\"")
            .replace("\"sticky\": true", "\"sticky\": false")
            .replace(
                "\"addr\": \"127.0.0.1:8790\"",
                "\"addr\": \"127.0.0.1:9700\"",
            ));
        let changes = changed_keys(&a, &b).unwrap();
        let path_strings: Vec<&str> = changes.iter().map(|c| c.path.as_str()).collect();
        let mut sorted = path_strings.clone();
        sorted.sort_unstable();
        assert_eq!(path_strings, sorted, "sorted lexicographically by path");
        assert_eq!(
            paths(&changes),
            vec![
                ("cache.sticky", ChangeKind::Changed),
                ("server.addr", ChangeKind::Changed),
                ("session.ttl", ChangeKind::Changed),
            ],
            "got: {changes:?}"
        );
        // A pure function: the same pair diffs to the same list twice.
        assert_eq!(changed_keys(&a, &b).unwrap(), changes);
    }

    /// The `plugins` array is keyed by `id` (D10's third identity).
    #[test]
    fn plugins_are_keyed_by_id() {
        let plugin = |id: &str, disabled: bool| {
            BASE.replace(
                "\"plugins\": []",
                &format!(
                    "\"plugins\": [{{\"id\": \"{id}\", \"kind\": \"builtin/transform_rules\", \"disabled\": {disabled}}}]"
                ),
            )
        };
        let a = cfg(&plugin("transform_rules", false));
        let b = cfg(&plugin("transform_rules", true));
        let changes = changed_keys(&a, &b).unwrap();
        assert_eq!(
            paths(&changes),
            vec![("plugins[transform_rules].disabled", ChangeKind::Changed)],
            "got: {changes:?}"
        );
    }
}
