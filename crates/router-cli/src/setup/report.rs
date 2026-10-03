//! setup/report.rs — the `--print` / `--check` / `--dry-run` renderings,
//! human and `--json` (DESIGN §12.14; spec §4.11's read-only surfaces).
//!
//! The secret boundary holds here too: every member the JSON forms carry
//! is a **name**, a status or a path — no field exists that a value could
//! occupy (spec §4.11's secret boundary; CONF-70's canary).

use crate::setup::edit::{Edit, EditKind, Plan};
use crate::setup::split::Split;
use serde_json::json;
use std::path::{Path, PathBuf};

/// How the run came to write the roster — spec §4.11's target-file
/// column, and the two labels the landing line says aloud: a file the
/// root **named** (`providers_file:`, §4.14) or one this run created by
/// **moving** the root's own inline block into it (ADR-038).
pub const NAMED_BY_PROVIDERS_FILE: &str = "named by providers_file";
pub const MOVED_FROM_INLINE: &str = "the root's inline roster, moved";

/// How the run came to write the rule file (ADR-046): the root's own
/// `plugins` entry names it at `config.rules_file` — read from the base,
/// never invented. Used for the lane's landing line and its `--dry-run`
/// `because` clause, in the same per-lane report shape the pair's lanes
/// print (`landed_roster_text` / `dry_run_would_write_text`).
pub const NAMED_BY_RULES_FILE: &str = "named by plugins[].config.rules_file";

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
/// each missing name and stops there"). `inline` is the roster note for a
/// root that carries the roster inline, and heads the rows: the fact is
/// about the file, not about a key.
pub fn check_text(rows: &[(String, KeyState)], inline: Option<&str>) -> String {
    let mut out = String::new();
    if let Some(name) = inline {
        out.push_str(&inline_roster_text(name));
    }
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
    inline: Option<&str>,
) -> serde_json::Value {
    let mut v = json!({
        "config": path.display().to_string(),
        "selected_by": selected_by,
        "keys": rows
            .iter()
            .map(|(n, s)| json!({ "name": n, "state": s.as_str() }))
            .collect::<Vec<_>>(),
        "all_present": rows.iter().all(|(_, s)| *s == KeyState::Present),
    });
    if let Some(name) = inline {
        v["roster"] = inline_roster_json(name);
    }
    v
}

/// A read-only surface's statement about an **inline** root (ADR-038 D9):
/// the run changes nothing, so it says what a *writing* run would do with
/// the file — the sentence that turns "no roster file appeared" from a
/// silent absence into a stated fact, without a flag to remember.
pub fn inline_roster_text(name: &str) -> String {
    format!("roster: inline in this file — a writing run moves it to {name}\n")
}

