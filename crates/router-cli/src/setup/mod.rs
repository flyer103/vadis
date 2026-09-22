//! setup/mod.rs — the run: the target/base decision, the section list,
//! the plan → validate → land sequence, the exit codes (DESIGN §12.14;
//! spec §4.11).
//!
//! `setup` is the second writer of a config file this repository has, and
//! like the other CLI commands it is **not** in the serving path: no
//! request path reaches it, and it writes no trace, no event and no store
//! row.

pub mod anchor;
pub mod edit;
pub mod prompt;
pub mod report;
pub mod sections;

use crate::config_path::{self, Resolved};
use edit::{Edit, EditKind, Plan};
use prompt::{Answer, Prompt};
use sections::{Ask, Section};
use std::io::Write;
use std::path::PathBuf;

/// The `config.example.yaml` embedded in this binary — the default
/// template is the one of **this build's own commit** (spec §4.11's
/// `--from` row), so an installed binary with no example beside it still
/// works.
pub const EMBEDDED_TEMPLATE: &str = include_str!("../../../../config.example.yaml");

/// `router setup`'s flags, as `main` parsed them.
#[derive(Debug, Clone, Default)]
pub struct SetupArgs {
    pub section: Option<String>,
    pub config: Option<String>,
    pub from: Option<String>,
    pub non_interactive: bool,
    pub quick: bool,
    pub print: bool,
    pub json: bool,
    pub check: bool,
    pub dry_run: bool,
    pub force: bool,
    pub backup: bool,
}

/// Exit codes (spec §4.11's table): 0 wrote / no change / printed; 2
/// refused, nothing written; 4 `--check` found a missing variable; 1 I/O.
pub const EXIT_OK: i32 = 0;
pub const EXIT_REFUSED: i32 = 2;
pub const EXIT_CHECK: i32 = 4;
pub const EXIT_IO: i32 = 1;

/// The run. Returns the process exit code; every report surface prints to
/// stdout, every refusal to stderr with the `router:` prefix, mirroring
/// the other commands.
pub fn run(args: SetupArgs) -> i32 {
    // The interactive check is part of the run itself (spec §4.11's "no
    // terminal on stdin"): with stdin not a terminal and
    // --non-interactive absent the command refuses with the working
    // command line in hand, before any question. `--non-interactive`
    // forces the non-interactive channel whatever stdin is.
    let interactive = !args.non_interactive && prompt::stdin_is_terminal();
    let prompter = Prompt::new(interactive);
    run_with_prompt(&args, &prompter)
}

/// The run with an injected answer channel — the in-process tests drive
/// the refusal ladder through this (the interactive path itself is driven
/// by a PTY script, not a Rust harness, per DESIGN §12.14).
pub fn run_with_prompt(args: &SetupArgs, prompter: &Prompt) -> i32 {
    match run_inner(args, prompter) {
        Ok(code) => code,
        Err(Failure::Refused(reason)) => {
            eprintln!("router: setup: {reason}");
            EXIT_REFUSED
        }
        Err(Failure::Io(reason)) => {
            eprintln!("router: setup: {reason}");
            EXIT_IO
        }
    }
}

enum Failure {
    Refused(String),
    Io(String),
}

impl From<String> for Failure {
    fn from(reason: String) -> Self {
        Failure::Refused(reason)
    }
}

fn io(reason: String) -> Failure {
    Failure::Io(reason)
}

