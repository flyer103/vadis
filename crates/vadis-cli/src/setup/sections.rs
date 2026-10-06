//! setup/sections.rs — the section table: the single place the wizard's
//! key set is written (DESIGN §12.14; spec §4.11's section table).
//!
//! Seven sections, in the order spec §4.11's table lists them. `Show` keys
//! are display-only (vendor facts and membership edits are never prompted
//! and never written — ADR-025 decisions 4/7); every value key's default
//! comes from the file, else the template, never from a constant in code.

use crate::setup::edit::EditKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Server,
    Auth,
    Session,
    Paths,
    Providers,
    Routing,
    Plugins,
}

impl Section {
    pub fn name(self) -> &'static str {
        match self {
            Section::Server => "server",
            Section::Auth => "auth",
            Section::Session => "session",
            Section::Paths => "paths",
            Section::Providers => "providers",
            Section::Routing => "routing",
            Section::Plugins => "plugins",
        }
    }

    pub fn parse(s: &str) -> Option<Section> {
        Some(match s {
            "all" | "server" => Section::Server,
            "auth" => Section::Auth,
            "session" => Section::Session,
            "paths" => Section::Paths,
            "providers" => Section::Providers,
            "routing" => Section::Routing,
            "plugins" => Section::Plugins,
            _ => return None,
        })
    }
}

/// All seven, in spec §4.11's table order (DESIGN §12.14 step 3).
pub const ALL: [Section; 7] = [
    Section::Server,
    Section::Auth,
    Section::Session,
    Section::Paths,
    Section::Providers,
    Section::Routing,
    Section::Plugins,
];

/// Which of the pair's files a section's keys are written in — spec
/// §4.11's *target file* column, as explicit data (ADR-037 D9). Six
/// sections own keys of the root config; `providers` owns keys of the
/// roster. Under an **inline** root the roster *is* the root and the
/// column collapses to `Root` for every section — today's behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetFile {
    /// The root config file — the one §4.12's discovery order finds.
    Root,
    /// The roster file the root names with `providers_file:` (§4.14).
    Roster,
}

impl TargetFile {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetFile::Root => "root",
            TargetFile::Roster => "roster",
        }
    }
}

impl Section {
    /// The file this section's anchors are resolved against and its edits
    /// land in, given the root's shape. The mapping is a property of the
    /// **section**, stated once here — the edit path carries no special
    /// case for `providers`.
    pub fn target(self, split: bool) -> TargetFile {
        match (self, split) {
            (Section::Providers, true) => TargetFile::Roster,
            _ => TargetFile::Root,
        }
    }
}

/// What a question is: a free line, one of an enum, a boolean, or a
/// display-only entry (never prompted, never written).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ask {
    Line,
    Enum(&'static [&'static str]),
    Bool,
    Show,
}

/// One askable (or display-only) key of the wizard's table.
#[derive(Debug, Clone)]
pub struct KeySpec {
    pub section: Section,
    /// A key path in the anchor grammar; the static rows are unit-asserted
    /// to resolve in `config.example.yaml` (the table and the example
    /// cannot drift — DESIGN §12.14's rig).
    pub path: &'static str,
    pub kind: EditKind,
    pub ask: Ask,
    /// For `Show` keys: the "edit by hand" line printed beside the value.
    pub note: &'static str,
}

impl KeySpec {
    /// A duration-valued row (the answer is validated against the §12.5
    /// grammar before it is encoded — spec §4.11 step 2).
    pub fn is_duration(&self) -> bool {
        matches!(
            self.path,
            "server.upstream_attempt_timeout"
                | "server.request_timeout"
                | "session.ttl"
                | "plan_policy.cooldown"
        )
    }
}