pub fn inline_roster_json(name: &str) -> serde_json::Value {
    json!({ "inline": true, "splits_on_write_to": name })
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

pub fn print_text(
    rows: &[PrintRow],
    path: &Path,
    from_template: bool,
    inline: Option<&str>,
) -> String {
    let mut out = String::new();
    if from_template {
        out.push_str(&format!(
            "# {path} does not exist; the template's values:\n",
            path = path.display()
        ));
    } else {
        out.push_str(&format!("# {}\n", path.display()));
    }
    if let Some(name) = inline {
        out.push_str(&inline_roster_text(name));
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

pub fn print_json(
    rows: &[PrintRow],
    path: &Path,
    selected_by: &str,
    inline: Option<&str>,
) -> serde_json::Value {
    let mut v = json!({
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
    });
    if let Some(name) = inline {
        v["roster"] = inline_roster_json(name);
    }
    v
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

/// The shape step's facts, for the surfaces that name it (ADR-038 D9):
/// the lines that moved out of the root, how many bytes they were, and
/// where they went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitFacts {
    pub first_line: usize,
    pub last_line: usize,
    pub lines: usize,
    pub bytes: usize,
    pub roster: PathBuf,
}

impl SplitFacts {
    pub fn of(s: &Split, roster: &Path) -> SplitFacts {
        SplitFacts {
            first_line: s.span.first_line(),
            last_line: s.span.last_line(),
            lines: s.span.lines(),
            bytes: s.roster.len(),
            roster: roster.to_path_buf(),
        }
    }
}

/// `split: providers: lines 100-1058 (959 lines, 71069 bytes) → <roster>` —
/// one line, so a run's report says the roster moved rather than leaving
/// the operator to diff two files (ADR-038 D9: silent is the one thing
/// this must not be).
pub fn split_note(f: &SplitFacts) -> String {
    format!(
        "split: providers: lines {first}-{last} ({lines} line{s}, {bytes} bytes) → {roster}\n",
        first = f.first_line,
        last = f.last_line,
        lines = f.lines,
        s = if f.lines == 1 { "" } else { "s" },
        bytes = f.bytes,
        roster = f.roster.display()
    )
}

pub fn split_json(f: &SplitFacts) -> serde_json::Value {
    json!({
        "moved": {
            "lines": [f.first_line, f.last_line],
            "count": f.lines,
            "bytes": f.bytes,
            "to": f.roster.display().to_string(),
        }
    })
}

/// The `--dry-run` JSON form: the same facts, machine-shaped. Under a
/// split root the roster lane's edits ride in an additive `roster`
/// member, labelled with its path — an edit's target file is never
/// ambiguous (spec §4.11's target-file column).
pub fn dry_run_json(
    plan: &Plan,
    roster: Option<(&Path, &str, &Plan)>,
    split: Option<&SplitFacts>,
) -> serde_json::Value {
    let edits = |p: &Plan| {
        p.edits
            .iter()
            .map(|e| {
                json!({
                    "kind": e.kind.as_str(),
                    "path": e.path,
                    "line": e.line + 1,
                    "old": e.old,
                    "new": e.replacement,
                })
            })
            .collect::<Vec<_>>()
    };
    let mut v = json!({ "edits": edits(plan) });
    if let Some((path, named_by, rplan)) = roster {
        v["roster"] = json!({
            "path": path.display().to_string(),
            "named_by": named_by,
            "edits": edits(rplan),
        });
    }
    if let Some(f) = split {
        v["split"] = split_json(f);
    }
    v
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

/// The roster lane's landing line: the roster is **named** by the root's
/// `providers_file` — never selected by §4.12's order — or it **is** the
/// block this run moved out of the root (ADR-038). Either way the line
/// says which, rather than borrowing the root's rule (spec §4.14: named,
/// never searched).
pub fn landed_roster_text(path: &Path, named_by: &str, n: usize) -> String {
    format!(
        "wrote {path} ({named_by}; {n} edit{s} applied)\n",
        path = path.display(),
        s = if n == 1 { "" } else { "s" }
    )
}

/// The landing JSON: the root's facts as today, plus an additive
/// `roster` member when the run also landed the roster, and an additive
/// `split` when this run performed the shape step.
pub fn landed_json(
    path: &Path,
    selected_by: &str,
    n: usize,
    roster: Option<(&Path, &str, usize)>,
    split: Option<&SplitFacts>,
) -> serde_json::Value {
    let mut v = json!({
        "wrote": path.display().to_string(),
        "selected_by": selected_by,
        "edits_applied": n,
    });
    if let Some((rpath, named_by, m)) = roster {
        v["roster"] = json!({
            "wrote": rpath.display().to_string(),
            "named_by": named_by,
            "edits_applied": m,
        });
    }
    if let Some(f) = split {
        v["split"] = split_json(f);
    }
    v
}

/// `no change: <path> left as it is` — the line that makes a second run
/// a no-op rather than a rewrite (spec §4.11; G3).
pub fn no_change_text(path: &Path) -> String {
    format!("no change: {} left as it is\n", path.display())
}

/// The `--dry-run` companion of a landing that has no edits to print:
/// the plan is empty but the base is not the file that is there (a
/// fresh target, `--force`, or a base the shape step moved), so the run
/// would write — the outcome is stated without performing it (spec
/// §4.11's `--dry-run` row: an empty plan is not silence). `because` is
/// the whole clause, so each lane says its own reason.
pub fn dry_run_would_write_text(path: &Path, because: &str) -> String {
    format!(
        "would write {path} ({because}; 0 edits applied)\n",
        path = path.display()
    )
}

/// The `set-enabled` marker for `--dry-run`'s kind column (kept distinct
/// so a plan's kinds are inspectable before they land).
pub fn kind_str(k: EditKind) -> &'static str {
    k.as_str()
}
