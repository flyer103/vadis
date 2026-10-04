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
pub mod split;

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

/// The roster half of the embedded template (spec §4.14): the shipped
/// example is a **pair** since the split, so the roster template is
/// embedded for the same reason the root is — an installed binary with no
/// example beside it still writes a complete pair (spec §4.11's `--from`
/// row: "the embedded **roster** template for the roster target").
pub const EMBEDDED_ROSTER: &str = include_str!("../../../../providers.example.yaml");

/// The rule file embedded in this binary (ADR-046 D2): the shipped
/// template's `plugins` entry names `./rules/tool_output.toml`, resolved
/// against the config file's own directory — a fresh landing outside the
/// repository would otherwise carry a named absence. Embedded for exactly
/// the reason the root and the roster are: an installed binary with no
/// example beside it must still land a working bundle. The repository's
/// `rules/tool_output.toml` is **read** here, never edited (the wizard
/// composes no rule content — ADR-046 D5).
pub const EMBEDDED_RULES: &str = include_str!("../../../../rules/tool_output.toml");

/// The root's shape, as the **loader parses it** (spec §4.14) — a parse
/// fact read off the same wire type the loader parses, never a text scan.
/// Three cases, and the third is "not this run's business":
///
/// - `Split`: the root names its roster; the pair is already the shape.
/// - `Inline`: the root carries the roster itself — the shape the shape
///   step (ADR-038) normalizes before anything is planned.
/// - `Refused`: neither key, **both** keys, or a text that does not parse.
///   Nothing is extracted and no roster lane is built: the parse refusal
///   is the candidate gate's to issue (the loader), exactly as before the
///   split — the wizard configures a file, it does not repair one that
///   does not load (ADR-038 D5).
enum Shape {
    Split { written: String, resolved: PathBuf },
    Inline,
    Refused,
}

/// Read the shape off a root text, resolving a named roster by §4.1's
/// rule against the root's own directory (never the CWD).
fn shape_of(root_text: &str, config_dir: &std::path::Path) -> Shape {
    let Ok(root) = serde_yaml::from_str::<vadis_core::config::RootFile>(root_text) else {
        return Shape::Refused;
    };
    match (&root.providers, &root.providers_file) {
        (None, Some(written)) => Shape::Split {
            written: written.clone(),
            resolved: crate::config_load::resolve(config_dir, written),
        },
        (Some(_), None) => Shape::Inline,
        _ => Shape::Refused,
    }
}

/// The shipped root template's own `providers_file` line, and the value it
/// names — the wizard's default roster name and the line a normalization
/// writes in the header line's place (spec §4.11: a default comes from the
/// file, never from a constant in code; ADR-038 D2/D4).
///
/// The `expect` cannot fire for a template this build ships: the unit test
/// `the_shipped_template_names_its_roster` pins the anchor, and a binary
/// whose template lost the key would have no default name to write — the
/// run refuses rather than inventing one (that refusal is the panic's
/// replacement in a future build with a selectable template).
fn shipped_roster_line() -> (String, String) {
    let a = anchor::resolve_typed(EMBEDDED_TEMPLATE, "providers_file")
        .expect("the shipped root template names its roster (spec §4.14)");
    let line = EMBEDDED_TEMPLATE
        .lines()
        .nth(a.line)
        .expect("the anchor's own line")
        .trim_end()
        .to_string();
    (a.value, line)
}

/// The rule file the base root names, read from the base's own text
/// (ADR-046 D1): `plugins[id=<entry>].config.rules_file` on its
/// `builtin/transform_rules` entry, resolved by §4.1's rule against the
/// config file's own directory (never the CWD). The first entry of the
/// kind that carries the key wins — `serve`'s own assembly rule (first
/// hit wins), so the writer materializes the file the reader would
/// mount. A base that names no rule file (no `builtin/transform_rules`
/// entry, or one without `config.rules_file`) returns `None`: **no**
/// third lane, nothing materialized. Never a constant in code — the
/// same rule the roster's name follows.
fn rule_file_target(base: &str, config_dir: &std::path::Path) -> Option<PathBuf> {
    // The first `builtin/transform_rules` entry that carries the key
    // wins — `serve`'s own assembly rule (first hit wins), so the
    // writer materializes the file the reader would mount. An entry of
    // the kind without `config.rules_file` falls through to the next of
    // the kind, exactly as a failed entry does at assembly.
    let ids = anchor::entry_names(base, "plugins", "id");
    let mut candidates = ids.iter().filter(|id| {
        anchor::resolve_typed(base, &format!("plugins[id={id}].kind"))
            .map(|a| a.value == "builtin/transform_rules")
            .unwrap_or(false)
    });
    let path = candidates.find_map(|id| {
        let a = anchor::resolve_typed(base, &format!("plugins[id={id}].config.rules_file")).ok()?;
        let written = a.value.trim().to_string();
        (!written.is_empty()).then(|| crate::config_load::resolve(config_dir, &written))
    });
    path
}

/// Step 1c — **the rule-file lane** (ADR-046; DESIGN §12.14 step 1c):
/// the third write target, in the roster lane's own shape. The path
/// comes from the base root's own text; the bytes are the embedded rule
/// template's when the file is created or replaced, and the file's own
/// when it is untouched (so the plan is empty and the wizard composes no
/// rule content — D5). The replacement rule is the roster lane's own,
/// read for a file no section owns:
/// `replaced = !exists || (force && the plugins section is selected)`
/// — D3's create arm unqualified by section, D4's replacement arm
/// qualified by it. A base that names no rule file has no lane.
fn build_rule_lane(
    base_root: &str,
    config_dir: &std::path::Path,
    selected: &[Section],
    force: bool,
) -> Result<Option<Lane>, Failure> {
    let Some(path) = rule_file_target(base_root, config_dir) else {
        return Ok(None);
    };
    let exists = path.is_file();
    let plugins_selected = selected.contains(&Section::Plugins);
    let replaced = !exists || (force && plugins_selected);
    let rule_base: String = if exists && !replaced {
        std::fs::read_to_string(&path)
            .map_err(|e| io(format!("cannot read rule file {}: {e}", path.display())))?
    } else {
        EMBEDDED_RULES.to_string()
    };
    Ok(Some(Lane {
        path,
        existed: exists,
        replaced,
        reshaped: false,
        overwrite: false,
        base: rule_base,
        plan: Plan::default(),
    }))
}