/// Value rows that are always present (their anchors are fixed paths).
const STATIC_ROWS: &[KeySpec] = &[
    // server
    KeySpec {
        section: Section::Server,
        path: "server.addr",
        kind: EditKind::SetValue,
        ask: Ask::Line,
        note: "",
    },
    KeySpec {
        section: Section::Server,
        path: "server.upstream_attempt_timeout",
        kind: EditKind::SetValue,
        ask: Ask::Line,
        note: "",
    },
    KeySpec {
        section: Section::Server,
        path: "server.request_timeout",
        kind: EditKind::SetValue,
        ask: Ask::Line,
        note: "",
    },
    // auth
    KeySpec {
        section: Section::Auth,
        path: "server.auth_token_env",
        kind: EditKind::SetValue,
        ask: Ask::Line,
        note: "",
    },
    // session (merges cache — spec §4.11's "what is not a section")
    KeySpec {
        section: Section::Session,
        path: "session.ttl",
        kind: EditKind::SetValue,
        ask: Ask::Line,
        note: "",
    },
    KeySpec {
        section: Section::Session,
        path: "cache.sticky",
        kind: EditKind::SetValue,
        ask: Ask::Bool,
        note: "",
    },
    KeySpec {
        section: Section::Session,
        path: "cache.breakeven.enabled",
        kind: EditKind::SetValue,
        ask: Ask::Bool,
        note: "",
    },
    KeySpec {
        section: Section::Session,
        path: "cache.breakeven.min_remaining_turns",
        kind: EditKind::SetValue,
        ask: Ask::Line,
        note: "",
    },
    KeySpec {
        section: Section::Session,
        path: "cache.breakeven.safety_factor",
        kind: EditKind::SetValue,
        ask: Ask::Line,
        note: "",
    },
    // paths
    KeySpec {
        section: Section::Paths,
        path: "trace.dir",
        kind: EditKind::SetValue,
        ask: Ask::Line,
        note: "",
    },
    // paths: trace.rollover — hourly is the only value §4.1 defines, so the
    // row is display-only with the note that says so.
    KeySpec {
        section: Section::Paths,
        path: "trace.rollover",
        kind: EditKind::SetValue,
        ask: Ask::Show,
        note: "hourly is the only value spec §4.1 defines; edit the file by hand to change it",
    },
    // routing
    KeySpec {
        section: Section::Routing,
        path: "plan_policy.family",
        kind: EditKind::SetValue,
        ask: Ask::Line,
        note: "",
    },
    KeySpec {
        section: Section::Routing,
        path: "plan_policy.primary",
        kind: EditKind::SetValue,
        ask: Ask::Line,
        note: "",
    },
    KeySpec {
        section: Section::Routing,
        path: "plan_policy.overflow",
        kind: EditKind::SetValue,
        ask: Ask::Line,
        note: "",
    },
    KeySpec {
        section: Section::Routing,
        path: "plan_policy.on_primary_exhausted",
        kind: EditKind::SetValue,
        ask: Ask::Enum(&["spill", "block"]),
        note: "",
    },
    KeySpec {
        section: Section::Routing,
        path: "plan_policy.recover",
        kind: EditKind::SetValue,
        ask: Ask::Enum(&["probe", "none"]),
        note: "",
    },
    KeySpec {
        section: Section::Routing,
        path: "plan_policy.cooldown",
        kind: EditKind::SetValue,
        ask: Ask::Line,
        note: "",
    },
    // spec §4.6.1 / ADR-049 §5.5: how a spilled request picks its metered
    // route — the tag's declared order, or the price ranking. The row sits
    // between `.cooldown` and `.overflow_monthly_cap_usd` (spec §4.11's
    // amended section table).
    KeySpec {
        section: Section::Routing,
        path: "plan_policy.overflow_selection",
        kind: EditKind::SetValue,
        ask: Ask::Enum(&["declared", "cheapest"]),
        note: "",
    },
    KeySpec {
        section: Section::Routing,
        path: "plan_policy.overflow_monthly_cap_usd",
        kind: EditKind::SetValue,
        ask: Ask::Line,
        note: "",
    },
];

/// The `Enabled` rows of `auth` and `routing`: the keys the template ships
/// commented out (`server.auth_token_env`,
/// `plan_policy.overflow_monthly_cap_usd`), toggled by a `set-enabled`
/// edit on the key's own line.
pub const ENABLE_ROWS: &[KeySpec] = &[
    KeySpec {
        section: Section::Auth,
        path: "server.auth_token_env",
        kind: EditKind::SetEnabled,
        ask: Ask::Bool,
        note: "",
    },
    KeySpec {
        section: Section::Routing,
        path: "plan_policy.overflow_monthly_cap_usd",
        kind: EditKind::SetEnabled,
        ask: Ask::Bool,
        note: "",
    },
];

