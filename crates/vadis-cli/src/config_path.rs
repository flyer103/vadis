//! The config file's location rule (spec §4.12, ADR-025 decisions 8–9):
//! one discovery order, obeyed by `serve`, `stats` and `setup` alike.
//!
//! 1. an explicit `--config <path>` — a path that does not exist is an
//!    error, never a fall-through to a later candidate;
//! 2. `${XDG_CONFIG_HOME:-$HOME/.config}/vadis/config.yaml` when it exists;
//! 3. `./config.yaml` when it exists;
//! 4. (the writer only) the XDG location, **created**.
//!
//! The rule lives here — not under `setup/` — because both sides of it call
//! it: `main` resolves once and hands an absolute path to `serve` / `stats` /
//! `setup`, whose entry-point signatures stay unchanged (CONF-23/25/43's rigs
//! drive them directly).
//!
//! This module resolves *which file*, never what the file says: the listen
//! address, the plugin set and the roster still come only from the file
//! (CONF-25).

use std::path::{Path, PathBuf};

/// Which rule of the §4.12 order selected the path — reported to the user
/// (`"selected_by"` in `--json`) so "did my `--config` matter?" and "where
/// did that file come from?" are answered by the command, not by guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectedBy {
    Flag,
    Xdg,
    Cwd,
    XdgCreated,
}

impl SelectedBy {
    pub fn as_str(self) -> &'static str {
        match self {
            SelectedBy::Flag => "flag",
            SelectedBy::Xdg => "xdg",
            SelectedBy::Cwd => "cwd",
            SelectedBy::XdgCreated => "xdg-created",
        }
    }
}

/// The resolved target: an absolute path plus the rule that chose it.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub path: PathBuf,
    pub selected_by: SelectedBy,
}

/// The XDG candidate: `${XDG_CONFIG_HOME:-$HOME/.config}/vadis/config.yaml`.
/// `None` when neither variable is set (an unusual environment; candidates
/// 2 and 4 are simply absent in it).
pub fn xdg_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(|h| PathBuf::from(h).join(".config"))
        })?;
    Some(base.join("vadis").join("config.yaml"))
}

/// Lexically absolutize without touching the filesystem (the CWD-relative
/// candidate must be reported as an absolute path whatever the process's
/// working directory is).
fn absolute(p: &Path) -> PathBuf {
    if p.is_absolute() {
        return normalize(p);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    normalize(&cwd.join(p))
}

/// Drop `.` components and collapse non-leading `..` pairs, mirroring
/// `config_load`'s anchor normalization so one path has one spelling.
fn normalize(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// The reading side (`serve`, `stats`, and `setup`'s `--print` / `--check`):
/// candidates 1–3, else **refuse** — the error names both `--config` and
/// `vadis setup`, the command that would create the file (spec §4.12's
/// row 4: a reader never invents a location).
pub fn resolve_read(explicit: Option<&str>) -> Result<Resolved, String> {
    if let Some(flag) = explicit {
        let path = absolute(Path::new(flag));
        if !path.is_file() {
            return Err(format!(
                "config file {}: does not exist — an explicit --config never \
                 falls back to another location; pass an existing file, or run \
                 `vadis setup` to create one",
                path.display()
            ));
        }
        return Ok(Resolved {
            path,
            selected_by: SelectedBy::Flag,
        });
    }
    if let Some(xdg) = xdg_path() {
        if xdg.is_file() {
            return Ok(Resolved {
                path: xdg,
                selected_by: SelectedBy::Xdg,
            });
        }
    }
    let cwd = PathBuf::from("config.yaml");
    if cwd.is_file() {
        return Ok(Resolved {
            path: absolute(&cwd),
            selected_by: SelectedBy::Cwd,
        });
    }
    Err("no config file found: no --config was given, and neither \
         ${XDG_CONFIG_HOME:-$HOME/.config}/vadis/config.yaml nor ./config.yaml \
         exists — pass --config <path>, or run `vadis setup` to create the \
         XDG file"
        .to_string())
}

/// The writing side (`setup`): candidates 1–3, else candidate 4 — the XDG
/// location, to be created by the caller (`mkdir -p`; the created directory
/// `0700`, the created file `0600`). The path is returned; creation happens
/// once, at landing time, so a refused run builds nothing.
pub fn resolve_write(explicit: Option<&str>) -> Result<Resolved, String> {
    if let Some(flag) = explicit {
        let path = absolute(Path::new(flag));
        if path.is_dir() {
            return Err(format!("config target {}: is a directory", path.display()));
        }
        return Ok(Resolved {
            path,
            selected_by: SelectedBy::Flag,
        });
    }
    if let Some(xdg) = xdg_path() {
        if xdg.is_file() {
            return Ok(Resolved {
                path: xdg,
                selected_by: SelectedBy::Xdg,
            });
        }
    }
    let cwd = PathBuf::from("config.yaml");
    if cwd.is_file() {
        return Ok(Resolved {
            path: absolute(&cwd),
            selected_by: SelectedBy::Cwd,
        });
    }
    // Candidate 4: the XDG location, created. It has no XDG/HOME anchor in
    // this environment only when HOME is unset too — an environment with no
    // home to put a config in, which the writer refuses rather than guesses.
    let xdg = xdg_path().ok_or_else(|| {
        "no default config location: neither XDG_CONFIG_HOME nor HOME is set — \
         pass --config <path>"
            .to_string()
    })?;
    Ok(Resolved {
        path: xdg,
        selected_by: SelectedBy::XdgCreated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // The order itself is exercised end-to-end by the self-check rigs (the
    // process environment cannot be sandboxed inside a unit test); these
    // pin the pure halves: absolutization and the refused shapes that do
    // not depend on the ambient environment.

    #[test]
    fn absolute_dots_are_normalized() {
        let a = absolute(Path::new("a/./b/../c.yaml"));
        let expect = std::env::current_dir().unwrap().join("a").join("c.yaml");
        assert_eq!(a, expect);
    }

    #[test]
    fn explicit_flag_is_absolutized_not_checked() {
        // The write side never stats a --config target that is absent — a
        // fresh target is the normal first-run case, and whether an absent
        // *reader* target is an error is resolve_read's concern.
        let r = resolve_write(Some("no-such-dir/x.yaml")).unwrap();
        assert_eq!(r.selected_by, SelectedBy::Flag);
        assert!(r.path.is_absolute());
        assert!(r.path.ends_with("no-such-dir/x.yaml"));
    }

    #[test]
    fn read_refusal_names_both_exits() {
        // Without touching the environment: the flag branch's refusal is
        // testable whatever the ambient XDG/CWD state is.
        let err = resolve_read(Some("/definitely/not/here.yaml")).unwrap_err();
        assert!(err.contains("--config"), "got: {err}");
        assert!(err.contains("vadis setup"), "got: {err}");
    }
}
