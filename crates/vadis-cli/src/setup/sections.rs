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
    /// grammar before it is encoded — spec §4.11 step 2). The `cooldown`
    /// row is matched by **shape**, because it exists in two spellings:
    /// `plan_policy.cooldown` and `plan_policies[i].cooldown` (ADR-052
    /// §2.7 item 3 — an equality list of literal paths would let a
    /// per-family duration answer be encoded unvalidated).
    pub fn is_duration(&self) -> bool {
        matches!(
            self.path,
            "server.upstream_attempt_timeout" | "server.request_timeout" | "session.ttl"
        ) || self.path == "plan_policy.cooldown"
            || (self.path.starts_with("plan_policies[") && self.path.ends_with("].cooldown"))
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
];

/// The eight policy keys of a family, in spec §4.11's order, with the
/// question each asks. The **paths** are not here: they are built per
/// spelling (`plan_policy.<key>` or `plan_policies[i].<key>`), so the
/// routing rows are built rows rather than static ones (ADR-052 F2) and
/// the list spelling's row set follows the file's own entries.
const ROUTING_KEYS: [(&str, Ask); 8] = [
    ("family", Ask::Line),
    ("primary", Ask::Line),
    ("overflow", Ask::Line),
    ("on_primary_exhausted", Ask::Enum(&["spill", "block"])),
    ("recover", Ask::Enum(&["probe", "none"])),
    ("cooldown", Ask::Line),
    // spec §4.6.1 / ADR-049 §5.5: how a spilled request picks its metered
    // route — the tag's declared order, or the price ranking. The row sits
    // between `.cooldown` and `.overflow_monthly_cap_usd` (spec §4.11's
    // amended section table).
    ("overflow_selection", Ask::Enum(&["declared", "cheapest"])),
    ("overflow_monthly_cap_usd", Ask::Line),
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
/// the file itself determines (`providers`/`plugins`/`routing`), then
/// `Enabled` rows. The section table is the single place the wizard's key
/// set is written — no other module decides what is askable.
pub fn rows_for(section: Section, file_text: &str) -> Vec<KeySpec> {
    use crate::setup::anchor;
    let mut rows: Vec<KeySpec> = STATIC_ROWS
        .iter()
        .filter(|r| r.section == section)
        .cloned()
        .collect();
    match section {
        Section::Routing => {
            // The two spellings (spec §4.6.1 / ADR-049 §4, amended by
            // ADR-052 §2.2). Over a root that writes `plan_policies:`, the
            // eight policy rows are built **per declared entry**,
            // family-major — one family is one block of questions, and the
            // index in a question stays constant for its whole block. Over
            // a root that writes the single spelling, or neither key, the
            // eight singleton rows are built, exactly as before. Both are
            // built (the `Box::leak` shape the `providers`/`plugins` rows
            // use), so the row set is a function of **the file** — a third
            // family added by hand moves the questions with it, and no
            // source here names a family.
            let families = plan_family_tags(file_text).len();
            if families > 0 {
                for i in 0..families {
                    for (key, ask) in ROUTING_KEYS {
                        rows.push(KeySpec {
                            section,
                            path: leak(format!("plan_policies[{i}].{key}")),
                            kind: EditKind::SetValue,
                            ask,
                            note: "",
                        });
                    }
                }
                // The per-family `set-enabled` row for the cap. The
                // `ENABLE_ROWS` const carries fixed paths, so this one is
                // built here: the shipped list carries the cap commented
                // out inside each entry, and a commented key inside a list
                // entry resolves with `enabled=false` — which is what makes
                // the edit reachable (ADR-052 §2.1 row 8, measured).
                for i in 0..families {
                    rows.push(KeySpec {
                        section,
                        path: leak(format!("plan_policies[{i}].overflow_monthly_cap_usd")),
                        kind: EditKind::SetEnabled,
                        ask: Ask::Bool,
                        note: "",
                    });
                }
            } else {
                for (key, ask) in ROUTING_KEYS {
                    rows.push(KeySpec {
                        section,
                        path: leak(format!("plan_policy.{key}")),
                        kind: EditKind::SetValue,
                        ask,
                        note: "",
                    });
                }
            }
        }
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
            // The two spellings (spec §4.6.1 / ADR-049 §4, amended by
            // ADR-052 §2.2). Over a list root the section prints one
            // **membership** line per declared family — its index and its
            // tag, with the note that adding, dropping or reordering a
            // family is a hand edit — and then the section's ordinary
            // display lines. The union is the point: `aliases` and
            // `fallback` are spelled independently of the policy's shape,
            // so a surface that dropped them because the policy is a list
            // would hide facts the file carries. The keys **inside** each
            // entry are prompted (`rows_for` builds them, one family at a
            // time); only the membership is display-only.
            let mut v: Vec<(String, String)> = Vec::new();
            if top_key_present(file_text, "plan_policies") {
                v.extend(membership_lines(file_text));
            }
            v.push((
                "aliases".to_string(),
                "membership edits (add / drop / reorder an entry) are hand \
                 edits — shown, not prompted"
                    .to_string(),
            ));
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

/// Each declared family of a `plan_policies:` root, in declaration order,
/// as its 0-based index and its tag — the membership fact spec §4.11
/// prints one line per family, and the set of entries `rows_for` builds
/// the questions for. A property of the **file**, never of the template.
fn plan_family_tags(text: &str) -> Vec<(usize, String)> {
    let Some(v) = yaml(text) else {
        return Vec::new();
    };
    let Some(list) = v.get("plan_policies").and_then(|p| p.as_sequence()) else {
        return Vec::new();
    };
    list.iter()
        .enumerate()
        .map(|(i, e)| {
            (
                i,
                e.get("family")
                    .and_then(|f| f.as_str())
                    .unwrap_or("")
                    .to_string(),
            )
        })
        .collect()
}

/// The display-only **membership** lines of a list root: one per declared
/// family, `plan_policies[<i>] — family <tag>` (spec §4.11's line), each
/// noting that adding, dropping or reordering a family is a hand edit
/// while the keys inside the entry **are** prompted (ADR-052 §2.2).
fn membership_lines(text: &str) -> Vec<(String, String)> {
    let note = "membership (add / drop / reorder a family) is a hand edit — \
                the keys inside each entry are prompted"
        .to_string();
    plan_family_tags(text)
        .into_iter()
        .map(|(i, tag)| (format!("plan_policies[{i}] — family {tag}"), note.clone()))
        .collect()
}

/// A built row's path, held for the run's life. `KeySpec::path` is
/// `&'static str`; the rows a file decides leak one small string each,
/// exactly as the `providers`/`plugins` rows do.
fn leak(path: String) -> &'static str {
    Box::leak(path.into_boxed_str())
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
    /// Since **ADR-052** (F2) the `routing` rows are **built** rather
    /// than static — the shipped root writes the list spelling, so the
    /// eight `plan_policy.*` paths cannot both stay in `STATIC_ROWS` and
    /// resolve — and this test therefore keeps the static table minus
    /// them. The routing witness is the second test below.
    #[test]
    fn every_static_row_resolves_in_the_example() {
        for row in STATIC_ROWS
            .iter()
            .chain(ENABLE_ROWS)
            .filter(|r| r.section != Section::Routing)
        {
            let text = shipped_example_for(row);
            assert!(
                anchor::resolve(&text, row.path).is_ok(),
                "{} must resolve in the shipped {} example",
                row.path,
                row.section.target(true).as_str()
            );
        }
    }

    /// ADR-052 §2.3(c): the routing section's rows are **built from the
    /// file** (`plan_policies[i].<key>` for each declared entry, plus
    /// that family's cap `set-enabled` row), so the witness is the built
    /// row set: every path it carries must resolve in the shipped root.
    /// The two tests stay two tests — one over the static table, one over
    /// the built row set (the ADR's `§2.3(c)`).
    #[test]
    fn every_built_routing_row_resolves_in_the_example() {
        let text = example();
        let rows = rows_for(Section::Routing, &text);
        assert!(
            !rows.is_empty(),
            "the shipped root declares a plan family (ADR-052's flip)"
        );
        for row in &rows {
            assert!(
                anchor::resolve(&text, row.path).is_ok(),
                "the built routing row `{}` must resolve in config.example.yaml",
                row.path
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

    /// Spec §4.11's list assertion (ADR-052 §2.2, §2.3(d) — re-pointed
    /// from `a_plan_policies_root_is_shown_never_edited`): over a root
    /// that writes `plan_policies:`, the routing section **builds the
    /// eight policy rows per declared family** — family-major, plus that
    /// family's `set-enabled` row for its commented-out cap — and builds
    /// **no** `plan_policy.*` row; and its display names each declared
    /// family's **membership** beside the section's ordinary
    /// `aliases`/`fallback` lines (the union of §2.2 — a
    /// spelling-independent line must not vanish because the policy is
    /// spelled as a list).
    #[test]
    fn a_plan_policies_root_is_edited_family_by_family() {
        use crate::setup::edit::EditKind;

        let rows = rows_for(Section::Routing, PLAN_POLICIES_ROOT);
        assert!(
            rows.iter().all(|r| !r.path.starts_with("plan_policy.")),
            "no plan_policy.* row over a plan_policies: root, got {:?}",
            rows.iter().map(|r| r.path).collect::<Vec<_>>()
        );
        // The relation: eight value rows per declared family,
        // family-major — the fixture declares two families.
        let keys = [
            "family",
            "primary",
            "overflow",
            "on_primary_exhausted",
            "recover",
            "cooldown",
            "overflow_selection",
            "overflow_monthly_cap_usd",
        ];
        let value_rows: Vec<&str> = rows
            .iter()
            .filter(|r| r.kind == EditKind::SetValue)
            .map(|r| r.path)
            .collect();
        assert_eq!(
            value_rows.len(),
            8 * 2,
            "8 x the number of declared families"
        );
        for (k, key) in keys.iter().enumerate() {
            assert_eq!(value_rows[k], format!("plan_policies[0].{key}"));
            assert_eq!(value_rows[8 + k], format!("plan_policies[1].{key}"));
        }
        // The rows are the single spelling's own questions.
        let ask_of = |path: &str| {
            rows.iter()
                .find(|r| r.path == path)
                .map(|r| r.ask)
                .expect("the row exists")
        };
        assert_eq!(
            ask_of("plan_policies[0].overflow_selection"),
            Ask::Enum(&["declared", "cheapest"])
        );
        assert_eq!(
            ask_of("plan_policies[1].on_primary_exhausted"),
            Ask::Enum(&["spill", "block"])
        );
        assert_eq!(
            ask_of("plan_policies[0].recover"),
            Ask::Enum(&["probe", "none"])
        );
        // One set-enabled row per family, for the cap key inside it.
        let enables: Vec<&str> = rows
            .iter()
            .filter(|r| r.kind == EditKind::SetEnabled)
            .map(|r| r.path)
            .collect();
        assert_eq!(
            enables,
            vec![
                "plan_policies[0].overflow_monthly_cap_usd",
                "plan_policies[1].overflow_monthly_cap_usd"
            ]
        );

        // The display: one membership line per declared family — its
        // index and its tag — then the section's ordinary lines.
        let shown = show_entries(Section::Routing, PLAN_POLICIES_ROOT);
        let first: Vec<&str> = shown.iter().take(3).map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            first,
            vec![
                "plan_policies[0] — family glm-5.3",
                "plan_policies[1] — family kimi-k3",
                "aliases",
            ],
            "membership lines, then the aliases header"
        );
        assert!(shown
            .iter()
            .filter(|(p, _)| p.starts_with("plan_policies["))
            .all(|(_, note)| note.contains("hand edit")));
        assert!(
            shown.iter().any(|(p, _)| p.starts_with("fallback:")),
            "the fallback line is spelled independently of the policy's shape"
        );
        // The duration row is recognised **by shape** over the list
        // spelling (ADR-052 §2.7 item 3) — an equality list of literal
        // paths would let a per-family cooldown answer be encoded
        // unvalidated.
        let cooldowns: Vec<&KeySpec> = rows
            .iter()
            .filter(|r| r.path.ends_with(".cooldown"))
            .collect();
        assert_eq!(cooldowns.len(), 2, "one cooldown row per declared family");
        assert!(cooldowns.iter().all(|r| r.is_duration()));
        let not_durations = [
            "plan_policies[0].family",
            "plan_policies[0].overflow_monthly_cap_usd",
        ];
        for path in not_durations {
            assert!(!rows
                .iter()
                .find(|r| r.path == path)
                .expect("the row exists")
                .is_duration());
        }

        // Over the **shipped root** itself — a list root from R70 on
        // (ADR-052 §2.6) — the same relation, over the file's own
        // families, and the `aliases` line is still shown.
        let shipped = example();
        let shipped_rows = rows_for(Section::Routing, &shipped);
        let families = anchor::entry_names(&shipped, "plan_policies", "family").len();
        assert!(families >= 1, "the shipped root declares a family");
        assert_eq!(
            shipped_rows
                .iter()
                .filter(|r| r.kind == EditKind::SetValue)
                .count(),
            8 * families,
            "the shipped root's built row set is 8 per declared family"
        );
        assert!(shipped_rows
            .iter()
            .all(|r| !r.path.starts_with("plan_policy.")));
        assert!(show_entries(Section::Routing, &shipped)
            .iter()
            .any(|(p, _)| p == "aliases"));
    }

    /// ADR-052 §2.3(e): the two spellings' inertness after the flip. The
    /// shipped root now writes the **live** list, so it can no longer be
    /// the inert carrier a commented spelling is witnessed on — a fixture
    /// text carrying `# plan_policies:` is — while a commented `api_keys:`
    /// line is still not a live pool (R68-2's frozen comments).
    #[test]
    fn commented_spellings_are_inert() {
        let root = example();
        assert!(
            top_key_present(&root, "plan_policies"),
            "the shipped root writes the live list (ADR-052's flip)"
        );
        assert!(rows_for(Section::Routing, &root)
            .iter()
            .any(|r| r.path.starts_with("plan_policies[")));

        // A commented spelling is prose, not a live list: no per-family
        // row, no membership line — and the single spelling's rows are
        // built instead (the file writes neither key).
        let commented = "# plan_policies:\n#   - family: ghost\n";
        assert!(!top_key_present(commented, "plan_policies"));
        assert!(rows_for(Section::Routing, commented)
            .iter()
            .all(|r| !r.path.starts_with("plan_policies[")));
        assert!(show_entries(Section::Routing, commented)
            .iter()
            .all(|(p, _)| !p.starts_with("plan_policies[")));

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