/// The rows of one section, static value rows first, then the entry rows
/// the file itself determines (`providers`/`plugins`), then `Enabled`
/// rows. The section table is the single place the wizard's key set is
/// written — no other module decides what is askable.
pub fn rows_for(section: Section, file_text: &str) -> Vec<KeySpec> {
    use crate::setup::anchor;
    // The list spelling (spec §4.6.1 / ADR-049 §4): over a root that
    // writes `plan_policies:`, none of the `plan_policy.*` rows is
    // askable — the single-family keys live inside a multi-line mapping
    // the anchor grammar cannot reach, and a run that quietly reports
    // `no change` over them is the silent no-op this rule removes
    // (R68-0 §2.4). The declared families are shown instead, one line
    // per family, by `show_entries`.
    if section == Section::Routing && top_key_present(file_text, "plan_policies") {
        return Vec::new();
    }
    let mut rows: Vec<KeySpec> = STATIC_ROWS
        .iter()
        .filter(|r| r.section == section)
        .cloned()
        .collect();
    match section {
        Section::Providers => {
            for name in anchor::entry_names(file_text, "providers", "name") {
                // The row set is decided by the entry the file itself
                // carries, never by the template (spec §4.11): an entry
                // that writes the pool instead of the one key contributes
                // no `api_key_env` row at all — the pool is displayed by
                // `show_entries`, one line per name, and never prompted.
                // The membership probe is a line-level read of the entry's
                // own block, not an anchor resolution: the pool is a flow
                // sequence, which the locator refuses by rule 6 — a
                // `NotSettable` error means the key *is* there, so it
                // cannot stand for "absent".
                if entry_writes_pool(file_text, &name) {
                    continue;
                }
                rows.push(KeySpec {
                    section,
                    path: Box::leak(format!("providers[name={name}].api_key_env").into_boxed_str()),
                    kind: EditKind::SetValue,
                    ask: Ask::Line,
                    note: "",
                });
            }
        }
        Section::Plugins => {
            for id in anchor::entry_names(file_text, "plugins", "id") {
                // `rules_file` exists only on builtin/transform_rules entries
                // (and is a flow-map key on cache-guard's single-line
                // config); a row whose anchor does not resolve is the
                // *warning* shape when no change is requested, so the
                // missing-anchor rows are simply skipped here.
                rows.push(KeySpec {
                    section,
                    path: Box::leak(format!("plugins[id={id}].disabled").into_boxed_str()),
                    kind: EditKind::SetValue,
                    ask: Ask::Bool,
                    note: "",
                });
            }
        }
        Section::Auth => {
            rows.extend(ENABLE_ROWS.iter().filter(|r| r.section == section).cloned());
        }
        _ => {}
    }
    rows
}

/// The display-only entries of a section (the membership and vendor-fact
/// lines `--print` shows one by one): aliases, fallback, and the vendor
/// facts the section that owns them displays (never prompts — spec
/// §4.11's "the vendor facts are never asked for, only shown").
pub fn show_entries(section: Section, file_text: &str) -> Vec<(String, String)> {
    use crate::setup::anchor;
    match section {
        Section::Providers => {
            let mut v = anchor::entry_names(file_text, "providers", "name")
                .into_iter()
                .map(|n| {
                    (
                        format!("providers.name = {n}"),
                        "the roster entry's own block (urls, models, prices) is a \
                         vendor-fact transcription — edit it by hand"
                            .to_string(),
                    )
                })
                .collect::<Vec<_>>();
            // ADR-049 §3's pool spelling: one display-only line per pool
            // name, in rotation order — a pool's membership (add, drop,
            // reorder a name) is a hand edit, exactly as `aliases` and
            // `fallback` are (spec §4.11).
            for n in anchor::entry_names(file_text, "providers", "name") {
                if let Some(pool) = pool_names(file_text, &n) {
                    for name in pool {
                        v.push((
                            format!("providers[name={n}].api_keys = {name}"),
                            "a pool's membership (add / drop / reorder a name) is a \
                             hand edit — shown, not prompted"
                                .to_string(),
                        ));
                    }
                }
            }
            v
        }
        Section::Routing => {
            // The list spelling (spec §4.6.1 / ADR-049 §4): over a root
            // that writes `plan_policies:`, the section's display names
            // each declared family — its tag and its
            // `overflow_selection` — one line per family, in declaration
            // order. Nothing is prompted and nothing is written there
            // (`rows_for` builds no `plan_policy.*` value row).
            if top_key_present(file_text, "plan_policies") {
                return declared_families(file_text);
            }
            let mut v = vec![(
                "aliases".to_string(),
                "membership edits (add / drop / reorder an entry) are hand \
                 edits — shown, not prompted"
                    .to_string(),
            )];
            for k in anchor::block_keys(file_text, "aliases") {
                if let Ok(a) =
                    crate::setup::anchor::resolve_typed(file_text, &format!("aliases.{k}"))
                {
                    v.push((format!("aliases.{k} = {}", a.value), String::new()));
                }
            }
            let n = anchor::seq_len(file_text, "fallback");
            v.push((
                format!("fallback: {n} entries, in order"),
                "membership edits are hand edits — shown, not prompted".to_string(),
            ));
            v
        }
        _ => Vec::new(),
    }
}

