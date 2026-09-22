//! setup/report.rs — the `--print` / `--check` / `--dry-run` renderings,
//! human and `--json` (DESIGN §12.14; spec §4.11's read-only surfaces).
//!
//! The secret boundary holds here too: every member the JSON forms carry
//! is a **name**, a status or a path — no field exists that a value could
//! occupy (spec §4.11's secret boundary; CONF-70's canary).

use crate::setup::edit::{Edit, EditKind, Plan};
use serde_json::json;
use std::path::Path;

/// One `--check` row: a variable the file names, and its presence state.
/// For the token, "absent" and "empty" are distinct states — the
/// distinction `serve` refuses the start on (spec §4.7 / §12.10.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyState {
    Present,
    Absent,
    Empty,
}

impl KeyState {
    pub fn as_str(&self) -> &'static str {
        match self {
            KeyState::Present => "present",
            KeyState::Absent => "absent",
            KeyState::Empty => "empty",
        }
    }
}

/// Probe the two probes `serve` already makes, and nothing else
/// (DESIGN §12.14): `var_os(name).is_some()` for a provider key;
/// `var(name)` mapped to present / **empty** / absent for the token.
pub fn probe(name: &str, distinguish_empty: bool) -> KeyState {
    if !distinguish_empty {
        return if std::env::var_os(name).is_some() {
            KeyState::Present
        } else {
            KeyState::Absent
        };
    }
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => KeyState::Present,
        Ok(_) => KeyState::Empty,
        Err(_) => KeyState::Absent,
    }
}

/// `--check`'s rows from a loaded config. The loader already ran (a file
/// that does not load is exit 2 before this); this only enumerates.
pub fn check_rows(cfg: &router_core::config::RouterConfig) -> Vec<(String, KeyState)> {
    let mut rows = Vec::new();
    for p in &cfg.providers {
        rows.push((p.api_key_env.clone(), probe(&p.api_key_env, false)));
    }
    if let Some(name) = &cfg.server.auth_token_env {
        rows.push((name.clone(), probe(name, true)));
    }
    rows
}

/// The human `--check` rendering: names and states, plus the export
/// snippet for each missing one — the boundary is names, and the snippet
/// is where the help stops (spec §4.11: "it prints the export snippet for
/// each missing name and stops there").
pub fn check_text(rows: &[(String, KeyState)]) -> String {
    let mut out = String::new();
    for (name, state) in rows {
        out.push_str(&format!("{name}: {}\n", state.as_str()));
        if *state != KeyState::Present {
            out.push_str(&format!(
                "  {}\n",
                crate::setup::prompt::export_snippet(name)
            ));
        }
    }
    out
}

/// The `--check --json` rendering. `selected_by` names the §4.12 rule
/// that chose the file (spec §4.11's "which file gets written is
/// printed, with the rule that chose it").
pub fn check_json(
    path: &Path,
    selected_by: &str,
    rows: &[(String, KeyState)],
) -> serde_json::Value {
    json!({
        "config": path.display().to_string(),
        "selected_by": selected_by,
        "keys": rows
            .iter()
            .map(|(n, s)| json!({ "name": n, "state": s.as_str() }))
            .collect::<Vec<_>>(),
        "all_present": rows.iter().all(|(_, s)| *s == KeyState::Present),
    })
}

/// One `--print` row.
pub struct PrintRow {
    pub path: String,
    pub value: String,
    pub enabled: bool,
    pub note: &'static str,
    /// `true` when the value shown is the template's, not the target's
    /// (a target that does not exist prints the template's values,
    /// labelled as such — spec §4.11's `--print` row).
    pub from_template: bool,
}

pub fn print_text(rows: &[PrintRow], path: &Path, from_template: bool) -> String {
    let mut out = String::new();
    if from_template {
        out.push_str(&format!(
            "# {path} does not exist; the template's values:\n",
            path = path.display()
        ));
    } else {
        out.push_str(&format!("# {}\n", path.display()));
    }
    for r in rows {
        let state = if r.enabled { "" } else { " (commented out)" };
        out.push_str(&format!("{} = {}{state}\n", r.path, r.value));
        if !r.note.is_empty() {
            out.push_str(&format!("    # {}\n", r.note));
        }
    }
    out
}

pub fn print_json(rows: &[PrintRow], path: &Path, selected_by: &str) -> serde_json::Value {
    json!({
        "config": path.display().to_string(),
        "selected_by": selected_by,
        "from_template": rows.iter().any(|r| r.from_template),
        "keys": rows
            .iter()
            .map(|r| json!({
                "path": r.path,
                "value": r.value,
                "enabled": r.enabled,
            }))
            .collect::<Vec<_>>(),
    })
}

/// `--dry-run`: each edit as `<anchor>: <old> → <new>`, with the edit
/// kind, in application order (spec §4.11's `--dry-run` row).
pub fn dry_run_text(base_first_line_of: impl Fn(usize) -> Option<String>, plan: &Plan) -> String {
    let mut out = String::new();
    for e in &plan.edits {
        let line = base_first_line_of(e.line)
            .unwrap_or_default()
            .trim()
            .to_string();
        out.push_str(&format!(
            "{} (line {}): {kind}\n  - {line}\n  + {}={}\n",
            e.path,
            e.line + 1,
            key_of(e),
            e.replacement,
            kind = e.kind.as_str()
        ));
    }
    out
}

fn key_of(e: &Edit) -> String {
    // The last path segment (the key on the edited line).
    e.path.rsplit('.').next().unwrap_or(&e.path).to_string()
}

/// The `--dry-run` JSON form: the same facts, machine-shaped.
pub fn dry_run_json(plan: &Plan) -> serde_json::Value {
    json!({
        "edits": plan
            .edits
            .iter()
            .map(|e| json!({
                "kind": e.kind.as_str(),
                "path": e.path,
                "line": e.line + 1,
                "old": e.old,
                "new": e.replacement,
            }))
            .collect::<Vec<_>>(),
    })
}

/// The landing line: the absolute path plus the rule that chose it, the
/// count of edits, and — under `--json` — the same facts as members.
pub fn landed_text(path: &Path, selected_by: &str, n: usize) -> String {
    format!(
        "wrote {path} (selected by {selected_by}; {n} edit{s} applied)\n",
        path = path.display(),
        s = if n == 1 { "" } else { "s" }
    )
}

pub fn landed_json(path: &Path, selected_by: &str, n: usize) -> serde_json::Value {
    json!({
        "wrote": path.display().to_string(),
        "selected_by": selected_by,
        "edits_applied": n,
    })
}

/// `no change: <path> left as it is` — the line that makes a second run
/// a no-op rather than a rewrite (spec §4.11; G3).
pub fn no_change_text(path: &Path) -> String {
    format!("no change: {} left as it is\n", path.display())
}

/// The `set-enabled` marker for `--dry-run`'s kind column (kept distinct
/// so a plan's kinds are inspectable before they land).
pub fn kind_str(k: EditKind) -> &'static str {
    k.as_str()
}