fn run_inner(args: &SetupArgs, prompter: &Prompt) -> Result<i32, Failure> {
    // `--print` and `--check` return before any prompt or write (step 2).
    if args.print || args.check {
        return print_or_check(args);
    }

    // Step 1: resolve the target (§4.12's discovery order, write side).
    let target = config_path::resolve_write(args.config.as_deref()).map_err(Failure::Refused)?;
    let template = load_template(args)?;

    // The base: the target's own bytes when it exists and `--force` is
    // absent; the template's otherwise (step 1). An existing target is
    // the normal case — no flag, no confirmation (spec §4.11).
    let target_exists = target.path.is_file();
    let base_text: String = if target_exists && !args.force {
        std::fs::read_to_string(&target.path)
            .map_err(|e| io(format!("cannot read target {}: {e}", target.path.display())))?
    } else {
        template.clone()
    };

    // No terminal on stdin and no --non-interactive: refuse with the
    // working command in hand (spec §4.11; the `interactive` flag the
    // prompter carries is exactly this disjunction).
    if !prompter.interactive && !args.non_interactive {
        let mut out = String::new();
        out.push_str(&format!(
            "stdin is not a terminal and --non-interactive was not given: nothing \
             was written.\nrun the non-interactive form:\n  router setup \
             --non-interactive{}\n",
            args.config
                .as_deref()
                .map(|c| format!(" --config {c}"))
                .unwrap_or_default()
        ));
        for name in missing_env_names(&base_text) {
            out.push_str(&prompt::export_snippet(&name));
            out.push('\n');
        }
        print!("{out}");
        return Ok(EXIT_REFUSED);
    }

    // Section list (step 3): bare / `all` = all seven, in table order.
    let selected: Vec<Section> = match &args.section {
        None => sections::ALL.to_vec(),
        Some(name) if name == "all" => sections::ALL.to_vec(),
        Some(name) => match Section::parse(name) {
            Some(s) => vec![s],
            None => {
                return Err(Failure::Refused(format!(
                    "unknown section `{name}` (sections: server, auth, session, \
                     paths, providers, routing, plugins)"
                )))
            }
        },
    };

    let plan = build_plan(&base_text, &selected, prompter, args)?;

    // `--dry-run`: the plan in place of any landing — checked before the
    // empty-plan branches and before the write itself, so "no write"
    // holds whatever the plan's size and whatever the target's state
    // (spec §4.11's `--dry-run` row; R22-F2: this used to sit below the
    // empty-plan landing, so a fresh target was really written).
    if args.dry_run {
        if args.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&report::dry_run_json(&plan)).unwrap()
            );
        } else {
            let lines: Vec<&str> = base_text.lines().collect();
            let get = move |i: usize| lines.get(i).copied().map(|s| s.to_string());
            print!("{}", report::dry_run_text(get, &plan));
            // An empty plan is not silence: the run's outcome, stated
            // without performing it — the landing it would perform (a
            // base that is not the file that is there), or the
            // no-change line it would print.
            if plan.is_empty() {
                if target_exists && !args.force {
                    print!("{}", report::no_change_text(&target.path));
                } else {
                    print!(
                        "{}",
                        report::dry_run_would_write_text(&target.path, target.selected_by.as_str())
                    );
                }
            }
        }
        return Ok(EXIT_OK);
    }

    // Step 9: an empty plan over an **existing** target writes nothing —
    // this is what makes a second run a no-op rather than a rewrite (G3).
    // A target that does not exist yet still lands: its base was the
    // template (G1 / CONF-67(a) — a fresh all-defaults run writes the
    // template verbatim), so "nothing to ask" is not "nothing to do".
    if plan.is_empty() {
        if !target_exists {
            // The candidate is the base itself here (no edits moved), but
            // the loader gate is not conditional on there being edits: a
            // template that does not load (`--from` a mangled file) must
            // refuse exactly like an edited candidate (spec §4.11's
            // "validation before anything lands").
            if let Err(reason) = crate::config_load::validate_text(&base_text) {
                return Err(Failure::Refused(format!(
                    "the candidate does not load: {reason}"
                )));
            }
            land(&target, base_text.as_bytes())?;
            if args.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report::landed_json(
                        &target.path,
                        target.selected_by.as_str(),
                        0
                    ))
                    .unwrap()
                );
            } else {
                print!(
                    "{}",
                    report::landed_text(&target.path, target.selected_by.as_str(), 0)
                );
            }
            return Ok(EXIT_OK);
        }
        print!("{}", report::no_change_text(&target.path));
        return Ok(EXIT_OK);
    }

    // `--dry-run` prints the plan in place of the landing (step 2).
    if args.dry_run {
        if args.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&report::dry_run_json(&plan)).unwrap()
            );
        } else {
            let lines: Vec<&str> = base_text.lines().collect();
            let get = move |i: usize| lines.get(i).copied().map(|s| s.to_string());
            print!("{}", report::dry_run_text(get, &plan));
        }
        return Ok(EXIT_OK);
    }

    // Step 7: apply → candidate bytes.
    let candidate = edit::apply(&base_text, &plan);

    // Step 8: the same loader, on the candidate, before anything lands.
    // A candidate that does not load is never written, and the message is
    // the loader's own, naming the key (G6).
    if let Err(reason) = crate::config_load::validate_text(&candidate) {
        return Err(Failure::Refused(format!(
            "the candidate does not load: {reason}"
        )));
    }

    // Step 10: `--backup` (or `--force`, which implies it) copies the
    // existing target to `<target>.bak` first — the anchored-edit
    // rollback path (spec §4.11's `--backup` row).
    if (args.backup || args.force) && target_exists {
        let bak = backup_path(&target.path);
        std::fs::copy(&target.path, &bak).map_err(|e| {
            io(format!(
                "cannot back up {} to {}: {e}",
                target.path.display(),
                bak.display()
            ))
        })?;
    }

    // Step 11: land (temp file + sync + rename; mkdir -p; modes).
    land(&target, candidate.as_bytes())?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report::landed_json(
                &target.path,
                target.selected_by.as_str(),
                plan.edits.len()
            ))
            .unwrap()
        );
    } else {
        print!(
            "{}",
            report::landed_text(&target.path, target.selected_by.as_str(), plan.edits.len())
        );
    }
    Ok(EXIT_OK)
}

fn backup_path(p: &std::path::Path) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(".bak");
    PathBuf::from(s)
}