/// The root or roster text as a YAML value, when it parses. The two
/// ADR-049 shape reads go through the parser rather than the line
/// locator: both shapes (a flow-sequence pool, a multi-line family list)
/// are exactly the forms the locator refuses by rule 6 / `a.b[i]`, so a
/// line-level probe cannot stand for them. A text that does not parse
/// has neither shape — the run's own warning ladder then says the rest.
fn yaml(text: &str) -> Option<serde_yaml::Value> {
    serde_yaml::from_str(text).ok()
}

/// Whether the file carries a top-level key as a **live** YAML mapping
/// entry (a commented line is not YAML, so comments cannot satisfy it).
fn top_key_present(text: &str, key: &str) -> bool {
    yaml(text)
        .map(|v| v.get(key).is_some_and(|x| !x.is_null()))
        .unwrap_or(false)
}

/// Whether a roster entry writes the pool spelling (`api_keys:`,
/// ADR-049 §3's exactly-one-of ladder) instead of `api_key_env`.
fn entry_writes_pool(text: &str, name: &str) -> bool {
    pool_names(text, name).is_some()
}

/// The pool's names, in `api_keys:` (rotation) order, when the entry
/// writes the pool spelling.
fn pool_names(text: &str, name: &str) -> Option<Vec<String>> {
    let v = yaml(text)?;
    let entries = v.get("providers")?.as_sequence()?;
    for e in entries {
        if e.get("name").and_then(|n| n.as_str()) == Some(name) {
            return e.get("api_keys").and_then(|k| k.as_sequence()).map(|seq| {
                seq.iter()
                    .filter_map(|n| n.as_str())
                    .map(str::to_string)
                    .collect()
            });
        }
    }
    None
}

