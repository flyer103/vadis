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
    let mut rows: Vec<KeySpec> = STATIC_ROWS
        .iter()
        .filter(|r| r.section == section)
        .cloned()
        .collect();
    match section {
        Section::Providers => {
            for name in anchor::entry_names(file_text, "providers", "name") {
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
        Section::Auth | Section::Routing => {
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
        Section::Providers => anchor::entry_names(file_text, "providers", "name")
            .into_iter()
            .map(|n| {
                (
                    format!("providers.name = {n}"),
                    "the roster entry's own block (urls, models, prices) is a \
                     vendor-fact transcription — edit it by hand"
                        .to_string(),
                )
            })
            .collect(),
        Section::Routing => {
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

    /// The table↔example check (DESIGN §12.14's rig): every static path
    /// the table carries must resolve in the shipped example — the table
    /// and the example cannot drift.
    #[test]
    fn every_static_row_resolves_in_the_example() {
        let text = example();
        for row in STATIC_ROWS.iter().chain(ENABLE_ROWS) {
            assert!(
                anchor::resolve(&text, row.path).is_ok(),
                "{} must resolve in config.example.yaml",
                row.path
            );
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
        let text = example();
        let rows = rows_for(Section::Providers, &text);
        assert!(rows
            .iter()
            .any(|r| r.path == "providers[name=deepseek].api_key_env"));
        assert!(rows
            .iter()
            .any(|r| r.path == "providers[name=kimi-cn-plan].api_key_env"));
    }
}