fn load_template(args: &SetupArgs) -> Result<String, Failure> {
    match &args.from {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|e| Failure::Refused(format!("template {path} cannot be read: {e}"))),
        None => Ok(EMBEDDED_TEMPLATE.to_string()),
    }
}

/// The named environment variables the file carries but the ambient
/// environment does not — `--quick`'s "only ask what is missing" and the
/// no-TTY branch's export snippets (spec §4.11). Names only; no value is
/// ever read (the secret boundary).
fn missing_env_names(base: &str) -> Vec<String> {
    let mut out = Vec::new();
    for name in anchor::entry_names(base, "providers", "name") {
        let p = format!("providers[name={name}].api_key_env");
        if let Ok(a) = anchor::resolve_typed(base, &p) {
            if std::env::var_os(&a.value).is_none() {
                out.push(a.value);
            }
        }
    }
    if let Ok(a) = anchor::resolve_typed(base, "server.auth_token_env") {
        if a.enabled && std::env::var_os(&a.value).is_none() {
            out.push(a.value);
        }
    }
    out
}

/// Parse a boolean answer: `true/false/yes/no/y/n/enabled/disabled`
/// (case-insensitive). Anything else is a refusal — an answer that cannot
/// be encoded exactly is never best-effort.
fn parse_bool(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "true" | "yes" | "y" | "enabled" | "on" => Some(true),
        "false" | "no" | "n" | "disabled" | "off" => Some(false),
        _ => None,
    }
}