/// `vadis setup`'s flags, as `main` parsed them.
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
/// stdout, every refusal to stderr with the `vadis:` prefix, mirroring
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
            eprintln!("vadis: setup: {reason}");
            EXIT_REFUSED
        }
        Err(Failure::Io(reason)) => {
            eprintln!("vadis: setup: {reason}");
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

/// One file the run may write: the root lane always exists; the roster
/// lane exists when the base root names a roster **or** carries it inline
/// (the shape step creates the file it names) — spec §4.11's target-file
/// column; §4.14. Since ADR-046 a third lane exists beside them: the
/// rule file the base root names at
/// `plugins[id=<entry>].config.rules_file` (the rule-file lane, DESIGN
/// §12.14 step 1c). `base` is the bytes the plan edits — the file's own
/// bytes, the lane's template when the run starts it fresh, or the block
/// the shape step moved. The rule lane's plan is always empty (the
/// wizard composes no rule content, ADR-046 D5).
struct Lane {
    path: PathBuf,
    /// The file was there before the run.
    existed: bool,
    /// The run replaces the file wholesale (fresh, or `--force` over a
    /// section that targets it) — the fact `--force ⇒ --backup` keys off.
    replaced: bool,
    /// **This run reshaped the base**: the inline roster moved out of
    /// this file (ADR-038). The base is therefore not the file that is
    /// there even when the plan is empty — the one landing reason that is
    /// neither an edit nor a wholesale replacement.
    reshaped: bool,
    /// The run overwrites a file it did not write, as part of the shape
    /// step: `<file>.bak` is taken **whatever** the flags say (ADR-038
    /// D6 — the operator's bytes are kept by name, and the run does not
    /// stop for a condition it can make safe).
    overwrite: bool,
    base: String,
    plan: Plan,
}

impl Lane {
    /// Whether this lane lands: an edit, a base that is not the file that
    /// is there (a fresh lane, a `--force`d one, or one the shape step
    /// moved), or a file this run overwrites — §4.11's empty-plan rows,
    /// read per file of the pair.
    fn lands(&self) -> bool {
        !self.plan.is_empty() || !self.existed || self.replaced || self.reshaped || self.overwrite
    }
}

fn run_inner(args: &SetupArgs, prompter: &Prompt) -> Result<i32, Failure> {
    // `--print` and `--check` return before any prompt or write (step 2).
    if args.print || args.check {
        return print_or_check(args);
    }

    // Step 1: resolve the target (§4.12's discovery order, write side).
    let target = config_path::resolve_write(args.config.as_deref()).map_err(Failure::Refused)?;
    let target_exists = target.path.is_file();
    let config_dir = target
        .path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let existing: Option<String> = if target_exists {
        Some(
            std::fs::read_to_string(&target.path)
                .map_err(|e| io(format!("cannot read target {}: {e}", target.path.display())))?,
        )
    } else {
        None
    };
    // `--from` is read eagerly, as before the split: an unreadable
    // template is a refusal whether or not the run ends up using it.
    let from_text: Option<String> = match &args.from {
        Some(path) => Some(
            std::fs::read_to_string(path)
                .map_err(|e| Failure::Refused(format!("template {path} cannot be read: {e}")))?,
        ),
        None => None,
    };

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

    // `--from`'s meaning follows the file the run starts from (spec
    // §4.11's `--from` / `--force` rows: "replaces the file it starts
    // from — for the `providers` section under the split form that is
    // how a roster is replaced as a unit"). Exactly one run shape reads
    // `--from` as the roster's replacement: the providers-section run
    // over an **existing** split root — the operator's own root is what
    // names the roster being swapped. Every other run reads `--from` as
    // the root template, as before the split (a fresh start begins from
    // a root; ADR-037 D9: a unit replacement, never an insertion).
    let existing_shape = existing.as_deref().map(|text| shape_of(text, &config_dir));
    let roster_scoped_from = from_text.is_some()
        && args.section.as_deref() == Some("providers")
        && matches!(existing_shape, Some(Shape::Split { .. }));
    let root_template: &str = if roster_scoped_from {
        EMBEDDED_TEMPLATE
    } else {
        from_text.as_deref().unwrap_or(EMBEDDED_TEMPLATE)
    };
    let roster_template: &str = if roster_scoped_from {
        from_text.as_deref().unwrap()
    } else {
        EMBEDDED_ROSTER
    };

    // The root lane's base: the target's own bytes when it exists and the
    // run does not replace it; the root template's otherwise (step 1).
    // `--force` replaces the root when a selected section targets it **in
    // the existing shape** — so the providers-only `--force` run over a
    // split root leaves the root alone. Anything the shape step does not
    // recognize as a pair is the root's to replace, as before the split.
    let root_targeted = match existing_shape {
        Some(Shape::Split { .. }) => selected
            .iter()
            .any(|s| s.target(true) == sections::TargetFile::Root),
        _ => true, // a fresh target, or a shape §4.14 refuses: the root is written
    };
    let root_replaced = !target_exists || (args.force && root_targeted);
    let mut root_base: String = match &existing {
        Some(text) if !root_replaced => text.clone(),
        _ => root_template.to_string(),
    };

    // Step 1b — **the shape step** (ADR-038; spec §4.11): a writing run
    // never leaves the roster inline. A base root whose *parsed* shape is
    // inline-with-`providers` is normalized here, before any question and
    // before any plan: the block's bytes become the roster file's bytes,
    // and the shipped root template's own `providers_file:` line takes
    // the header line's place. `Refused` — both keys written, neither, or
    // a text that does not parse — is left alone: the loader's refusal is
    // the run's outcome, not the wizard's repair job (D5).
    let (shipped_name, shipped_line) = shipped_roster_line();
    let mut split_move: Option<(split::Split, PathBuf)> = None;
    let roster_lane: Option<Lane> = match shape_of(&root_base, &config_dir) {
        Shape::Split { resolved, .. } => {
            let roster_exists = resolved.is_file();
            let roster_targeted = selected
                .iter()
                .any(|s| s.target(true) == sections::TargetFile::Roster);
            let roster_replaced = !roster_exists || (args.force && roster_targeted);
            let roster_base: String = if roster_exists && !roster_replaced {
                std::fs::read_to_string(&resolved)
                    .map_err(|e| io(format!("cannot read roster {}: {e}", resolved.display())))?
            } else {
                roster_template.to_string()
            };
            Some(Lane {
                path: resolved,
                existed: roster_exists,
                replaced: roster_replaced,
                reshaped: false,
                overwrite: false,
                base: roster_base,
                plan: Plan::default(),
            })
        }
        Shape::Inline => {
            let moved = split::inline_to_pair(&root_base, &shipped_line)
                .map_err(Failure::Refused)?
                .ok_or_else(|| {
                    Failure::Refused(format!(
                        "the file carries `providers` but no line of it can be moved into \
                         {shipped_name} (spec §4.11's shape step); nothing was written"
                    ))
                })?;
            let path = crate::config_load::resolve(&config_dir, &shipped_name);
            let exists = path.is_file();
            // The shape step's own overwrite: a file this run did not
            // write is kept at `<roster>.bak` (D6). A file whose bytes
            // already are the moved block is neither written nor backed
            // up — the pair is already the pair (D10).
            let differs = if exists {
                std::fs::read(&path)
                    .map_err(|e| io(format!("cannot read roster {}: {e}", path.display())))?
                    .as_slice()
                    != moved.roster.as_bytes()
            } else {
                false
            };
            root_base = moved.root.clone();
            split_move = Some((moved, path.clone()));
            Some(Lane {
                path,
                existed: exists,
                replaced: differs,
                reshaped: false,
                overwrite: differs,
                base: split_move.as_ref().unwrap().0.roster.clone(),
                plan: Plan::default(),
            })
        }
        Shape::Refused => None,
    };
    let mut root_lane = Lane {
        path: target.path.clone(),
        existed: target_exists,
        replaced: root_replaced,
        reshaped: split_move.is_some(),
        overwrite: false,
        base: root_base,
        plan: Plan::default(),
    };
    let mut roster_lane = roster_lane;

    // Step 1c — the rule-file lane (ADR-046; DESIGN §12.14 step 1c):
    // built from the **base root** — the file's own bytes under a bare
    // run (so a `--from` template is not the writer's rule-file authority
    // either), and the post-shape-step root under an inline base, so the
    // lane follows the run's candidate rather than the file that is
    // there.
    let rule_lane: Option<Lane> =
        build_rule_lane(&root_lane.base, &config_dir, &selected, args.force)?;

    // No terminal on stdin and no --non-interactive: refuse with the
    // working command in hand (spec §4.11; the `interactive` flag the
    // prompter carries is exactly this disjunction).
    if !prompter.interactive && !args.non_interactive {
        let mut out = String::new();
        out.push_str(&format!(
            "stdin is not a terminal and --non-interactive was not given: nothing \
             was written.\nrun the non-interactive form:\n  vadis setup \
             --non-interactive{}\n",
            args.config
                .as_deref()
                .map(|c| format!(" --config {c}"))
                .unwrap_or_default()
        ));
        for name in missing_env_names(
            &root_lane.base,
            roster_lane.as_ref().map(|l| l.base.as_str()),
        ) {
            out.push_str(&prompt::export_snippet(&name));
            out.push('\n');
        }
        print!("{out}");
        return Ok(EXIT_REFUSED);
    }

    build_plan(
        &mut root_lane,
        roster_lane.as_mut(),
        &selected,
        prompter,
        args,
    )?;

    // Step 7: apply → candidate bytes, per lane. An empty plan's
    // candidate is the base itself (no edits moved it).
    let root_candidate = if root_lane.plan.is_empty() {
        root_lane.base.clone()
    } else {
        edit::apply(&root_lane.base, &root_lane.plan)
    };
    let roster_candidate = roster_lane.as_ref().map(|l| {
        if l.plan.is_empty() {
            l.base.clone()
        } else {
            edit::apply(&l.base, &l.plan)
        }
    });

    // Step 8: the same loader, on the candidate, before anything lands —
    // and before ANY outcome may be reported (R43-F7; spec §4.11's *the
    // loader is the gate* bullet): **every run that reaches a plan reaches
    // this step, including a run whose plan is empty** — there the
    // candidate is the base itself. The plan's emptiness is a statement
    // about the operator's answers, never about the file, so a base the
    // loader refuses is refused here (exit 2, the loader's reason, nothing
    // written) rather than told `no change` — on the write path and on
    // `--dry-run` alike. The candidate is the **pair** (the two files are
    // validated together), and a candidate that does not load is never
    // written — a failure lands neither file, G4 read for two targets.
    // Shape 3 of §4.14's ladder (a `providers_file` whose path is not
    // there) is NOT this step's: the roster lane's base is the embedded
    // roster template then, the pair validates from it, and the run
    // creates the roster — what that arm *is* stays the register's
    // (R46-0-F1); this step decides nothing about it.
    let gate = match (&roster_lane, &roster_candidate) {
        (Some(lane), Some(roster)) => {
            crate::config_load::validate_pair(&root_candidate, roster, &lane.path)
        }
        _ => crate::config_load::validate_text(&root_candidate),
    };
    if let Err(reason) = gate {
        return Err(Failure::Refused(format!(
            "the candidate does not load: {reason}"
        )));
    }

    // `--dry-run`: the plan in place of any landing — checked before the
    // empty-plan branches and before the write itself, so "no write"
    // holds whatever the plan's size and whatever the target's state
    // (spec §4.11's `--dry-run` row; R22-F2: this used to sit below the
    // empty-plan landing, so a fresh target was really written). A base
    // the loader refuses never reaches this branch: step 8 above reports
    // the refusal instead of a plan of nothing (R43-F7).
    if args.dry_run {
        if args.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&report::dry_run_json(
                    &root_lane.plan,
                    roster_lane.as_ref().map(|l| {
                        (
                            l.path.as_path(),
                            if split_move.is_some() {
                                report::MOVED_FROM_INLINE
                            } else {
                                report::NAMED_BY_PROVIDERS_FILE
                            },
                            &l.plan,
                        )
                    }),
                    split_move
                        .as_ref()
                        .map(|(s, p)| report::SplitFacts::of(s, p))
                        .as_ref()
                ))
                .unwrap()
            );
        } else {
            let print_lane = |lane: &Lane, because: &str| {
                let lines: Vec<&str> = lane.base.lines().collect();
                let get = move |i: usize| lines.get(i).copied().map(|s| s.to_string());
                print!("{}", report::dry_run_text(get, &lane.plan));
                // An empty plan is not silence: the run's outcome, stated
                // without performing it — the landing it would perform (a
                // base that is not the file that is there), or the
                // no-change line it would print.
                if lane.plan.is_empty() {
                    if lane.lands() {
                        print!("{}", report::dry_run_would_write_text(&lane.path, because));
                    } else {
                        print!("{}", report::no_change_text(&lane.path));
                    }
                }
            };
            if let Some(roster) = &roster_lane {
                // Two lanes: label each with its file so an edit's target
                // is never ambiguous (spec §4.11's target-file column).
                println!("# {}", root_lane.path.display());
                if let Some((s, p)) = &split_move {
                    print!("{}", report::split_note(&report::SplitFacts::of(s, p)));
                }
                print_lane(
                    &root_lane,
                    &format!("selected by {}", target.selected_by.as_str()),
                );
                println!("# {}", roster.path.display());
                print_lane(
                    roster,
                    if split_move.is_some() {
                        "the root's inline roster, moved"
                    } else {
                        "named by providers_file"
                    },
                );
            } else {
                print_lane(
                    &root_lane,
                    &format!("selected by {}", target.selected_by.as_str()),
                );
            }
            // The rule-file lane's own `--dry-run` line (ADR-046 D9):
            // the file it would create or replace is named, in the same
            // per-lane report shape.
            if let Some(rule) = &rule_lane {
                println!("# {}", rule.path.display());
                print_lane(rule, report::NAMED_BY_RULES_FILE);
            }
        }
        return Ok(EXIT_OK);
    }

    // Step 9: an empty plan over a base that **is** the file that is
    // there — and that step 8 above has just proven LOADS (R43-F7) —
    // writes nothing: this is what makes a second run a no-op rather
    // than a rewrite (G3; spec §4.11: "Nothing to change ⇒ nothing is
    // written — over a target that loads"). Read for the pair: nothing
    // lands only when **neither** lane has a reason to land, and each
    // file of the pair says its own no-change line — a roster that
    // silently did nothing is how "no roster file appeared" reads as a
    // bug (ADR-038 D9).
    if !root_lane.lands()
        && !roster_lane.as_ref().is_some_and(|l| l.lands())
        && !rule_lane.as_ref().is_some_and(|l| l.lands())
    {
        print!("{}", report::no_change_text(&target.path));
        if let Some(roster) = &roster_lane {
            print!("{}", report::no_change_text(&roster.path));
        }
        if let Some(rule) = &rule_lane {
            print!("{}", report::no_change_text(&rule.path));
        }
        return Ok(EXIT_OK);
    }

    // Step 10: `--backup` (or `--force`, which implies it) copies the
    // existing file to `<file>.bak` first — per lane, before either
    // landing, so a backup failure aborts the run with nothing written.
    // The shape step's own overwrite is unconditional: a file this run
    // replaces without having written it is kept whatever the flags say
    // (ADR-038 D6).
    let mut lanes: Vec<&Lane> = match &roster_lane {
        Some(r) => vec![&root_lane, r],
        None => vec![&root_lane],
    };
    if let Some(rule) = &rule_lane {
        lanes.push(rule);
    }
    for lane in &lanes {
        if lane.lands()
            && lane.existed
            && (args.backup || (args.force && lane.replaced) || lane.overwrite)
        {
            let bak = backup_path(&lane.path);
            std::fs::copy(&lane.path, &bak).map_err(|e| {
                io(format!(
                    "cannot back up {} to {}: {e}",
                    lane.path.display(),
                    bak.display()
                ))
            })?;
        }
    }

    // Step 11: land, roster first (an orphan roster is harmless where a
    // root naming a missing roster is not), the rule file second, the
    // root last — the root's `plugins` entry names the rule file, so the
    // name never dangles between landings. Temp file + sync + rename.
    if let Some(roster) = &roster_lane {
        if roster.lands() {
            land(
                &roster.path,
                roster_candidate.as_deref().unwrap().as_bytes(),
            )?;
        }
    }
    if let Some(rule) = &rule_lane {
        if rule.lands() {
            land(&rule.path, rule.base.as_bytes())?;
        }
    }
    if root_lane.lands() {
        land(&target.path, root_candidate.as_bytes())?;
    }
    if args.json {
        let named_by = if split_move.is_some() {
            report::MOVED_FROM_INLINE
        } else {
            report::NAMED_BY_PROVIDERS_FILE
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&report::landed_json(
                &target.path,
                target.selected_by.as_str(),
                root_lane.plan.edits.len(),
                roster_lane.as_ref().filter(|l| l.lands()).map(|l| (
                    l.path.as_path(),
                    named_by,
                    l.plan.edits.len()
                )),
                split_move
                    .as_ref()
                    .map(|(s, p)| report::SplitFacts::of(s, p))
                    .as_ref()
            ))
            .unwrap()
        );
    } else {
        if let Some((s, p)) = &split_move {
            if roster_lane.as_ref().is_some_and(|l| l.lands()) {
                print!("{}", report::split_note(&report::SplitFacts::of(s, p)));
            }
        }
        if let Some(roster) = &roster_lane {
            if roster.lands() {
                print!(
                    "{}",
                    report::landed_roster_text(
                        &roster.path,
                        if split_move.is_some() {
                            report::MOVED_FROM_INLINE
                        } else {
                            report::NAMED_BY_PROVIDERS_FILE
                        },
                        roster.plan.edits.len()
                    )
                );
            }
        }
        if let Some(rule) = &rule_lane {
            if rule.lands() {
                print!(
                    "{}",
                    report::landed_roster_text(&rule.path, report::NAMED_BY_RULES_FILE, 0)
                );
            }
        }
        if root_lane.lands() {
            print!(
                "{}",
                report::landed_text(
                    &target.path,
                    target.selected_by.as_str(),
                    root_lane.plan.edits.len()
                )
            );
        }
    }
    Ok(EXIT_OK)
}

