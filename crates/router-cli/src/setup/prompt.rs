//! setup/prompt.rs — the answer channel: `std::io::IsTerminal` on stdin,
//! line reads, the `[default]` rendering (DESIGN §12.14). No prompt crate
//! (ADR-025's alternatives: the questions are lines with defaults, and the
//! one place a masked input would be wanted is a place this design refuses
//! to go).

use std::cell::RefCell;
use std::io::{BufRead, IsTerminal, Write};

/// How a question reaches the user and its answer comes back.
pub struct Prompt {
    /// `false` under `--non-interactive`: every question takes its
    /// default without touching stdin or stdout.
    pub interactive: bool,
    /// Scripted answers (the in-process tests' answer channel; empty in
    /// the real run, where answers come from stdin). Consumed in order;
    /// an empty channel falls through to stdin so a partially scripted
    /// run behaves like a user who stopped typing (EOF ⇒ cancel).
    scripted: RefCell<Vec<String>>,
}

/// One answer: `Enter` (the default), a value, or EOF mid-run — which is
/// a **cancel**: a half-answered run never lands (DESIGN §12.14 step 5).
pub enum Answer {
    Default,
    Value(String),
    Eof,
}

impl Prompt {
    pub fn new(interactive: bool) -> Prompt {
        Prompt {
            interactive,
            scripted: RefCell::new(Vec::new()),
        }
    }

    /// Load a scripted answer list (the tests' channel; the real run
    /// never calls this). Consumed first-in-first-out.
    pub fn load_answers(&self, answers: Vec<String>) {
        let mut v = answers;
        v.reverse(); // `line` pops from the back
        *self.scripted.borrow_mut() = v;
    }

    /// Ask one free-line question. `print` carries the fully rendered
    /// question (the caller knows the key, the default and the note).
    pub fn line(&self, question: &str) -> Answer {
        if let Some(a) = self.scripted.borrow_mut().pop() {
            return if a.trim().is_empty() {
                Answer::Default
            } else {
                Answer::Value(a.trim().to_string())
            };
        }
        if !self.interactive {
            return Answer::Default;
        }
        Self::ask_stdin(question)
    }

    fn ask_stdin(question: &str) -> Answer {
        print!("{question} ");
        let _ = std::io::stdout().flush();
        let mut buf = String::new();
        match std::io::stdin().lock().read_line(&mut buf) {
            Ok(0) => Answer::Eof,
            Ok(_) => {
                let t = buf.trim_end_matches(['\n', '\r']);
                if t.trim().is_empty() {
                    Answer::Default
                } else {
                    Answer::Value(t.trim().to_string())
                }
            }
            Err(_) => Answer::Eof,
        }
    }
}

/// The `no terminal on stdin` branch (spec §4.11): with stdin not a
/// terminal and `--non-interactive` absent the command prompts nothing,
/// prints the exact working command line plus the export snippets, and
/// exits 2 — refuse (a run that did not do the work is not a success)
/// **and** hand over the command that does work.
pub fn stdin_is_terminal() -> bool {
    std::io::stdin().is_terminal()
}

/// Render one question with its default, in the wizard's one shape:
/// `section.key [<default>] (note):`. The default is the file's own value
/// — the thing Enter keeps.
pub fn render(path: &str, default: &str, note: &str) -> String {
    if note.is_empty() {
        format!("{path} [{default}]:")
    } else {
        format!("{path} [{default}] ({note}):")
    }
}

/// Render an enum question with the default item marked:
/// `plan_policy.recover [probe|none] (default probe):`.
pub fn render_enum(path: &str, items: &[&str], default: &str) -> String {
    let list = items
        .iter()
        .map(|i| {
            if *i == default {
                format!("{i}*")
            } else {
                (*i).to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("|");
    format!("{path} [{list}] (default {default}):")
}

/// The export snippet printed for each missing environment **name** —
/// names only; no value is ever read, printed or written (spec §4.11's
/// secret boundary).
pub fn export_snippet(name: &str) -> String {
    format!("export {name}='<paste the key here>'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendering_shapes() {
        assert_eq!(
            render("server.addr", "127.0.0.1:8790", ""),
            "server.addr [127.0.0.1:8790]:"
        );
        assert_eq!(
            render_enum("plan_policy.recover", &["probe", "none"], "probe"),
            "plan_policy.recover [probe*|none] (default probe):"
        );
        assert_eq!(
            export_snippet("DEEPSEEK_API_KEY"),
            "export DEEPSEEK_API_KEY='<paste the key here>'"
        );
    }

    #[test]
    fn non_interactive_never_touches_io() {
        let p = Prompt::new(false);
        assert!(matches!(p.line("x [y]:"), Answer::Default));
    }
}