/// Steps 4–6: walk the sections' rows against the **base**, show each
/// display-only key, ask each askable one, and turn each answer that
/// differs from the value at its anchor into one edit. A key whose anchor
/// did not resolve is a **refusal** when it carries a requested change
/// and a **warning** when it does not (spec §4.11 step 3).
fn build_plan(
    base: &str,
    selected: &[Section],
    prompter: &Prompt,
    args: &SetupArgs,
) -> Result<Plan, Failure> {
    let mut edits: Vec<Edit> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let quick_missing: Vec<String> = if args.quick {
        missing_env_names(base)
    } else {
        Vec::new()
    };

    for section in selected {
        println!("== {} ==", section.name());
        for (path, note) in sections::show_entries(*section, base) {
            if note.is_empty() {
                println!("  {path}");
            } else {
                println!("  {path} — {note}");
            }
        }
        for row in sections::rows_for(*section, base) {
            let anchor_res = anchor::resolve_typed(base, row.path);
            if let Err(e) = &anchor_res {
                // An anchor that resolves nowhere (base **and** template)
                // is the warning shape when nothing is asked of it — but
                // it is never prompted for: a question about a key this
                // file does not carry is how a wizard starts guessing.
                let template_has_it = anchor::resolve_typed(EMBEDDED_TEMPLATE, row.path).is_ok();
                if !template_has_it {
                    warnings.push(format!(
                        "{}: not settable in this file ({}); left alone",
                        row.path,
                        e.reason()
                    ));
                    continue;
                }
            }
            // The shown default: the file's current value, else the
            // commented value, else the template's value at that key
            // (DESIGN §12.14 step 4) — never a constant in code.
            let (default, a) = match &anchor_res {
                Ok(a) => (a.value.clone(), Some(a.clone())),
                Err(_) => match anchor::resolve_typed(EMBEDDED_TEMPLATE, row.path) {
                    Ok(t) => (t.value.clone(), None),
                    Err(_) => unreachable!("handled by the warning branch above"),
                },
            };

            match row.ask {
                Ask::Show => {
                    let note = if row.note.is_empty() {
                        "display-only"
                    } else {
                        row.note
                    };
                    let shown = if a.as_ref().is_some_and(|x| !x.enabled) {
                        format!("{default} (commented out)")
                    } else {
                        default.clone()
                    };
                    println!("  {} = {shown} — {note}", row.path);
                    continue;
                }
                Ask::Line | Ask::Enum(_) | Ask::Bool => {}
            }

            // `--quick`: only the items `--check` reports unsatisfied —
            // the rows whose named environment variable is missing.
            if args.quick {
                let names_a_variable =
                    row.path.ends_with(".api_key_env") || row.path == "server.auth_token_env";
                let its_variable_is_missing = quick_missing.contains(&default);
                if !(names_a_variable && its_variable_is_missing) {
                    continue;
                }
            }

            let question = match row.ask {
                Ask::Enum(items) => prompt::render_enum(row.path, items, &default),
                Ask::Bool if row.kind == EditKind::SetEnabled => {
                    // The enabled question's default is the current state.
                    let state = a.as_ref().map(|x| x.enabled).unwrap_or(false);
                    prompt::render(row.path, if state { "enabled" } else { "disabled" }, "y/n")
                }
                _ => prompt::render(row.path, &default, ""),
            };
            let answer = match prompter.line(&question) {
                Answer::Default => {
                    if let Ask::Enum(_) = row.ask {
                        println!("Skipped (keeping current)");
                    }
                    // An enabled-row's default answer is the *state*
                    // (enabled/disabled), not the key's value — pressing
                    // return on `auth_token_env` keeps the comment marker
                    // where it is.
                    if row.kind == EditKind::SetEnabled {
                        let state = a.as_ref().map(|x| x.enabled).unwrap_or(false);
                        if state {
                            "enabled".to_string()
                        } else {
                            "disabled".to_string()
                        }
                    } else {
                        default.clone()
                    }
                }
                Answer::Eof => {
                    return Err(Failure::Refused(
                        "EOF mid-run: nothing was written (a half-answered run \
                         never lands)"
                            .to_string(),
                    ))
                }
                Answer::Value(v) => v,
            };

            // The two askable shapes that are not free lines.
            let answer = match row.ask {
                Ask::Enum(items) => {
                    if !items.contains(&answer.as_str()) {
                        return Err(Failure::Refused(format!(
                            "{}: `{answer}` is not one of {}",
                            row.path,
                            items.join("|")
                        )));
                    }
                    answer
                }
                Ask::Bool if row.kind == EditKind::SetEnabled => {
                    let want_enabled = match parse_bool(&answer) {
                        Some(b) => b,
                        None => {
                            return Err(Failure::Refused(format!(
                                "{}: `{answer}` is not a boolean (y/n)",
                                row.path
                            )))
                        }
                    };
                    let is_enabled = a.as_ref().map(|x| x.enabled).unwrap_or(false);
                    if want_enabled == is_enabled {
                        // No change for this row.
                        String::new()
                    } else {
                        // Encoded below into a marker edit.
                        format!("__enable__{want_enabled}")
                    }
                }
                _ => answer,
            };

            if row.kind == EditKind::SetEnabled {
                if let Some(want) = answer.strip_prefix("__enable__") {
                    let want: bool = want.parse().unwrap();
                    let a = a.as_ref().expect("an enabled edit needs its anchor");
                    let marker = a.marker.clone().ok_or_else(|| {
                        Failure::Refused(format!(
                            "{}: cannot toggle the comment marker (no marker on \
                             the key's line)",
                            row.path
                        ))
                    })?;
                    let old = base
                        .lines()
                        .nth(a.line)
                        .map(|l| l.trim().to_string())
                        .unwrap_or_default();
                    edits.push(Edit {
                        kind: EditKind::SetEnabled,
                        path: row.path.to_string(),
                        line: a.line,
                        range: marker,
                        replacement: if want {
                            String::new()
                        } else {
                            "# ".to_string()
                        },
                        old,
                    });
                }
                continue;
            }

            if answer == default {
                continue;
            }
            // A requested change on a key whose anchor did not resolve is
            // a refusal — never a guess, never half a write.
            let Some(a) = a else {
                return Err(Failure::Refused(format!(
                    "{}: not settable in this file ({}); refusing to write anything",
                    row.path,
                    anchor_res.unwrap_err().reason()
                )));
            };
            // Value validation before encoding (durations; the loader
            // validates the whole candidate again afterwards).
            if row.is_duration() && !edit::is_duration(&answer) {
                return Err(Failure::Refused(format!(
                    "{}: `{answer}` is not a duration in the §12.5 grammar \
                     (e.g. 60s, 10m, 1h30m)",
                    row.path
                )));
            }
            let old = base
                .lines()
                .nth(a.line)
                .map(|l| l.trim().to_string())
                .unwrap_or_default();
            edits.push(Edit {
                kind: EditKind::SetValue,
                path: row.path.to_string(),
                line: a.line,
                range: a.range.clone(),
                replacement: edit::encode(a.quote, &answer),
                old,
            });
        }
    }
    for w in warnings {
        eprintln!("router: setup: warning: {w}");
    }
    Ok(Plan::build(edits)?)
}