fn backup_path(p: &std::path::Path) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(".bak");
    PathBuf::from(s)
}

/// The named environment variables the pair carries but the ambient
/// environment does not — `--quick`'s "only ask what is missing" and the
/// no-TTY branch's export snippets (spec §4.11). Names only; no value is
/// ever read (the secret boundary). The provider names resolve against
/// the roster's text when the root names one (the target-file column).
fn missing_env_names(root_base: &str, roster_base: Option<&str>) -> Vec<String> {
    let roster_text = roster_base.unwrap_or(root_base);
    let mut out = Vec::new();
    for name in anchor::entry_names(roster_text, "providers", "name") {
        let p = format!("providers[name={name}].api_key_env");
        if let Ok(a) = anchor::resolve_typed(roster_text, &p) {
            if std::env::var_os(&a.value).is_none() {
                out.push(a.value);
            }
        }
    }
    if let Ok(a) = anchor::resolve_typed(root_base, "server.auth_token_env") {
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

/// Steps 4–6: walk the sections' rows against the **base of the file
/// that owns each section** (the section table's target-file column —
/// `providers` resolves against the roster's base under a split root),
/// show each display-only key, ask each askable one, and turn each
/// answer that differs from the value at its anchor into one edit of
/// that lane's plan. A key whose anchor did not resolve is a **refusal**
/// when it carries a requested change and a **warning** when it does not
/// (spec §4.11 step 3).
fn build_plan(
    root_lane: &mut Lane,
    roster_lane: Option<&mut Lane>,
    selected: &[Section],
    prompter: &Prompt,
    args: &SetupArgs,
) -> Result<(), Failure> {
    let split = roster_lane.is_some();
    let mut root_edits: Vec<Edit> = Vec::new();
    let mut roster_edits: Vec<Edit> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let quick_missing: Vec<String> = if args.quick {
        missing_env_names(
            &root_lane.base,
            roster_lane.as_ref().map(|l| l.base.as_str()),
        )
    } else {
        Vec::new()
    };

    for section in selected {
        println!("== {} ==", section.name());
        // The lane this section's keys live in — the mapping is the
        // section table's data, not a special case here.
        let roster_side = section.target(split) == sections::TargetFile::Roster;
        let base: &str = if roster_side {
            &roster_lane
                .as_ref()
                .expect("a roster lane under the split")
                .base
        } else {
            &root_lane.base
        };
        // The fallback default is the **embedded** example of the lane's
        // file — the shipped pair the table is rigged against (DESIGN
        // §12.14), never the `--from` text.
        let template: &str = if roster_side {
            EMBEDDED_ROSTER
        } else {
            EMBEDDED_TEMPLATE
        };
        let edits: &mut Vec<Edit> = if roster_side {
            &mut roster_edits
        } else {
            &mut root_edits
        };
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
                let template_has_it = anchor::resolve_typed(template, row.path).is_ok();
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
                Err(_) => match anchor::resolve_typed(template, row.path) {
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
        eprintln!("vadis: setup: warning: {w}");
    }
    // Disjointness is asserted per file (spec §4.11 step 3's overlap
    // refusal) — two edits of two different files cannot overlap.
    root_lane.plan = Plan::build(root_edits)?;
    match roster_lane {
        Some(l) => l.plan = Plan::build(roster_edits)?,
        None => debug_assert!(roster_edits.is_empty()),
    }
    Ok(())
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

    // The shape the **loader parses** (spec §4.14), read off the same
    // wire type, for the read-only surfaces' roster note (ADR-038 D9):
    // these runs change nothing, so they state what a *writing* run
    // would do with the file — an inline root's roster moves to the
    // file the shipped template names.
    let config_dir = target
        .path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let shape = shape_of(&text, &config_dir);
    let inline: Option<String> = match &shape {
        Shape::Inline => Some(shipped_roster_line().0),
        _ => None,
    };

    if args.check {
        // Load with the same loader `serve` runs — the one entry point
        // that knows the configuration can be a **pair** — then probe
        // every name the file carries (presence only — the secret
        // boundary). A broken pair's refusal is the loader's own message
        // and names the roster's own resolved path (spec §4.14's ladder);
        // the exit codes are unchanged: a config that does not load is
        // exit 2, as today.
        let cfg = crate::config_load::load(&target.path).map_err(Failure::Refused)?;
        let rows = report::check_rows(&cfg.vadis);
        let all_present = rows.iter().all(|(_, s)| *s == report::KeyState::Present);
        if args.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&report::check_json(
                    &target.path,
                    target.selected_by.as_str(),
                    &rows,
                    inline.as_deref()
                ))
                .unwrap()
            );
        } else {
            print!("{}", report::check_text(&rows, inline.as_deref()));
        }
        return Ok(if all_present { EXIT_OK } else { EXIT_CHECK });
    }

    // `--print`: every section's keys with the value the file carries
    // (and the state of a key the template ships commented out). Under a
    // split root the providers section's anchors resolve in the roster's
    // text (spec §4.11's target-file column); a roster that is named but
    // unreadable is stated on the section, and the root still prints.
    let split = matches!(shape, Shape::Split { .. });
    let (roster_text, roster_note): (Option<String>, Option<String>) = match &shape {
        // Neither an inline root nor a refused one has a roster file to
        // read: every section resolves against the root's own text (an
        // inline root's providers block is *in* the root; a refused
        // root prints what it carries, and `--check`'s loader gate is
        // where the refusal itself is stated).
        Shape::Inline | Shape::Refused => (None, None),
        Shape::Split { written, resolved } => {
            if from_template {
                (Some(EMBEDDED_ROSTER.to_string()), None)
            } else {
                match std::fs::read_to_string(resolved) {
                    Ok(t) => (Some(t), None),
                    Err(e) => (
                        None,
                        Some(format!(
                            "providers_file '{written}' (resolved: {}): cannot be read: {e}",
                            resolved.display()
                        )),
                    ),
                }
            }
        }
    };
    let mut rows = Vec::new();
    for section in sections::ALL {
        let section_text: Option<&str> = match section.target(split) {
            sections::TargetFile::Root => Some(&text),
            sections::TargetFile::Roster => roster_text.as_deref(),
        };
        let Some(section_text) = section_text else {
            rows.push(report::PrintRow {
                path: "providers".to_string(),
                value: format!("({})", roster_note.clone().unwrap_or_default()),
                enabled: false,
                note: "",
                from_template,
            });
            continue;
        };
        for (path, note) in sections::show_entries(section, section_text) {
            rows.push(report::PrintRow {
                path,
                value: note,
                enabled: true,
                note: "",
                from_template,
            });
        }
        for row in sections::rows_for(section, section_text) {
            match anchor::resolve_typed(section_text, row.path) {
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
                target.selected_by.as_str(),
                inline.as_deref()
            ))
            .unwrap()
        );
    } else {
        print!(
            "{}",
            report::print_text(&rows, &target.path, from_template, inline.as_deref())
        );
    }
    Ok(EXIT_OK)
}