/// One display line per declared family of a `plan_policies:` root
/// (spec §4.11): the family's tag and its `overflow_selection`, in
/// declaration order — the list is shown, never edited.
fn declared_families(text: &str) -> Vec<(String, String)> {
    let Some(v) = yaml(text) else {
        return Vec::new();
    };
    let Some(list) = v.get("plan_policies").and_then(|p| p.as_sequence()) else {
        return Vec::new();
    };
    list.iter()
        .map(|e| {
            let tag = e.get("family").and_then(|f| f.as_str()).unwrap_or("");
            let sel = e
                .get("overflow_selection")
                .and_then(|f| f.as_str())
                .unwrap_or("declared");
            (
                format!("plan_policies: family {tag} — overflow_selection {sel}"),
                "the family list is shown, never edited — edits inside a \
                 plan_policies[i] entry are hand edits"
                    .to_string(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::anchor;

    fn example() -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config.example.yaml"),
        )
        .unwrap()
    }

    /// The roster half of the shipped pair (spec §4.14): the example
    /// splits, so the `providers` section's anchors live here.
    fn roster_example() -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../providers.example.yaml"),
        )
        .unwrap()
    }

    /// The shipped example for a row's target file — the test form of the
    /// section table's target-file column (the shipped root is split, so
    /// `providers` rows resolve in `providers.example.yaml`).
    fn shipped_example_for(row: &KeySpec) -> String {
        match row.section.target(true) {
            TargetFile::Root => example(),
            TargetFile::Roster => roster_example(),
        }
    }

    /// The table↔example check (DESIGN §12.14's rig): every static path
    /// the table carries must resolve in the shipped example **of its
    /// target file** — the table and the example cannot drift, and the
    /// split cannot move a section's keys without this test seeing it.
    #[test]
    fn every_static_row_resolves_in_the_example() {
        for row in STATIC_ROWS.iter().chain(ENABLE_ROWS) {
            let text = shipped_example_for(row);
            assert!(
                anchor::resolve(&text, row.path).is_ok(),
                "{} must resolve in the shipped {} example",
                row.path,
                row.section.target(true).as_str()
            );
        }
    }

    /// The target-file column itself (spec §4.11): `providers` owns the
    /// roster when the root names one, and the column collapses to the
    /// root for every section of the inline form.
    #[test]
    fn the_target_file_column() {
        assert_eq!(Section::Providers.target(true), TargetFile::Roster);
        for s in ALL {
            assert_eq!(
                s.target(false),
                TargetFile::Root,
                "the inline form edits the root for every section"
            );
            if s != Section::Providers {
                assert_eq!(
                    s.target(true),
                    TargetFile::Root,
                    "{}'s keys are the root's even under the split",
                    s.name()
                );
            }
        }
    }

    #[test]
    fn seven_sections_in_table_order() {
        let names: Vec<&str> = ALL.iter().map(|s| s.name()).collect();
        assert_eq!(
            names,
            vec![
                "server",
                "auth",
                "session",
                "paths",
                "providers",
                "routing",
                "plugins"
            ]
        );
        assert_eq!(Section::parse("all"), Some(Section::Server));
        assert_eq!(Section::parse("nope"), None);
    }

    #[test]
    fn provider_rows_come_from_the_file() {
        // The roster file is the providers section's text under the split
        // (spec §4.11's target-file column).
        let text = roster_example();
        let rows = rows_for(Section::Providers, &text);
        assert!(rows
            .iter()
            .any(|r| r.path == "providers[name=deepseek].api_key_env"));
        assert!(rows
            .iter()
            .any(|r| r.path == "providers[name=kimi-cn-plan].api_key_env"));
    }

    /// A roster fixture that writes the pool spelling for **one** entry
    /// (spec §4.11's assertion list; the shipped pair ships both shapes
    /// commented out, so it cannot witness either — the fixtures are the
    /// witness). The `deepseek` entry writes the pool; `zai` keeps the
    /// one-key spelling, so the fixture covers both sides of the
    /// exactly-one-of ladder in one file.
    const POOL_ROSTER: &str = r#"providers:
  - name: deepseek
    urls:
      chat: https://api.deepseek.com/chat/completions
      responses: https://api.deepseek.com/responses
    api_keys: [DS_POOL_A, DS_POOL_B, DS_POOL_C]
    wire_api: chat
    supports: [chat, responses]
    account: api
    models:
      - id: deepseek-chat
        context: 128k
        price:
          input_miss: 0.00027
          input_hit: 0.000007
          cache_write: 0.0
          output: 0.0011
          peak: { multiplier: 1.0, windows: [] }
        source: "https://api-docs.deepseek.com/quick_start/pricing @2026-09-19"
  - name: zai
    urls:
      chat: https://api.z.ai/api/paas/v4/chat/completions
    api_key_env: ZAI_API_KEY
    wire_api: chat
    supports: [chat]
    account: api
    models:
      - id: glm-5.3
        context: 200k
        family: glm-5.3
        price:
          input_miss: 0.00066
          input_hit: 0.000022
          cache_write: 0.0
          output: 0.00198
          peak: { multiplier: 1.0, windows: [] }
        source: "https://z.ai/pricing @2026-09-19"
"#;

    /// Spec §4.11's pool assertion: the pool entry contributes a
    /// display-only row per pool name (in `api_keys:` order) and **no**
    /// `api_key_env` row, while every one-key entry's row is unchanged.
    #[test]
    fn a_pool_entry_is_shown_never_asked() {
        let rows = rows_for(Section::Providers, POOL_ROSTER);
        // No api_key_env row for the pooled entry — the pool spelling and
        // the one-key spelling are never both asked for one entry.
        assert!(
            !rows
                .iter()
                .any(|r| r.path == "providers[name=deepseek].api_key_env"),
            "a pool entry contributes no api_key_env row"
        );
        // The one-key entry's row is unchanged.
        assert!(rows
            .iter()
            .any(|r| r.path == "providers[name=zai].api_key_env"));
        // The display lines: one per pool name, in rotation order.
        let shown = show_entries(Section::Providers, POOL_ROSTER);
        let pool_lines: Vec<&str> = shown
            .iter()
            .filter(|(p, _)| p.starts_with("providers[name=deepseek].api_keys"))
            .map(|(p, _)| p.as_str())
            .collect();
        assert_eq!(
            pool_lines,
            vec![
                "providers[name=deepseek].api_keys = DS_POOL_A",
                "providers[name=deepseek].api_keys = DS_POOL_B",
                "providers[name=deepseek].api_keys = DS_POOL_C",
            ],
            "one display line per pool name, in api_keys: order"
        );
        // The note says what the run does not do: membership is a hand
        // edit (the same shape as `aliases`/`fallback`).
        assert!(shown
            .iter()
            .filter(|(p, _)| p.starts_with("providers[name=deepseek].api_keys"))
            .all(|(_, note)| note.contains("hand edit")));
        // The one-key entry contributes no pool line.
        assert!(!shown
            .iter()
            .any(|(p, _)| p.starts_with("providers[name=zai].api_keys")));
    }

    /// A root fixture that writes the list spelling (spec §4.11's
    /// assertion list). Two families, so the one-line-per-family shape is
    /// asserted on a list and not on a singleton; the second family
    /// omits `overflow_selection`, so the default line names `declared`.
    const PLAN_POLICIES_ROOT: &str = r#"server: { addr: "127.0.0.1:8790", upstream_attempt_timeout: 60s, request_timeout: 10m }
session: { key_sources: ["prompt_cache_key"], ttl: 12h }
cache: { sticky: true, breakeven: { enabled: true, min_remaining_turns: 3, safety_factor: 1.2 } }
trace: { dir: "./state/traces", rollover: hourly }
aliases: {}
plugins: []
fallback: []
plan_policies:
  - family: glm-5.3
    primary: zai-plan/glm-5.3
    overflow: zai/glm-5.3
    on_primary_exhausted: spill
    recover: probe
    cooldown: 15m
    overflow_selection: cheapest
  - family: kimi-k3
    primary: kimi-plan/kimi-k3
    overflow: kimi/kimi-k3
    on_primary_exhausted: block
    recover: none
    cooldown: 30m
"#;

    /// Spec §4.11's list assertion: over a root that writes
    /// `plan_policies:`, the routing section builds no `plan_policy.*`
    /// value row and its display names each declared family — the
    /// alternative spelling's keys are never named as if the file were
    /// missing them.
    #[test]
    fn a_plan_policies_root_is_shown_never_edited() {
        let rows = rows_for(Section::Routing, PLAN_POLICIES_ROOT);
        assert!(
            rows.iter().all(|r| !r.path.starts_with("plan_policy.")),
            "no plan_policy.* value row over a plan_policies: root, got {:?}",
            rows.iter().map(|r| r.path).collect::<Vec<_>>()
        );
        let shown = show_entries(Section::Routing, PLAN_POLICIES_ROOT);
        let families: Vec<&str> = shown
            .iter()
            .map(|(p, _)| p.as_str())
            .filter(|p| p.starts_with("plan_policies:"))
            .collect();
        assert_eq!(
            families,
            vec![
                "plan_policies: family glm-5.3 — overflow_selection cheapest",
                "plan_policies: family kimi-k3 — overflow_selection declared",
            ],
            "one line per declared family (its tag and its overflow_selection), \
             in declaration order"
        );
        assert!(shown
            .iter()
            .filter(|(p, _)| p.starts_with("plan_policies:"))
            .all(|(_, note)| note.contains("shown, never edited")));
        // The single-family root is unchanged: the same calls over the
        // shipped example still build the eight rows (including the new
        // overflow_selection row) and the aliases/fallback lines.
        let shipped = rows_for(Section::Routing, &example());
        assert!(shipped
            .iter()
            .any(|r| r.path == "plan_policy.overflow_selection"));
        assert!(shipped
            .iter()
            .any(|r| r.path == "plan_policy.overflow_monthly_cap_usd"));
        assert!(show_entries(Section::Routing, &example())
            .iter()
            .any(|(p, _)| p == "aliases"));
    }

    /// The commented spellings the shipped pair carries (R68-2's frozen
    /// comments) are inert: a commented `api_keys:` line is not a live
    /// pool, and a commented `plan_policies:` line is not a live list —
    /// neither shape is detected over prose alone.
    #[test]
    fn commented_spellings_are_inert() {
        let root = example();
        assert!(!top_key_present(&root, "plan_policies"));
        assert!(!top_key_present(&root, "plan_policy_key_absent_by_construction"));
        let roster = roster_example();
        assert!(!entry_writes_pool(&roster, "deepseek"));
        assert_eq!(
            rows_for(Section::Providers, &roster)
                .iter()
                .filter(|r| r.path == "providers[name=deepseek].api_key_env")
                .count(),
            1,
            "the shipped one-key entries keep their rows"
        );
    }
}