/// `--print` / `--check` (step 2): no prompt, no write. Both resolve
/// candidates 1–3; `--check` with no target refuses (exit 2), `--print`
/// with no target prints the template's values, labelled as such.
fn print_or_check(args: &SetupArgs) -> Result<i32, Failure> {
    let (target, text, from_template) = match config_path::resolve_read(args.config.as_deref()) {
        Ok(r) => {
            let text = std::fs::read_to_string(&r.path)
                .map_err(|e| io(format!("cannot read {}: {e}", r.path.display())))?;
            (r, text, false)
        }
        Err(reason) => {
            if args.check || args.config.is_some() {
                // `--check` has no target ⇒ exit 2; an explicit --config
                // that is missing never falls through (§4.12 row 1).
                return Err(Failure::Refused(reason));
            }
            // `--print` with no target: the template's values, labelled.
            let path = config_path::resolve_write(None)
                .map_err(Failure::Refused)?
                .path;
            (
                Resolved {
                    path,
                    selected_by: config_path::SelectedBy::XdgCreated,
                },
                EMBEDDED_TEMPLATE.to_string(),
                true,
            )
        }
    };

    if args.check {
        // Load with the same loader `serve` runs, then probe every name
        // the file carries (presence only — the secret boundary).
        let cfg: router_core::config::RouterConfig = crate::config_load::validate_text(&text)
            .map_err(|e| Failure::Refused(format!("config file {}: {e}", target.path.display())))?;
        let rows = report::check_rows(&cfg);
        let all_present = rows.iter().all(|(_, s)| *s == report::KeyState::Present);
        if args.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&report::check_json(
                    &target.path,
                    target.selected_by.as_str(),
                    &rows
                ))
                .unwrap()
            );
        } else {
            print!("{}", report::check_text(&rows));
        }
        return Ok(if all_present { EXIT_OK } else { EXIT_CHECK });
    }

    // `--print`: every section's keys with the value the file carries
    // (and the state of a key the template ships commented out).
    let mut rows = Vec::new();
    for section in sections::ALL {
        for (path, note) in sections::show_entries(section, &text) {
            rows.push(report::PrintRow {
                path,
                value: note,
                enabled: true,
                note: "",
                from_template,
            });
        }
        for row in sections::rows_for(section, &text) {
            match anchor::resolve_typed(&text, row.path) {
                Ok(a) => rows.push(report::PrintRow {
                    path: row.path.to_string(),
                    value: a.value,
                    enabled: a.enabled,
                    note: row.note,
                    from_template,
                }),
                Err(e) => rows.push(report::PrintRow {
                    path: row.path.to_string(),
                    value: format!("({})", e.reason()),
                    enabled: false,
                    note: row.note,
                    from_template,
                }),
            }
        }
    }
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report::print_json(
                &rows,
                &target.path,
                target.selected_by.as_str()
            ))
            .unwrap()
        );
    } else {
        print!("{}", report::print_text(&rows, &target.path, from_template));
    }
    Ok(EXIT_OK)
}

/// Landing (step 11): candidate to `<target>.setup.tmp` **in the target's
/// directory**, `sync_all`, `rename` over the target; the temporary file
/// is removed on any failure, so nothing lands partially and no other
/// process can observe a half-written config. The target's directory is
/// created when missing (`mkdir -p`): a file this run creates is `0600`
/// — exact under any umask, `OpenOptions::mode` can only have bits
/// cleared by one — and a directory this run creates is `0700` (set
/// explicitly after `create_dir_all`, which cannot express a mode).
/// Nothing that already exists is re-moded.
fn land(target: &Resolved, bytes: &[u8]) -> Result<(), Failure> {
    let dir = target
        .path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| {
            Failure::Refused(format!("target {} has no directory", target.path.display()))
        })?;
    if !dir.exists() {
        if let Err(e) = std::fs::create_dir_all(dir) {
            return Err(Failure::Refused(format!(
                "cannot create target directory {}: {e}",
                dir.display()
            )));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    let tmp = target.path.with_extension("setup.tmp");
    {
        use std::io::Write as _;
        #[cfg(unix)]
        let file = {
            use std::os::unix::fs::OpenOptionsExt as _;
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)
                .map_err(|e| io(format!("cannot write {}: {e}", tmp.display())))?
        };
        #[cfg(not(unix))]
        let file = std::fs::File::create(&tmp)
            .map_err(|e| io(format!("cannot write {}: {e}", tmp.display())))?;
        let mut file = file;
        if let Err(e) = file.write_all(bytes) {
            let _ = std::fs::remove_file(&tmp);
            return Err(io(format!("cannot write {}: {e}", tmp.display())));
        }
        if let Err(e) = file.sync_all() {
            let _ = std::fs::remove_file(&tmp);
            return Err(io(format!("cannot sync {}: {e}", tmp.display())));
        }
    }
    if let Err(e) = std::fs::rename(&tmp, &target.path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(io(format!(
            "cannot move {} over {}: {e}",
            tmp.display(),
            target.path.display()
        )));
    }
    let _ = std::io::stdout().flush();
    Ok(())
}