/// Landing (step 11): candidate to `<path>.setup.tmp` **in the file's
/// directory**, `sync_all`, `rename` over the target; the temporary file
/// is removed on any failure, so nothing lands partially and no other
/// process can observe a half-written config. The file's directory is
/// created when missing (`mkdir -p`): a file this run creates is `0600`
/// — exact under any umask, `OpenOptions::mode` can only have bits
/// cleared by one — and a directory this run creates is `0700` (set
/// explicitly after `create_dir_all`, which cannot express a mode).
/// Nothing that already exists is re-moded. The roster lane lands through
/// the same function: one landing rule for both files of the pair.
///
/// **A target that is already there keeps the mode it had** (R43-F6;
/// spec §4.11's *Landing* bullet, G8; DESIGN §12.14's mode paragraph):
/// `0600` is the temporary file's **creation** mode, never the landed
/// file's. The replacement carries the target's own mode — set on the
/// temporary file BEFORE the `rename`, so there is no window in which
/// the target's name carries the wrong mode. A `rename` needs the
/// target's directory to be writable, never the file, so a read-only
/// (`0444`) target is written rather than refused and is read-only again
/// when the run returns: no run of this command changes a mode the
/// operator set. What the rule covers is the **mode** — the replace is a
/// new inode, so ownership follows the user who ran the command and a
/// hard link, an ACL or an xattr on the old inode does not travel.
fn land(path: &std::path::Path, bytes: &[u8]) -> Result<(), Failure> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| Failure::Refused(format!("target {} has no directory", path.display())))?;
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
    // The mode the landing must keep (R43-F6): the target's own, read
    // before the temporary file is created. `None` = the target is not
    // there, and the temporary file's `0600` creation mode below IS the
    // landed file's — the fresh-file half of the rule.
    #[cfg(unix)]
    let kept_mode: Option<std::fs::Permissions> = match std::fs::metadata(path) {
        Ok(m) => {
            use std::os::unix::fs::PermissionsExt as _;
            Some(std::fs::Permissions::from_mode(
                m.permissions().mode() & 0o777,
            ))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err(io(format!("cannot stat {}: {e}", path.display())));
        }
    };
    let tmp = path.with_extension("setup.tmp");
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
    // The kept mode rides the temporary file across the `rename` (the
    // preferred half of DESIGN §12.14's rule: no window in which the
    // target's name carries the wrong mode).
    #[cfg(unix)]
    if let Some(mode) = kept_mode {
        if let Err(e) = std::fs::set_permissions(&tmp, mode) {
            let _ = std::fs::remove_file(&tmp);
            return Err(io(format!(
                "cannot keep the mode of {} on {}: {e}",
                path.display(),
                tmp.display()
            )));
        }
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(io(format!(
            "cannot move {} over {}: {e}",
            tmp.display(),
            path.display()
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
            "vadis-setup-{name}-{}-{}",
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

    fn scripted_owned(answers: Vec<String>) -> Prompt {
        let p = Prompt::new(true);
        p.load_answers(answers);
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
    /// **pair**'s bytes are identical to the embedded templates', and it
    /// loads through the same loader `serve` runs. The shipped template is
    /// split (spec §4.14), so a fresh run writes two files: the root and
    /// the roster it names.
    #[test]
    fn g1_defaults_write_the_template_verbatim() {
        let dir = temp_root("g1");
        let target = dir.join("config.yaml");
        assert_eq!(run_with_prompt(&args(&target), &non_interactive()), 0);
        let written = std::fs::read(&target).unwrap();
        assert_eq!(
            written,
            EMBEDDED_TEMPLATE.as_bytes(),
            "G1: the fresh all-defaults root must be byte-identical to the template"
        );
        let roster = dir.join("providers.example.yaml");
        assert_eq!(
            std::fs::read(&roster).unwrap(),
            EMBEDDED_ROSTER.as_bytes(),
            "G1: the fresh all-defaults roster must be byte-identical to the roster template"
        );
        // The written pair loads through the same loader `serve` runs —
        // the pair entry point, not a text-only gate.
        assert!(crate::config_load::load(&target).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The embedded pair itself: the root template names the roster
    /// template's shipped file name, and the two validate together — the
    /// two `include_str!`s cannot drift apart.
    #[test]
    fn the_embedded_pair_is_a_pair() {
        let dir = temp_root("embedded-pair");
        match shape_of(EMBEDDED_TEMPLATE, &dir) {
            Shape::Split { written, resolved } => {
                assert_eq!(written, "providers.example.yaml");
                assert_eq!(resolved, dir.join("providers.example.yaml"));
            }
            Shape::Inline | Shape::Refused => {
                panic!("the shipped template is split (spec §4.14)")
            }
        }
        crate::config_load::validate_pair(
            EMBEDDED_TEMPLATE,
            EMBEDDED_ROSTER,
            &dir.join("providers.example.yaml"),
        )
        .expect("the embedded pair validates");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// G3 / CONF-69: the same answers twice ⇒ the second run writes
    /// nothing (bytes **and** mtime unchanged) — of **both** files of
    /// the pair.
    #[test]
    fn g3_second_run_is_a_no_op() {
        let dir = temp_root("g3");
        let target = dir.join("config.yaml");
        run_with_prompt(&args(&target), &non_interactive());
        let roster = dir.join("providers.example.yaml");
        let (b1, m1) = (
            std::fs::read(&target).unwrap(),
            std::fs::metadata(&target).unwrap().modified().unwrap(),
        );
        let (r1, rm1) = (
            std::fs::read(&roster).unwrap(),
            std::fs::metadata(&roster).unwrap().modified().unwrap(),
        );
        assert_eq!(run_with_prompt(&args(&target), &non_interactive()), 0);
        let (b2, m2) = (
            std::fs::read(&target).unwrap(),
            std::fs::metadata(&target).unwrap().modified().unwrap(),
        );
        let (r2, rm2) = (
            std::fs::read(&roster).unwrap(),
            std::fs::metadata(&roster).unwrap().modified().unwrap(),
        );
        assert_eq!(b1, b2, "root bytes unchanged");
        assert_eq!(m1, m2, "root mtime unchanged (nothing was written)");
        assert_eq!(r1, r2, "roster bytes unchanged");
        assert_eq!(rm1, rm2, "roster mtime unchanged (nothing was written)");
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
        // The roster file is not the server section's target: untouched.
        let roster = dir.join("providers.example.yaml");
        assert_eq!(
            std::fs::read(&roster).unwrap(),
            EMBEDDED_ROSTER.as_bytes(),
            "a server answer never moves the roster's bytes"
        );
        // The written pair loads through the same loader `serve` runs.
        assert!(crate::config_load::load(&target).is_ok());
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
            // G4 read for two targets: the roster half of the pair does
            // not move either (the pair is validated before either file
            // is written) — the first run's roster is still there,
            // byte-identical, and no roster temp file is left behind.
            assert_eq!(
                std::fs::read(dir.join("providers.example.yaml")).unwrap(),
                EMBEDDED_ROSTER.as_bytes(),
                "(d) the roster lane does not land on a refused pair"
            );
            assert!(
                !dir.join("providers.example.setup.tmp").exists(),
                "(d) no roster temporary file is left behind"
            );
        }

        // The untouched-target half: whatever happened above, the original
        // pair never moved.
        assert_eq!(before, std::fs::read(&target).unwrap());
        assert_eq!(
            before_mtime,
            std::fs::metadata(&target).unwrap().modified().unwrap()
        );
        assert_eq!(
            std::fs::read(dir.join("providers.example.yaml")).unwrap(),
            EMBEDDED_ROSTER.as_bytes(),
            "the refusal ladder leaves the roster byte-identical too"
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
        // The config is the pair: the root names its roster (spec §4.14).
        std::fs::write(&target, EMBEDDED_TEMPLATE).unwrap();
        std::fs::write(dir.join("providers.example.yaml"), EMBEDDED_ROSTER).unwrap();
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

    /// §4.14's ladder at the `--check` surface (the pair validated as a
    /// pair): a missing roster, a malformed roster and a root writing both
    /// shapes are each refused with the **same exit code** a bad root has
    /// today (2), and the loader's message names the roster's own path —
    /// the message contents are asserted in `config_load`'s tests; here
    /// the codes are the contract (spec §4.11's table).
    #[test]
    fn check_refuses_a_broken_pair_like_a_bad_root() {
        let dir = temp_root("check-pair");
        let check = |root: &std::path::Path| {
            run_with_prompt(
                &SetupArgs {
                    check: true,
                    config: Some(root.display().to_string()),
                    ..Default::default()
                },
                &non_interactive(),
            )
        };
        // The control: a good pair exits 0 or 4 (environment-dependent).
        let root = dir.join("config.yaml");
        std::fs::write(&root, EMBEDDED_TEMPLATE).unwrap();
        std::fs::write(dir.join("providers.example.yaml"), EMBEDDED_ROSTER).unwrap();
        assert!(check(&root) == 0 || check(&root) == 4);

        // (i) the named roster is missing.
        let _ = std::fs::remove_file(dir.join("providers.example.yaml"));
        assert_eq!(check(&root), 2, "a missing roster refuses");

        // (ii) the roster is not the roster block (spec §4.14 shape 4).
        std::fs::write(dir.join("providers.example.yaml"), "server: {}\n").unwrap();
        assert_eq!(check(&root), 2, "a malformed roster refuses");

        // (iii) the root writes both shapes (spec §4.14 shape 1).
        let both = dir.join("both.yaml");
        std::fs::write(&both, format!("{EMBEDDED_TEMPLATE}providers: []\n")).unwrap();
        std::fs::write(dir.join("providers.example.yaml"), EMBEDDED_ROSTER).unwrap();
        assert_eq!(check(&both), 2, "both shapes written refuses");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R43-F7 / spec §4.11's *the loader is the gate* bullet: an empty
    /// plan does not skip the candidate gate — when nothing was planned
    /// the candidate **is** the base, and a base the loader refuses is
    /// refused by this command too (exit 2, the loader's own message,
    /// nothing written), on the write path **and** on `--dry-run`. The
    /// shipped site let the empty plan sweep past step 8 and printed
    /// `no change`, exit 0, over a pair `--check` refuses.
    #[test]
    fn an_unloadable_pair_is_refused_even_with_an_empty_plan() {
        let dir = temp_root("f7-gate");

        // Shape 4 (spec §4.14's ladder): a roster that is present but is
        // not the roster block. The root itself is the shipped split
        // template — valid — and the plan is empty (zero answers).
        let target = dir.join("config.yaml");
        std::fs::write(&target, EMBEDDED_TEMPLATE).unwrap();
        let roster = dir.join("providers.example.yaml");
        let bad_roster = "server: {}\n";
        std::fs::write(&roster, bad_roster).unwrap();
        let root_mtime = std::fs::metadata(&target).unwrap().modified().unwrap();
        let roster_mtime = std::fs::metadata(&roster).unwrap().modified().unwrap();

        let reason = match run_inner(&args(&target), &non_interactive()) {
            Err(Failure::Refused(reason)) => reason,
            Err(Failure::Io(reason)) => panic!("an I/O failure is not this refusal: {reason}"),
            Ok(code) => {
                panic!("a present-but-unloadable roster must refuse; the run exited {code}")
            }
        };
        assert!(
            reason.contains("does not load"),
            "the refusal is the candidate gate's, got: {reason}"
        );
        assert!(
            reason.contains("providers.example.yaml"),
            "the loader's own message names the roster's own path, got: {reason}"
        );
        // Nothing written, to either file: no bytes, no mtimes, no
        // backups, no temporary files (G4 read for two targets).
        assert_eq!(
            std::fs::read(&target).unwrap(),
            EMBEDDED_TEMPLATE.as_bytes()
        );
        assert_eq!(std::fs::read(&roster).unwrap(), bad_roster.as_bytes());
        assert_eq!(
            root_mtime,
            std::fs::metadata(&target).unwrap().modified().unwrap()
        );
        assert_eq!(
            roster_mtime,
            std::fs::metadata(&roster).unwrap().modified().unwrap()
        );
        assert!(!dir.join("config.yaml.bak").exists());
        assert!(!dir.join("providers.example.yaml.bak").exists());
        assert!(!dir.join("config.yaml.setup.tmp").exists());
        assert!(!dir.join("providers.example.setup.tmp").exists());

        // `--dry-run` reports the refusal rather than printing a plan of
        // nothing (the same bullet's last sentence).
        let code = run_with_prompt(
            &SetupArgs {
                dry_run: true,
                ..args(&target)
            },
            &non_interactive(),
        );
        assert_eq!(code, 2, "--dry-run reports the refusal");
        assert_eq!(std::fs::read(&roster).unwrap(), bad_roster.as_bytes());

        // The other shapes the gate covers, over the same empty plan:
        // shape 1 (both keys written), shape 2 (neither written), and the
        // pre-ladder case (a root that does not parse at all) — each
        // refused at exit 2, nothing written.
        let both = dir.join("both.yaml");
        let both_text = format!("{EMBEDDED_TEMPLATE}providers: []\n");
        std::fs::write(&both, &both_text).unwrap();
        let reason = match run_inner(&args(&both), &non_interactive()) {
            Err(Failure::Refused(reason)) => reason,
            Err(Failure::Io(reason)) => panic!("an I/O failure is not this refusal: {reason}"),
            Ok(code) => panic!("shape 1 (both keys) must refuse; the run exited {code}"),
        };
        assert!(
            reason.contains("both"),
            "the loader's reason, got: {reason}"
        );
        assert_eq!(std::fs::read(&both).unwrap(), both_text.as_bytes());

        let line = EMBEDDED_TEMPLATE
            .lines()
            .find(|l| l.starts_with("providers_file:"))
            .unwrap();
        let neither = dir.join("neither.yaml");
        let neither_text = EMBEDDED_TEMPLATE.replace(&format!("{line}\n"), "");
        std::fs::write(&neither, &neither_text).unwrap();
        let code = run_with_prompt(&args(&neither), &non_interactive());
        assert_eq!(code, 2, "shape 2 (neither key) refuses on an empty plan");
        assert_eq!(std::fs::read(&neither).unwrap(), neither_text.as_bytes());

        let broken = dir.join("broken.yaml");
        let broken_text = "server: [1]\n";
        std::fs::write(&broken, broken_text).unwrap();
        let code = run_with_prompt(&args(&broken), &non_interactive());
        assert_eq!(code, 2, "an unparsable root refuses on an empty plan");
        assert_eq!(std::fs::read(&broken).unwrap(), broken_text.as_bytes());

        // The control: over a pair that LOADS, the same empty plan is
        // exactly the no-op the old code reported over the broken pair —
        // the fix moves the gate, not the no-op.
        let good = dir.join("good/config.yaml");
        std::fs::create_dir_all(good.parent().unwrap()).unwrap();
        assert_eq!(run_with_prompt(&args(&good), &non_interactive()), 0);
        let (b1, m1) = (
            std::fs::read(&good).unwrap(),
            std::fs::metadata(&good).unwrap().modified().unwrap(),
        );
        assert_eq!(
            run_with_prompt(&args(&good), &non_interactive()),
            0,
            "a no-op run is a no-op over a config that works"
        );
        assert_eq!(b1, std::fs::read(&good).unwrap());
        assert_eq!(m1, std::fs::metadata(&good).unwrap().modified().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R43-F6 / spec §4.11's *Landing* bullet + G8's file half: **a target
    /// that is already there keeps the mode it had** across the landing —
    /// the temporary file's `0600` is a *creation* mode, never the landed
    /// file's. A read-only (`0444`) pair stays `0444` and is written all
    /// the same (the `rename` needs the target's **directory** to be
    /// writable, never the file), a `0644` target stays `0644` across an
    /// edit landing — on the root lane and the roster lane alike, so no
    /// run of this command changes a mode the operator set — while a file
    /// the run **creates** is `0600` (the creation half, unchanged). The
    /// `.bak` copy keeps the mode of the file it copied.
    #[test]
    #[cfg(unix)]
    fn a_landing_keeps_the_existing_targets_mode() {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let dir = temp_root("f6-mode");

        // (a) the read-only pair, stat-verified before the run.
        let target = dir.join("config.yaml");
        std::fs::write(&target, EMBEDDED_TEMPLATE).unwrap();
        let roster = dir.join("providers.example.yaml");
        std::fs::write(&roster, EMBEDDED_ROSTER).unwrap();
        for p in [&target, &roster] {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o444)).unwrap();
        }
        assert_eq!(mode(&target), 0o444, "stat: the root is read-only");
        assert_eq!(mode(&roster), 0o444, "stat: the roster is read-only");
        // `--force` lands BOTH lanes (byte-identical to the templates —
        // the finding's own shape: "the hash did not move and something
        // did").
        let code = run_with_prompt(
            &SetupArgs {
                force: true,
                ..args(&target)
            },
            &non_interactive(),
        );
        assert_eq!(code, 0, "a read-only target is written, not refused");
        assert_eq!(
            mode(&target),
            0o444,
            "the root lane kept the operator's mode across the landing"
        );
        assert_eq!(
            mode(&roster),
            0o444,
            "the roster lane kept the operator's mode across the landing"
        );
        assert_eq!(
            std::fs::read(&target).unwrap(),
            EMBEDDED_TEMPLATE.as_bytes()
        );
        assert_eq!(std::fs::read(&roster).unwrap(), EMBEDDED_ROSTER.as_bytes());
        // The `.bak` copies keep the mode of the files they copied.
        assert_eq!(mode(&dir.join("config.yaml.bak")), 0o444);
        assert_eq!(mode(&dir.join("providers.example.yaml.bak")), 0o444);
        // And the run remains re-runnable over the read-only pair: a
        // second run is a no-op (G3's control in the finding's transcript).
        assert_eq!(run_with_prompt(&args(&target), &non_interactive()), 0);
        assert_eq!(mode(&target), 0o444);
        assert_eq!(mode(&roster), 0o444);

        // (b) the ordinary case: a `0644` pair, one answered edit — the
        // landing lane keeps `0644`, the un-landed lane is untouched.
        for p in [&target, &roster] {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        let code = run_with_prompt(
            &SetupArgs {
                section: Some("server".to_string()),
                ..args(&target)
            },
            &scripted(&["127.0.0.1:9911", "", ""]),
        );
        assert_eq!(code, 0);
        assert_eq!(mode(&target), 0o644, "0644 stays 0644 on the root lane");
        assert_eq!(mode(&roster), 0o644, "the roster lane did not land");
        assert!(std::fs::read_to_string(&target)
            .unwrap()
            .contains("127.0.0.1:9911"));

        // (c) the creation half is unchanged: a file the run creates is
        // `0600`, whatever the umask.
        let fresh = dir.join("fresh/config.yaml");
        assert_eq!(run_with_prompt(&args(&fresh), &non_interactive()), 0);
        assert_eq!(mode(&fresh), 0o600, "a created file is 0600");
        assert_eq!(mode(&fresh.with_file_name("providers.example.yaml")), 0o600);
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
            // The roster half of the pair is a file this run creates too.
            let roster = dir.join("nested/deeper/providers.example.yaml");
            let roster_mode = std::fs::metadata(&roster).unwrap().permissions().mode();
            assert_eq!(roster_mode & 0o777, 0o600, "created roster is 0600");
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
        // The server section does not target the roster: under `--force`
        // the roster is neither replaced nor backed up.
        let roster = dir.join("providers.example.yaml");
        assert_eq!(std::fs::read(&roster).unwrap(), EMBEDDED_ROSTER.as_bytes());
        assert!(
            !dir.join("providers.example.yaml.bak").exists(),
            "a lane the run does not replace is not backed up"
        );
        assert!(crate::config_load::load(&target).is_ok());
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
        // The server section never touches the roster half of the pair.
        assert_eq!(
            std::fs::read(dir.join("providers.example.yaml")).unwrap(),
            EMBEDDED_ROSTER.as_bytes()
        );
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
                !dir.join("providers.example.yaml").exists(),
                "(a) --dry-run: the fresh roster is not created either"
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
            assert!(
                !dir.join("providers.example.yaml").exists(),
                "(b) --dry-run with edits: the roster lane does not land either"
            );
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
            assert!(
                !dir.join("providers.example.yaml.bak").exists(),
                "(c) --dry-run: no roster backup either"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R22-F1 (a): `--force` is consulted on the empty-plan path — over
    /// an existing target it replaces the file with the template (0
    /// edits) and keeps the previous bytes at `<target>.bak` (`--force`
    /// implies `--backup`; spec §4.11's `--force` and `--backup` rows).
    #[test]
    fn r22_f1_force_replaces_on_the_empty_plan_path() {
        let dir = temp_root("f1force");
        let target = dir.join("config.yaml");
        run_with_prompt(&args(&target), &non_interactive());
        // A hand note `--force` is defined to discard (spec §4.11: "What
        // it discards is any note **you** wrote into your own file").
        let hand = EMBEDDED_TEMPLATE.replace(
            "  request_timeout: 10m",
            "  request_timeout: 11m          # my own note",
        );
        std::fs::write(&target, &hand).unwrap();
        let code = run_with_prompt(
            &SetupArgs {
                force: true,
                non_interactive: true,
                config: Some(target.display().to_string()),
                ..Default::default()
            },
            &non_interactive(),
        );
        assert_eq!(code, 0);
        assert_eq!(
            std::fs::read(&target).unwrap(),
            EMBEDDED_TEMPLATE.as_bytes(),
            "the empty-plan --force run replaced the target with the template"
        );
        assert_eq!(
            std::fs::read(dir.join("config.yaml.bak")).unwrap(),
            hand.as_bytes(),
            "--force implies --backup: the replaced file survives at <target>.bak"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R22-F1 (b): the loader gate runs on the empty-plan path too —
    /// `--force --from <a template that does not load>` over an existing
    /// target refuses (exit 2), replaces nothing, and writes no backup
    /// (spec §4.11: "a candidate that does not load" ⇒ exit 2, nothing
    /// written).
    #[test]
    fn r22_f1_loader_gate_runs_on_the_empty_plan_path() {
        let dir = temp_root("f1loader");
        let target = dir.join("config.yaml");
        run_with_prompt(&args(&target), &non_interactive());
        let (before, before_mtime) = (
            std::fs::read(&target).unwrap(),
            std::fs::metadata(&target).unwrap().modified().unwrap(),
        );
        // A template that does not load: an unknown key under
        // `deny_unknown_fields`.
        let mangled = EMBEDDED_TEMPLATE.replace("server:\n", "server:\n  no_such_key: 1\n");
        let from = dir.join("mangled.yaml");
        std::fs::write(&from, &mangled).unwrap();
        let code = run_with_prompt(
            &SetupArgs {
                force: true,
                non_interactive: true,
                from: Some(from.display().to_string()),
                config: Some(target.display().to_string()),
                ..Default::default()
            },
            &non_interactive(),
        );
        assert_eq!(
            code, 2,
            "--force --from <unloadable> over an existing target refuses"
        );
        assert_eq!(before, std::fs::read(&target).unwrap(), "bytes unchanged");
        assert_eq!(
            before_mtime,
            std::fs::metadata(&target).unwrap().modified().unwrap(),
            "mtime unchanged"
        );
        assert!(
            !dir.join("config.yaml.bak").exists(),
            "nothing was written, so nothing was backed up either"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R22-F1 (c): the empty plan over an existing target **without**
    /// `--force` keeps its no-op behavior — the fix moved the branch,
    /// this pins that it did not move away. Basis: spec §4.11's `--force`
    /// row ("the base becomes the template **instead of the file that is
    /// there**") — without the flag the file that is there is the base,
    /// and an empty plan over it has nothing to land ("Nothing to change
    /// ⇒ nothing is written").
    #[test]
    fn r22_f1_empty_plan_existing_target_without_force_still_no_op() {
        let dir = temp_root("f1noop");
        let target = dir.join("config.yaml");
        run_with_prompt(&args(&target), &non_interactive());
        let (before, m) = (
            std::fs::read(&target).unwrap(),
            std::fs::metadata(&target).unwrap().modified().unwrap(),
        );
        assert_eq!(run_with_prompt(&args(&target), &non_interactive()), 0);
        assert_eq!(before, std::fs::read(&target).unwrap(), "bytes unchanged");
        assert_eq!(
            m,
            std::fs::metadata(&target).unwrap().modified().unwrap(),
            "mtime unchanged"
        );
        assert!(
            !dir.join("config.yaml.bak").exists(),
            "no backup: no write was attempted"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------
    // The pair (spec §4.11's target-file column; §4.14): the providers
    // section's anchored edit lands in the file that owns the key, and
    // `--from <roster> --force` replaces the roster as a unit.
    // -----------------------------------------------------------------

    /// The providers section's rows, as the wizard walks them: one
    /// `api_key_env` question per roster entry, in file order.
    fn providers_answers(first_answer: &str) -> Vec<String> {
        let n = anchor::entry_names(EMBEDDED_ROSTER, "providers", "name").len();
        let mut v = vec![first_answer.to_string()];
        v.extend(std::iter::repeat(String::new()).take(n - 1));
        v
    }

    /// (b) split shape: the `providers` section's anchored edit lands in
    /// the **roster** — the root does not move one byte, and only the
    /// answered line of the roster moves (G2 read for the roster lane).
    #[test]
    fn providers_edit_lands_in_the_roster_under_a_split_root() {
        let dir = temp_root("edit-split");
        let target = dir.join("config.yaml");
        run_with_prompt(&args(&target), &non_interactive());
        let roster = dir.join("providers.example.yaml");
        let (root_before, root_mtime) = (
            std::fs::read(&target).unwrap(),
            std::fs::metadata(&target).unwrap().modified().unwrap(),
        );
        let code = run_with_prompt(
            &SetupArgs {
                section: Some("providers".to_string()),
                config: Some(target.display().to_string()),
                ..Default::default()
            },
            &scripted_owned(providers_answers("DEEPSEEK_API_KEY_2")),
        );
        assert_eq!(code, 0);
        // The root: byte- and mtime-identical — the edit is not its.
        assert_eq!(root_before, std::fs::read(&target).unwrap());
        assert_eq!(
            root_mtime,
            std::fs::metadata(&target).unwrap().modified().unwrap()
        );
        // The roster: exactly the answered key's own line moved.
        let a =
            anchor::resolve_typed(EMBEDDED_ROSTER, "providers[name=deepseek].api_key_env").unwrap();
        let base_lines: Vec<&str> = EMBEDDED_ROSTER.lines().collect();
        let after = std::fs::read_to_string(&roster).unwrap();
        let after_lines: Vec<&str> = after.lines().collect();
        assert_eq!(base_lines.len(), after_lines.len());
        let moved: Vec<usize> = base_lines
            .iter()
            .zip(after_lines.iter())
            .enumerate()
            .filter(|(_, (b, c))| b != c)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(moved, vec![a.line], "only the answered line may move");
        assert!(after_lines[a.line].contains("DEEPSEEK_API_KEY_2"));
        assert_eq!(
            provenance_counts(EMBEDDED_ROSTER),
            provenance_counts(&after)
        );
        // The pair still loads through the same loader `serve` runs.
        assert!(crate::config_load::load(&target).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An inline root built from the shipped pair's own bytes: the roster
    /// block spliced back in where the root's `providers_file:` line
    /// stands — the pre-split shape, the bug report's file. Splitting it
    /// reproduces the shipped root byte for byte (split.rs's round trip),
    /// which is what makes the assertions below relations rather than
    /// snapshots.
    fn inline_fixture() -> String {
        let line = EMBEDDED_TEMPLATE
            .lines()
            .find(|l| l.starts_with("providers_file:"))
            .expect("the shipped root names its roster");
        let inline = EMBEDDED_TEMPLATE.replace(
            &format!("{line}\n"),
            &format!("{}\n", EMBEDDED_ROSTER.trim_end()),
        );
        assert!(
            inline.contains("providers:\n")
                && !inline.lines().any(|l| l.starts_with("providers_file:")),
            "the fixture is the inline form"
        );
        inline
    }

    /// (b) inline shape, **flipped** (ADR-038 — the old assertion that an
    /// inline root is preserved *was* the bug): a writing run over an
    /// inline root moves the block into the file the root then names. The
    /// roster file's bytes are exactly the moved block, the root keeps
    /// every other byte with the shipped `providers_file:` line in the
    /// header line's place, and the pair loads through the same loader
    /// `serve` runs.
    #[test]
    fn a_writing_run_never_leaves_the_roster_inline() {
        let dir = temp_root("shape-step");
        let target = dir.join("config.yaml");
        let inline = inline_fixture();
        std::fs::write(&target, &inline).unwrap();
        let code = run_with_prompt(&args(&target), &non_interactive());
        assert_eq!(code, 0);
        // The roster file is the moved block, byte for byte — entries,
        // comments and every source citation included.
        let roster = dir.join("providers.example.yaml");
        let moved = format!("{}\n", EMBEDDED_ROSTER.trim_end());
        assert_eq!(
            std::fs::read(&roster).unwrap(),
            moved.as_bytes(),
            "the roster file's bytes are exactly the moved block"
        );
        // The root is the shipped root byte for byte: every other byte
        // kept, the shipped line where the header was.
        assert_eq!(
            std::fs::read(&target).unwrap(),
            EMBEDDED_TEMPLATE.as_bytes(),
            "the shape step's root is the shipped root (the fixture is the splice)"
        );
        // The byte boundary (AGENTS constraint 1 read for the shape
        // step): old root vs new root differ by the removed span plus
        // the one written line — and nothing else.
        let (shipped_name, shipped_line) = shipped_roster_line();
        assert_eq!(shipped_name, "providers.example.yaml");
        assert_eq!(
            EMBEDDED_TEMPLATE.len() + moved.len(),
            inline.len() + shipped_line.len() + 1,
            "the pair's bytes are the inline root's minus the span plus the one line"
        );
        assert!(crate::config_load::load(&target).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Idempotence, read for the shape step: a second run over the
    /// now-split pair writes nothing — no bytes, no mtimes, no backups —
    /// and each file says its own `no change:` line (asserted on the
    /// binary in the round's behavioral probe; stdout is not capturable
    /// in-process).
    #[test]
    fn a_second_run_over_the_split_pair_is_a_no_op() {
        let dir = temp_root("shape-idem");
        let target = dir.join("config.yaml");
        std::fs::write(&target, inline_fixture()).unwrap();
        assert_eq!(run_with_prompt(&args(&target), &non_interactive()), 0);
        let roster = dir.join("providers.example.yaml");
        let before: Vec<_> = [&target, &roster]
            .iter()
            .map(|p| {
                (
                    std::fs::read(p).unwrap(),
                    std::fs::metadata(p).unwrap().modified().unwrap(),
                )
            })
            .collect();
        assert_eq!(run_with_prompt(&args(&target), &non_interactive()), 0);
        for (p, (bytes, mtime)) in [&target, &roster].iter().zip(before.iter()) {
            assert_eq!(&std::fs::read(p).unwrap(), bytes, "{} moved", p.display());
            assert_eq!(
                &std::fs::metadata(p).unwrap().modified().unwrap(),
                mtime,
                "{} was rewritten",
                p.display()
            );
        }
        assert!(
            !dir.join("providers.example.yaml.bak").exists(),
            "a no-op run takes no backup"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The shape step's own overwrite (ADR-038 D6/D10): an existing
    /// roster whose bytes differ from the moved block is kept at
    /// `<roster>.bak` **without** `--backup`; a roster whose bytes
    /// already are the moved block is neither written nor backed up.
    #[test]
    fn the_shape_steps_overwrite_backs_up_only_a_differing_roster() {
        let dir = temp_root("shape-overwrite");
        let moved = format!("{}\n", EMBEDDED_ROSTER.trim_end());

        // (a) differing bytes: the operator's file survives at .bak,
        // whatever the flags say.
        let target = dir.join("a/config.yaml");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, inline_fixture()).unwrap();
        let roster = target.parent().unwrap().join("providers.example.yaml");
        let foreign = b"# my own roster, not the moved block\n".as_slice();
        std::fs::write(&roster, foreign).unwrap();
        let code = run_with_prompt(&args(&target), &non_interactive());
        assert_eq!(code, 0);
        assert_eq!(
            std::fs::read(target.parent().unwrap().join("providers.example.yaml.bak")).unwrap(),
            foreign,
            "D6: a differing roster is kept at <roster>.bak without --backup"
        );
        assert_eq!(std::fs::read(&roster).unwrap(), moved.as_bytes());

        // (b) equal bytes: the pair is already the pair — the roster is
        // neither written nor backed up, and its mtime does not move.
        let target = dir.join("b/config.yaml");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, inline_fixture()).unwrap();
        let roster = target.parent().unwrap().join("providers.example.yaml");
        std::fs::write(&roster, &moved).unwrap();
        let mtime = std::fs::metadata(&roster).unwrap().modified().unwrap();
        let code = run_with_prompt(&args(&target), &non_interactive());
        assert_eq!(code, 0);
        assert_eq!(std::fs::read(&roster).unwrap(), moved.as_bytes());
        assert_eq!(
            std::fs::metadata(&roster).unwrap().modified().unwrap(),
            mtime,
            "D10: an already-equal roster is not rewritten"
        );
        assert!(
            !target
                .parent()
                .unwrap()
                .join("providers.example.yaml.bak")
                .exists(),
            "D10: an already-equal roster is not backed up either"
        );
        // The root half of (b) still reshaped and loads.
        assert_eq!(
            std::fs::read(&target).unwrap(),
            EMBEDDED_TEMPLATE.as_bytes()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The read-only surfaces over an inline root (ADR-038 D9): `--print`
    /// and `--check` change nothing — no roster file appears, the root's
    /// bytes and mtime are untouched — and the run still works: print is
    /// exit 0, check exits 0/4 exactly as over any loadable file. (That
    /// the output *states* the inline fact is the binary probe's
    /// assertion; the rendering itself is `report::inline_roster_text`.)
    #[test]
    fn print_and_check_over_an_inline_root_move_nothing() {
        let dir = temp_root("shape-readonly");
        let target = dir.join("config.yaml");
        let inline = inline_fixture();
        std::fs::write(&target, &inline).unwrap();
        let mtime = std::fs::metadata(&target).unwrap().modified().unwrap();
        let code = run_with_prompt(
            &SetupArgs {
                print: true,
                config: Some(target.display().to_string()),
                ..Default::default()
            },
            &non_interactive(),
        );
        assert_eq!(code, 0, "--print over an inline root prints, exit 0");
        let code = run_with_prompt(
            &SetupArgs {
                check: true,
                config: Some(target.display().to_string()),
                ..Default::default()
            },
            &non_interactive(),
        );
        assert!(
            code == 0 || code == 4,
            "--check over a loadable inline root exits 0 or 4 (got {code})"
        );
        assert_eq!(std::fs::read(&target).unwrap(), inline.as_bytes());
        assert_eq!(
            std::fs::metadata(&target).unwrap().modified().unwrap(),
            mtime
        );
        assert!(
            !dir.join("providers.example.yaml").exists(),
            "a read-only run moves nothing"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The refusal ladder stands (ADR-038 D5): a root that writes both
    /// keys, or neither, is refused through the **loader** — the same
    /// exit code and the same nothing-written as before the shape step;
    /// the wizard does not repair what does not load. The run carries one
    /// answered change so the candidate gate fires on the *edited*
    /// candidate; that the same refusal fires with an EMPTY plan (the
    /// base itself as the candidate) is R43-F7's test above.
    #[test]
    fn a_root_writing_both_keys_or_neither_is_refused_through_the_loader() {
        let dir = temp_root("shape-refused");
        let one_change = |config: &std::path::Path| {
            run_with_prompt(
                &SetupArgs {
                    section: Some("server".to_string()),
                    config: Some(config.display().to_string()),
                    ..Default::default()
                },
                &scripted(&["127.0.0.1:9911", "", ""]),
            )
        };
        // Both keys: the shipped root plus an inline `providers:` line.
        let both = dir.join("both.yaml");
        let both_text = format!("{EMBEDDED_TEMPLATE}providers: []\n");
        std::fs::write(&both, &both_text).unwrap();
        let code = one_change(&both);
        assert_eq!(code, 2, "both keys written refuses, as before");
        assert_eq!(
            std::fs::read(&both).unwrap(),
            both_text.as_bytes(),
            "nothing was written"
        );
        // Neither key: the shipped root with its `providers_file:` line
        // removed.
        let line = EMBEDDED_TEMPLATE
            .lines()
            .find(|l| l.starts_with("providers_file:"))
            .unwrap();
        let neither = dir.join("neither.yaml");
        let neither_text = EMBEDDED_TEMPLATE.replace(&format!("{line}\n"), "");
        std::fs::write(&neither, &neither_text).unwrap();
        let code = one_change(&neither);
        assert_eq!(code, 2, "neither key written refuses, as before");
        assert_eq!(
            std::fs::read(&neither).unwrap(),
            neither_text.as_bytes(),
            "nothing was written"
        );
        assert!(
            !dir.join("providers.example.yaml").exists(),
            "a refused run creates no roster file"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (b) after the shape step, the `providers` section's answered
    /// question lands in the **roster** the run just created — the root
    /// does not move one byte (G2 read for the moved block).
    #[test]
    fn providers_edit_lands_in_the_moved_roster() {
        let dir = temp_root("edit-after-split");
        let target = dir.join("config.yaml");
        std::fs::write(&target, inline_fixture()).unwrap();
        assert_eq!(run_with_prompt(&args(&target), &non_interactive()), 0);
        let roster = dir.join("providers.example.yaml");
        let moved = format!("{}\n", EMBEDDED_ROSTER.trim_end());
        let (root_before, root_mtime) = (
            std::fs::read(&target).unwrap(),
            std::fs::metadata(&target).unwrap().modified().unwrap(),
        );
        let code = run_with_prompt(
            &SetupArgs {
                section: Some("providers".to_string()),
                config: Some(target.display().to_string()),
                ..Default::default()
            },
            &scripted_owned(providers_answers("DEEPSEEK_API_KEY_2")),
        );
        assert_eq!(code, 0);
        // The root: byte- and mtime-identical — the edit is not its.
        assert_eq!(root_before, std::fs::read(&target).unwrap());
        assert_eq!(
            root_mtime,
            std::fs::metadata(&target).unwrap().modified().unwrap()
        );
        // The roster: exactly the answered key's own line moves.
        let a = anchor::resolve_typed(&moved, "providers[name=deepseek].api_key_env").unwrap();
        let base_lines: Vec<&str> = moved.lines().collect();
        let after = std::fs::read_to_string(&roster).unwrap();
        let after_lines: Vec<&str> = after.lines().collect();
        assert_eq!(base_lines.len(), after_lines.len());
        let moved_lines: Vec<usize> = base_lines
            .iter()
            .zip(after_lines.iter())
            .enumerate()
            .filter(|(_, (b, c))| b != c)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(moved_lines, vec![a.line], "only the answered line may move");
        assert!(after_lines[a.line].contains("DEEPSEEK_API_KEY_2"));
        assert!(crate::config_load::load(&target).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (c) `--from <roster> --force` over a split root replaces the
    /// roster **as a unit** — the new roster's bytes land verbatim, the
    /// replaced roster survives at `<roster>.bak` (`--force` implies
    /// `--backup`), and no other byte of any file moves: the root keeps
    /// even a hand-written note, and no root backup is made (ADR-037 D9;
    /// a replacement, never an insertion — Q21's boundary stands).
    #[test]
    fn from_roster_force_replaces_the_roster_and_nothing_else() {
        let dir = temp_root("roster-swap");
        let target = dir.join("config.yaml");
        run_with_prompt(&args(&target), &non_interactive());
        // A hand note in the root the swap must not touch.
        let hand = EMBEDDED_TEMPLATE.replace(
            "  request_timeout: 10m",
            "  request_timeout: 10m          # my own note",
        );
        std::fs::write(&target, &hand).unwrap();
        // The operator's replacement roster: the shipped one with one
        // entry's key-variable name changed.
        let replacement =
            EMBEDDED_ROSTER.replace("api_key_env: DEEPSEEK_API_KEY", "api_key_env: DS_KEY");
        let from = dir.join("my-roster.yaml");
        std::fs::write(&from, &replacement).unwrap();
        let code = run_with_prompt(
            &SetupArgs {
                section: Some("providers".to_string()),
                from: Some(from.display().to_string()),
                force: true,
                non_interactive: true,
                config: Some(target.display().to_string()),
                ..Default::default()
            },
            &non_interactive(),
        );
        assert_eq!(code, 0);
        let roster = dir.join("providers.example.yaml");
        assert_eq!(
            std::fs::read(&roster).unwrap(),
            replacement.as_bytes(),
            "the roster is the handed file, verbatim"
        );
        assert_eq!(
            std::fs::read(dir.join("providers.example.yaml.bak")).unwrap(),
            EMBEDDED_ROSTER.as_bytes(),
            "the replaced roster survives at <roster>.bak"
        );
        assert_eq!(
            std::fs::read(&target).unwrap(),
            hand.as_bytes(),
            "the root did not move one byte (the hand note survives)"
        );
        assert!(
            !dir.join("config.yaml.bak").exists(),
            "the root was not written, so it was not backed up"
        );
        assert!(crate::config_load::load(&target).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------
    // The rule-file lane (ADR-046; spec §4.11's rule-file paragraph;
    // DESIGN §12.14 step 1c and its rig). The shipped template's
    // `tool-output-rules` entry names `./rules/tool_output.toml`,
    // resolved against the config file's own directory — so every
    // helper below reads the lane's path from the base's own text.
    // -----------------------------------------------------------------

    /// The rule file a landed tree carries, read from the base root's
    /// own text through the same anchor walk the run uses (`fn
    /// rule_file_target`), never from a constant in the test.
    fn rule_file_of(config: &std::path::Path) -> std::path::PathBuf {
        let dir = config.parent().unwrap().to_path_buf();
        let base = std::fs::read_to_string(config).unwrap();
        rule_file_target(&base, &dir).expect("the shipped base names a rule file")
    }

    /// The repository's own `rules/tool_output.toml`, read from the
    /// source tree at test time — the byte-equal witness that stands in
    /// for the sha256 comparison in-process (the hash itself is taken by
    /// the round's harness probe on the real binary; byte equality is
    /// strictly stronger, and no hash crate is a vadis-cli dependency).
    fn repo_rules_file() -> std::path::PathBuf {
        std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../rules/tool_output.toml"
        ))
        .to_path_buf()
    }

    /// Limb 1 — fresh run: the root, the roster **and** the rule file
    /// land; the rule file's bytes equal the embedded template's (one
    /// hash comparison — taken byte-exact here, by `shasum -a 256` in
    /// the round's harness probe), and `rules/` / the file carry
    /// `0700` / `0600` read back from the filesystem (G8, the creation
    /// half). The landed bundle loads through the same loader `serve`
    /// runs.
    #[test]
    fn fresh_run_lands_the_rule_file_with_modes() {
        let dir = temp_root("rule-fresh");
        let target = dir.join("config.yaml");
        assert_eq!(run_with_prompt(&args(&target), &non_interactive()), 0);
        let rule = rule_file_of(&target);
        assert_eq!(rule, dir.join("rules/tool_output.toml"));
        assert_eq!(
            std::fs::read(&rule).unwrap(),
            EMBEDDED_RULES.as_bytes(),
            "the created rule file's bytes are the embedded template's"
        );
        assert_eq!(
            EMBEDDED_RULES.as_bytes(),
            std::fs::read(repo_rules_file()).unwrap().as_slice(),
            "the embedded template is the repository's own rule file, unedited"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let file_mode = std::fs::metadata(&rule).unwrap().permissions().mode();
            assert_eq!(file_mode & 0o777, 0o600, "created rule file is 0600");
            let dir_mode = std::fs::metadata(dir.join("rules"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(dir_mode & 0o777, 0o700, "created rules/ is 0700");
        }
        assert!(crate::config_load::load(&target).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A creation arm unqualified by section (ADR-046 D3): a one-section
    /// run that touches neither the roster nor the rule file's section
    /// still creates the rule file when it is absent — the same reason
    /// `vadis setup server` lands the pair.
    #[test]
    fn a_section_run_creates_an_absent_rule_file() {
        let dir = temp_root("rule-create-section");
        let target = dir.join("config.yaml");
        run_with_prompt(&args(&target), &non_interactive());
        // The operator deleted the rule file; a bare one-section run
        // recreates it.
        let rule = rule_file_of(&target);
        std::fs::remove_file(&rule).unwrap();
        let code = run_with_prompt(
            &SetupArgs {
                section: Some("server".to_string()),
                config: Some(target.display().to_string()),
                ..Default::default()
            },
            &scripted(&["127.0.0.1:9911", "", ""]),
        );
        assert_eq!(code, 0);
        assert_eq!(std::fs::read(&rule).unwrap(), EMBEDDED_RULES.as_bytes());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Limb 2 — idempotence (G3 / CONF-69's property, extended to the
    /// third file): the same command again leaves **all three** files'
    /// bytes and mtimes unmoved.
    #[test]
    fn second_run_leaves_all_three_files_untouched() {
        let dir = temp_root("rule-idem");
        let target = dir.join("config.yaml");
        assert_eq!(run_with_prompt(&args(&target), &non_interactive()), 0);
        let rule = rule_file_of(&target);
        let before: Vec<_> = [&target, &dir.join("providers.example.yaml"), &rule]
            .iter()
            .map(|p| {
                (
                    std::fs::read(p).unwrap(),
                    std::fs::metadata(p).unwrap().modified().unwrap(),
                )
            })
            .collect();
        assert_eq!(run_with_prompt(&args(&target), &non_interactive()), 0);
        for (p, (bytes, mtime)) in [&target, &dir.join("providers.example.yaml"), &rule]
            .iter()
            .zip(before.iter())
        {
            assert_eq!(&std::fs::read(p).unwrap(), bytes, "{} moved", p.display());
            assert_eq!(
                &std::fs::metadata(p).unwrap().modified().unwrap(),
                mtime,
                "{} was rewritten",
                p.display()
            );
        }
        assert!(
            !rule.with_extension("toml.bak").exists(),
            "a no-op run takes no rule-file backup"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Limb 3 — replacement and its RED control, both directions
    /// observed: with the rule file hand-edited, `plugins --force`
    /// replaces it with the embedded template **and** keeps the
    /// operator's bytes at `<file>.bak`; the control — the same
    /// hand-edited file under a bare `vadis setup` — is not touched
    /// (bytes and mtime identical).
    #[test]
    fn force_with_plugins_replaces_the_rule_file_bare_does_not() {
        let dir = temp_root("rule-force");
        let target = dir.join("config.yaml");
        run_with_prompt(&args(&target), &non_interactive());
        let rule = rule_file_of(&target);
        let bak = backup_path(&rule);
        // The operator's own rules — deterministic distinct bytes: a
        // header the template does not carry, prepended.
        let hand = format!("# R60-2 hand edit\n{EMBEDDED_RULES}");
        std::fs::write(&rule, &hand).unwrap();

        // The RED control FIRST: a bare run leaves it alone.
        let (h_bytes, h_mtime) = (
            std::fs::read(&rule).unwrap(),
            std::fs::metadata(&rule).unwrap().modified().unwrap(),
        );
        assert_eq!(run_with_prompt(&args(&target), &non_interactive()), 0);
        assert_eq!(
            std::fs::read(&rule).unwrap(),
            h_bytes,
            "the control: a bare run leaves the operator's rule file byte-identical"
        );
        assert_eq!(
            std::fs::metadata(&rule).unwrap().modified().unwrap(),
            h_mtime,
            "the control: a bare run leaves the operator's rule file's mtime unmoved"
        );
        assert!(!bak.exists(), "the control: no .bak was taken");

        // `--force` **without** the plugins section: still not replaced.
        let code = run_with_prompt(
            &SetupArgs {
                section: Some("server".to_string()),
                force: true,
                non_interactive: true,
                config: Some(target.display().to_string()),
                ..Default::default()
            },
            &scripted(&["127.0.0.1:9911", "", ""]),
        );
        assert_eq!(code, 0);
        assert_eq!(
            std::fs::read(&rule).unwrap(),
            hand.as_bytes(),
            "force without the plugins section replaces nothing"
        );
        assert!(!bak.exists());

        // `--force` **with** the plugins section: replaced, `.bak` first.
        let code = run_with_prompt(
            &SetupArgs {
                section: Some("plugins".to_string()),
                force: true,
                non_interactive: true,
                config: Some(target.display().to_string()),
                ..Default::default()
            },
            &non_interactive(),
        );
        assert_eq!(code, 0);
        assert_eq!(
            std::fs::read(&rule).unwrap(),
            EMBEDDED_RULES.as_bytes(),
            "plugins --force replaced the rule file with the embedded template"
        );
        assert_eq!(
            std::fs::read(&bak).unwrap(),
            hand.as_bytes(),
            "the operator's bytes survive at <file>.bak before the replacement"
        );
        // The wizard composed nothing inside the file: the bytes are the
        // template's own, always (D5).
        assert_eq!(
            std::fs::read(&rule).unwrap(),
            std::fs::read(repo_rules_file()).unwrap(),
            "the replaced bytes are the repository's own rule file, unedited"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// D4 — a roster-scoped replacement is not a rule-file replacement:
    /// `providers --from <roster> --force` over a split root swaps the
    /// roster as a unit and does not replace the rule file; only the
    /// create arm can touch it, and it fires only if the file is absent.
    #[test]
    fn a_roster_scoped_force_does_not_replace_the_rule_file() {
        let dir = temp_root("rule-roster-force");
        let target = dir.join("config.yaml");
        run_with_prompt(&args(&target), &non_interactive());
        let rule = rule_file_of(&target);
        let hand = format!("# R60-2 hand edit\n{EMBEDDED_RULES}");
        std::fs::write(&rule, &hand).unwrap();
        let replacement =
            EMBEDDED_ROSTER.replace("api_key_env: DEEPSEEK_API_KEY", "api_key_env: DS_KEY");
        let from = dir.join("my-roster.yaml");
        std::fs::write(&from, &replacement).unwrap();
        let code = run_with_prompt(
            &SetupArgs {
                section: Some("providers".to_string()),
                from: Some(from.display().to_string()),
                force: true,
                non_interactive: true,
                config: Some(target.display().to_string()),
                ..Default::default()
            },
            &non_interactive(),
        );
        assert_eq!(code, 0);
        assert_eq!(
            std::fs::read(&rule).unwrap(),
            hand.as_bytes(),
            "the roster swap does not replace the rule file (the plugins section is not in the list)"
        );
        assert!(
            !backup_path(&rule).exists(),
            "and it takes no rule-file backup"
        );
        assert_eq!(
            std::fs::read(dir.join("providers.example.yaml")).unwrap(),
            replacement.as_bytes(),
            "the roster itself was swapped (the run's own semantics held)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A base that names no rule file has no third lane: `plugins: []`
    /// materializes nothing (ADR-046 D1's last sentence; spec §4.11's
    /// *a base that names no rule file … materializes nothing*).
    #[test]
    fn a_base_naming_no_rule_file_materializes_nothing() {
        let dir = temp_root("rule-none");
        let target = dir.join("config.yaml");
        // The shipped template with its whole `plugins:` block replaced
        // by `plugins: []` — a legal config the loader accepts, and one
        // no transform entry of. The block's own bytes are located the
        // way the shape step locates the roster's: header line through
        // the last line before the next top-level key.
        let lines: Vec<&str> = EMBEDDED_TEMPLATE.lines().collect();
        let header = lines
            .iter()
            .position(|l| l.starts_with("plugins:"))
            .expect("the template carries a plugins block");
        let end = lines
            .iter()
            .enumerate()
            .skip(header + 1)
            .find(|(i, l)| {
                !l.trim().is_empty() && !l.starts_with(' ') && !l.starts_with('\t') && *i > header
            })
            .map(|(i, _)| i)
            .unwrap_or(lines.len());
        let mut text = lines[..header].join("\n");
        text.push_str("\nplugins: []\n");
        text.push_str(&lines[end..].join("\n"));
        text.push('\n');
        std::fs::write(&target, &text).unwrap();
        let roster = dir.join("providers.example.yaml");
        std::fs::write(&roster, EMBEDDED_ROSTER).unwrap();
        assert!(
            crate::config_load::load(&target).is_ok(),
            "the spliced base loads (the run is not a repair)"
        );
        let mtime = std::fs::metadata(&target).unwrap().modified().unwrap();
        assert_eq!(run_with_prompt(&args(&target), &non_interactive()), 0);
        assert!(
            !dir.join("rules").exists(),
            "no rule file lane: nothing materialized"
        );
        assert_eq!(
            mtime,
            std::fs::metadata(&target).unwrap().modified().unwrap(),
            "the root itself is a no-op (an empty plan over the file that is there)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Limb 5(a) — the end-to-end witness, in-process half: the plugins
    /// of the **landed** config, assembled with the landed config's own
    /// directory as the resolver (the same closure `serve` passes,
    /// `lib.rs`'s one `assemble` call), mount the transform engine —
    /// `transform_engine().is_some()`. Before the lane this was the
    /// named absence (ADR-046's *Background*): the declared entry
    /// pointed at a file nothing had written.
    #[test]
    fn the_landed_config_mounts_the_transform_engine() {
        let dir = temp_root("rule-assembly");
        let target = dir.join("config.yaml");
        assert_eq!(run_with_prompt(&args(&target), &non_interactive()), 0);
        let rc = crate::config_load::load(&target).expect("the landed bundle loads");
        assert!(
            rc.vadis
                .plugins
                .iter()
                .any(|p| p.kind == "builtin/transform_rules"),
            "the landed config declares the transform entry"
        );
        let assembly = vadis_plugins::assemble(&rc.vadis.plugins, &|file| {
            crate::config_load::resolve(&rc.config_dir, file)
        });
        assert!(
            assembly.transform_engine().is_some(),
            "the engine mounts from the landed rule file (the round's real acceptance)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `--dry-run` names the file it would create (ADR-046 D9): the
    /// same per-lane `would write …` line the other lanes print, and
    /// nothing lands — not the rule file, not its directory.
    #[test]
    fn dry_run_names_the_rule_file_and_lands_nothing() {
        let dir = temp_root("rule-dryrun");
        let target = dir.join("config.yaml");
        run_with_prompt(&args(&target), &non_interactive());
        let rule = rule_file_of(&target);
        std::fs::remove_file(&rule).unwrap();
        std::fs::remove_dir(dir.join("rules")).unwrap();
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
        assert!(!rule.exists(), "--dry-run: the rule file is not created");
        assert!(
            !dir.join("rules").exists(),
            "--dry-run: no rules/ directory"
        );
        // Over a present rule file the same run says `no change` for it
        // — asserted on the binary in the round's behavioral probe
        // (stdout is not capturable in-process); here the lane not
        // landing is the fact under test.
        let _ = std::fs::remove_dir_all(&dir);
    }
}