// ---------------------------------------------------------------------------
// Integration tests: the file contract (G1–G8; CONF-67…70's shape). The
// interactive path itself is driven by a PTY script, not a Rust harness
// (DESIGN §12.14); these drive the same code through `run_with_prompt`
// with a scripted answer channel.
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod testutil {
    pub(crate) fn temp_root(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "router-setup-{name}-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// G2's relation, never a snapshot: the counts of `source:` citations
    /// and TODO markers in the candidate equal the base's.
    pub(crate) fn provenance_counts(text: &str) -> (usize, usize) {
        (
            text.matches("source:").count(),
            text.matches("TODO verify against official source").count(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::{provenance_counts, temp_root};
    use super::*;
    use prompt::Prompt;

    fn scripted(answers: &[&str]) -> Prompt {
        let p = Prompt::new(true);
        p.load_answers(answers.iter().map(|s| s.to_string()).collect());
        p
    }

    fn non_interactive() -> Prompt {
        Prompt::new(false)
    }

    fn args(config: &std::path::Path) -> SetupArgs {
        SetupArgs {
            non_interactive: true,
            config: Some(config.display().to_string()),
            ..Default::default()
        }
    }

    /// G1 / CONF-67(a): a fresh target, every answer at its default ⇒ the
    /// file's bytes are identical to the template's, and it loads through
    /// the same loader `serve` runs.
    #[test]
    fn g1_defaults_write_the_template_verbatim() {
        let dir = temp_root("g1");
        let target = dir.join("config.yaml");
        assert_eq!(run_with_prompt(&args(&target), &non_interactive()), 0);
        let written = std::fs::read(&target).unwrap();
        assert_eq!(
            written,
            EMBEDDED_TEMPLATE.as_bytes(),
            "G1: the fresh all-defaults file must be byte-identical to the template"
        );
        assert!(crate::config_load::validate_text(EMBEDDED_TEMPLATE).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// G3 / CONF-69: the same answers twice ⇒ the second run writes
    /// nothing (bytes **and** mtime unchanged).
    #[test]
    fn g3_second_run_is_a_no_op() {
        let dir = temp_root("g3");
        let target = dir.join("config.yaml");
        run_with_prompt(&args(&target), &non_interactive());
        let (b1, m1) = (
            std::fs::read(&target).unwrap(),
            std::fs::metadata(&target).unwrap().modified().unwrap(),
        );
        assert_eq!(run_with_prompt(&args(&target), &non_interactive()), 0);
        let (b2, m2) = (
            std::fs::read(&target).unwrap(),
            std::fs::metadata(&target).unwrap().modified().unwrap(),
        );
        assert_eq!(b1, b2, "bytes unchanged");
        assert_eq!(m1, m2, "mtime unchanged (nothing was written)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// G2 / CONF-67(b): with k answered changes, every byte outside those
    /// k lines' own extents is identical to the base, and the
    /// provenance counts equal the base's.
    #[test]
    fn g2_one_answered_change_moves_only_its_line() {
        let dir = temp_root("g2");
        let target = dir.join("config.yaml");
        run_with_prompt(&args(&target), &non_interactive());
        // One answered change: server.addr, the section's first question.
        run_with_prompt(
            &SetupArgs {
                section: Some("server".to_string()),
                config: Some(target.display().to_string()),
                ..Default::default()
            },
            &scripted(&["127.0.0.1:9911", "", ""]),
        );
        let base = EMBEDDED_TEMPLATE;
        let after = std::fs::read_to_string(&target).unwrap();
        let a = anchor::resolve_typed(base, "server.addr").unwrap();
        let base_lines: Vec<&str> = base.lines().collect();
        let after_lines: Vec<&str> = after.lines().collect();
        assert_eq!(base_lines.len(), after_lines.len());
        let mut moved = Vec::new();
        for (i, (b, c)) in base_lines.iter().zip(after_lines.iter()).enumerate() {
            if b != c {
                moved.push(i);
            }
        }
        assert_eq!(moved, vec![a.line], "only the answered line may move");
        assert!(after_lines[a.line].contains("127.0.0.1:9911"));
        assert_eq!(provenance_counts(base), provenance_counts(&after));
        assert!(crate::config_load::validate_text(&after).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// G4 / CONF-68: the refusal ladder — (a) an anchor the file does not
    /// carry, with a requested change; (b) an ambiguous anchor; (c) a key
    /// whose value is not a single-line scalar; (d) a candidate that does
    /// not load — each leaves the target byte-identical and mtime
    /// unchanged, with no temporary file behind.
    #[test]
    fn g4_the_refusal_ladder_leaves_the_target_untouched() {
        let dir = temp_root("g4");
        let target = dir.join("config.yaml");
        run_with_prompt(&args(&target), &non_interactive());
        let before = std::fs::read(&target).unwrap();
        let before_mtime = std::fs::metadata(&target).unwrap().modified().unwrap();

        // (a) the anchor is gone, and the answer asks for a change:
        // default falls back to the template's value, so any other answer
        // is a requested change on an unresolvable anchor.
        let no_addr = target.clone();
        {
            let mangled = EMBEDDED_TEMPLATE.replace("  addr:", "  addr_x:");
            std::fs::write(dir.join("no-addr.yaml"), &mangled).unwrap();
            let code = run_with_prompt(
                &SetupArgs {
                    section: Some("server".to_string()),
                    config: Some(no_addr.display().to_string()),
                    from: Some(dir.join("no-addr.yaml").display().to_string()),
                    force: true,
                    ..Default::default()
                },
                // force ⇒ base = the mangled template; answers: addr, then
                // the two timeouts at default.
                &scripted(&["127.0.0.1:9911", "", ""]),
            );
            assert_eq!(
                code, 2,
                "(a) a missing anchor with a requested change refuses"
            );
        }

        // (b) the anchor resolves to two lines.
        {
            let mangled = EMBEDDED_TEMPLATE.replace(
                "  addr: \"127.0.0.1:8790\"",
                "  addr: \"127.0.0.1:8790\"\n  addr: \"127.0.0.1:8791\"",
            );
            std::fs::write(dir.join("dup-addr.yaml"), &mangled).unwrap();
            let dup = dir.join("dup.yaml");
            let code = run_with_prompt(
                &SetupArgs {
                    section: Some("server".to_string()),
                    config: Some(dup.display().to_string()),
                    from: Some(dir.join("dup-addr.yaml").display().to_string()),
                    force: true,
                    ..Default::default()
                },
                &scripted(&["127.0.0.1:9911", "", ""]),
            );
            assert_eq!(code, 2, "(b) an ambiguous anchor refuses");
            assert!(!dup.exists(), "(b) nothing landed");
        }

        // (c) the server block rewritten as a flow mapping.
        {
            let mangled = EMBEDDED_TEMPLATE
                .replace("server:\n", "server: {addr: \"127.0.0.1:8790\"}  #\n")
                .replace("  addr: \"127.0.0.1:8790\"        # local-first; with server.auth_token_env set, bind it anywhere you can reach it — ingest is then authenticated\n", "");
            std::fs::write(dir.join("flow.yaml"), &mangled).unwrap();
            // It need not even load — the anchor must refuse first.
            let flow = dir.join("flow-target.yaml");
            let code = run_with_prompt(
                &SetupArgs {
                    section: Some("server".to_string()),
                    config: Some(flow.display().to_string()),
                    from: Some(dir.join("flow.yaml").display().to_string()),
                    force: true,
                    ..Default::default()
                },
                &scripted(&["127.0.0.1:9911", "", ""]),
            );
            assert_eq!(code, 2, "(c) a non-scalar value refuses");
            assert!(!flow.exists(), "(c) nothing landed");
        }

        // (d) the candidate does not load: an addr that is not a listen
        // address passes the anchor, fails `validate()` (listen_addr).
        {
            let fresh = dir.join("fresh-d.yaml");
            let code = run_with_prompt(
                &SetupArgs {
                    section: Some("server".to_string()),
                    config: Some(fresh.display().to_string()),
                    ..Default::default()
                },
                &scripted(&["not an address", "", ""]),
            );
            assert_eq!(
                code, 2,
                "(d) a candidate that does not load is never written"
            );
            assert!(!fresh.exists(), "(d) nothing landed");
            assert!(
                !dir.join("fresh-d.yaml.setup.tmp").exists(),
                "(d) no temporary file is left behind"
            );
        }

        // The untouched-target half: whatever happened above, the original
        // target never moved.
        assert_eq!(before, std::fs::read(&target).unwrap());
        assert_eq!(
            before_mtime,
            std::fs::metadata(&target).unwrap().modified().unwrap()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// G5 / CONF-70 (the in-process half): the probe returns states, and
    /// `--check`'s rows carry names only — the canary itself (an actual
    /// value in output) is asserted by the PTY/rig scripts.
    #[test]
    fn check_exit_code_by_environment() {
        let dir = temp_root("check");
        let target = dir.join("config.yaml");
        std::fs::write(&target, EMBEDDED_TEMPLATE).unwrap();
        let code = run_with_prompt(
            &SetupArgs {
                check: true,
                config: Some(target.display().to_string()),
                ..Default::default()
            },
            &non_interactive(),
        );
        // The test process's environment decides which of the two.
        assert!(code == 0 || code == 4, "--check exits 0 or 4 (got {code})");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// §4.12 / CONF-79 (the writer half): a missing target directory is
    /// created (`0700` when this run created it) and the created file is
    /// `0600`, read back from the filesystem.
    #[test]
    fn writer_creates_directory_and_file_with_modes() {
        let dir = temp_root("modes");
        let target = dir.join("nested/deeper/config.yaml");
        assert_eq!(run_with_prompt(&args(&target), &non_interactive()), 0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let file_mode = std::fs::metadata(&target).unwrap().permissions().mode();
            assert_eq!(file_mode & 0o777, 0o600, "created file is 0600");
            let dir_mode = std::fs::metadata(dir.join("nested/deeper"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(dir_mode & 0o777, 0o700, "created directory is 0700");
            // An existing directory is never re-moded.
            let outer = dir.join("nested");
            let before = std::fs::metadata(&outer).unwrap().permissions().mode();
            let again = dir.join("nested/other/config.yaml");
            assert_eq!(run_with_prompt(&args(&again), &non_interactive()), 0);
            let after = std::fs::metadata(&outer).unwrap().permissions().mode();
            assert_eq!(before, after, "an existing directory is not re-moded");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The refusal of an unknown section (exit 2, nothing written).
    #[test]
    fn unknown_section_is_refused() {
        let dir = temp_root("unknown");
        let target = dir.join("x.yaml");
        let code = run_with_prompt(
            &SetupArgs {
                section: Some("nope".to_string()),
                ..Default::default()
            },
            &non_interactive(),
        );
        assert_eq!(code, 2);
        assert!(!target.exists(), "refused ⇒ nothing written");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `--force` implies `--backup`: the replaced file survives at
    /// `<target>.bak` (spec §4.11's rows).
    #[test]
    fn force_implies_backup() {
        let dir = temp_root("force");
        let target = dir.join("config.yaml");
        run_with_prompt(&args(&target), &non_interactive());
        let first = std::fs::read(&target).unwrap();
        let code = run_with_prompt(
            &SetupArgs {
                force: true,
                section: Some("server".to_string()),
                config: Some(target.display().to_string()),
                ..Default::default()
            },
            &scripted(&["127.0.0.1:9911", "", ""]),
        );
        assert_eq!(code, 0);
        assert_eq!(std::fs::read(dir.join("config.yaml.bak")).unwrap(), first);
        let after = std::fs::read_to_string(&target).unwrap();
        assert_ne!(after.as_bytes(), first.as_slice());
        assert!(crate::config_load::validate_text(&after).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A second run over an edited file: the file's own bytes are the
    /// base, and only the answered keys move (spec §4.11: an existing
    /// target is the normal case).
    #[test]
    fn existing_target_is_the_base() {
        let dir = temp_root("existing");
        let target = dir.join("config.yaml");
        run_with_prompt(&args(&target), &non_interactive());
        // Hand-edit: a note the wizard never writes.
        let hand = EMBEDDED_TEMPLATE.replace(
            "  request_timeout: 10m",
            "  request_timeout: 11m          # my own note",
        );
        std::fs::write(&target, &hand).unwrap();
        let code = run_with_prompt(
            &SetupArgs {
                section: Some("server".to_string()),
                config: Some(target.display().to_string()),
                ..Default::default()
            },
            // addr changes; timeouts keep the hand-edited values.
            &scripted(&["127.0.0.1:9911", "", ""]),
        );
        assert_eq!(code, 0);
        let after = std::fs::read_to_string(&target).unwrap();
        assert!(
            after.contains("# my own note"),
            "hand notes survive verbatim"
        );
        assert!(after.contains("request_timeout: 11m"));
        assert!(after.contains("127.0.0.1:9911"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R22-F2: `--dry-run` lands nothing — whatever the plan's size and
    /// whatever the target's state. (a) is the exact run that used to
    /// write the template verbatim (empty plan, fresh target); (c) adds
    /// that `--dry-run` implies no backup either — `--backup` is "before
    /// a write", and there is no write (spec §4.11's rows).
    #[test]
    fn r22_f2_dry_run_never_lands() {
        let dir = temp_root("dryrun");
        // (a) fresh target, empty plan, non-interactive.
        {
            let target = dir.join("fresh.yaml");
            let dir_before = std::fs::metadata(&dir).unwrap().modified().unwrap();
            let code = run_with_prompt(
                &SetupArgs {
                    dry_run: true,
                    non_interactive: true,
                    config: Some(target.display().to_string()),
                    ..Default::default()
                },
                &non_interactive(),
            );
            assert_eq!(code, 0);
            assert!(
                !target.exists(),
                "(a) --dry-run: the fresh target is not created"
            );
            assert!(
                !dir.join("fresh.yaml.setup.tmp").exists(),
                "(a) --dry-run: no temporary file"
            );
            assert_eq!(
                dir_before,
                std::fs::metadata(&dir).unwrap().modified().unwrap(),
                "(a) --dry-run: the target's directory mtime is unchanged"
            );
        }
        // (b) fresh target with one answered change: the plan is built
        // (and printed by the walk) and still nothing lands.
        {
            let target = dir.join("fresh2.yaml");
            let code = run_with_prompt(
                &SetupArgs {
                    dry_run: true,
                    non_interactive: true,
                    section: Some("server".to_string()),
                    config: Some(target.display().to_string()),
                    ..Default::default()
                },
                &scripted(&["127.0.0.1:9911", "", ""]),
            );
            assert_eq!(code, 0);
            assert!(!target.exists(), "(b) --dry-run with edits: nothing lands");
        }
        // (c) existing target + --force + empty plan: the run would
        // write, and does not.
        {
            let target = dir.join("exists.yaml");
            run_with_prompt(&args(&target), &non_interactive());
            let (before, m) = (
                std::fs::read(&target).unwrap(),
                std::fs::metadata(&target).unwrap().modified().unwrap(),
            );
            let code = run_with_prompt(
                &SetupArgs {
                    dry_run: true,
                    force: true,
                    non_interactive: true,
                    config: Some(target.display().to_string()),
                    ..Default::default()
                },
                &non_interactive(),
            );
            assert_eq!(code, 0);
            assert_eq!(
                before,
                std::fs::read(&target).unwrap(),
                "(c) bytes unchanged"
            );
            assert_eq!(
                m,
                std::fs::metadata(&target).unwrap().modified().unwrap(),
                "(c) mtime unchanged"
            );
            assert!(
                !dir.join("exists.yaml.bak").exists(),
                "(c) --dry-run implies no backup (there is no write to precede)"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
