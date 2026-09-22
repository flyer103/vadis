//! setup/edit.rs — the two edit kinds, the value codec, `Plan`, `apply`
//! (DESIGN §12.14; spec §4.11's write strategy).
//!
//! An edit is `(line, byte range within that line, replacement)`. The two
//! kinds are exactly the two the contract allows: `set-value` (replace the
//! value's extent on a single line) and `set-enabled` (add or remove the
//! leading comment marker on the key's own line). Nothing else — no
//! insertion, no deletion, no reformatting (ADR-025 decision 1/7).

use std::ops::Range;

/// Which of the two edit kinds an entry is — carried so `--dry-run` can
/// print the kind (spec §4.11's `--dry-run` row).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    SetValue,
    SetEnabled,
}

impl EditKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EditKind::SetValue => "set-value",
            EditKind::SetEnabled => "set-enabled",
        }
    }
}

/// One planned edit. `old` keeps the bytes being replaced so `--dry-run`
/// can print `<anchor>: <old> → <new>` without re-reading the base.
#[derive(Debug, Clone)]
pub struct Edit {
    pub kind: EditKind,
    /// The key path the edit belongs to (for messages and `--dry-run`).
    pub path: String,
    /// 0-based line index.
    pub line: usize,
    /// Byte extent **within that line** (the line taken without its
    /// terminator).
    pub range: Range<usize>,
    pub replacement: String,
    pub old: String,
}

/// A plan: edits sorted by `(line, range.start)` and asserted pairwise
/// disjoint — overlapping ranges are a refusal that writes nothing
/// (spec §4.11 step 3).
#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub edits: Vec<Edit>,
}

impl Plan {
    pub fn build(mut edits: Vec<Edit>) -> Result<Plan, String> {
        edits.sort_by(|a, b| a.line.cmp(&b.line).then(a.range.start.cmp(&b.range.start)));
        for w in edits.windows(2) {
            let (a, b) = (&w[0], &w[1]);
            if a.line == b.line && b.range.start < a.range.end {
                return Err(format!(
                    "edits for `{}` and `{}` overlap on line {}",
                    a.path,
                    b.path,
                    a.line + 1
                ));
            }
        }
        Ok(Plan { edits })
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }
}

/// Split into lines, **keeping** each line's terminator (`\n` or `\r\n`)
/// so `apply` can splice inside the content and re-append it verbatim —
/// the base's line endings are part of its bytes and are never rewritten.
fn split_keep_ends(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = s.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'\n' {
            out.push(&s[start..=i]);
            start = i + 1;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

fn split_terminator(line: &str) -> (&str, &str) {
    match line.strip_suffix("\r\n") {
        Some(content) => (content, "\r\n"),
        None => match line.strip_suffix('\n') {
            Some(content) => (content, "\n"),
            None => (line, ""),
        },
    }
}

/// Splice the plan into a **copy** of the base. Pure: the base is never
/// touched, and the output is a function of (base, plan) only — never of
/// the clock, the CWD or the environment (spec §4.11's determinism head).
pub fn apply(base: &str, plan: &Plan) -> String {
    use std::collections::BTreeMap;
    let lines = split_keep_ends(base);
    let mut by_line: BTreeMap<usize, Vec<&Edit>> = BTreeMap::new();
    for e in &plan.edits {
        by_line.entry(e.line).or_default().push(e);
    }
    let mut out = String::with_capacity(base.len() + 64);
    for (i, line) in lines.iter().enumerate() {
        match by_line.get(&i) {
            None => out.push_str(line),
            Some(edits) => {
                // `Plan::build` sorted by (line, start); within one line
                // the edits are therefore in offset order and disjoint.
                let (content, term) = split_terminator(line);
                let mut pos = 0;
                for e in edits {
                    out.push_str(&content[pos..e.range.start]);
                    out.push_str(&e.replacement);
                    pos = e.range.end;
                }
                out.push_str(&content[pos..]);
                out.push_str(term);
            }
        }
    }
    out
}

/// Encode an answer in the anchor's own style at that key: quoted iff the
/// value there is quoted, bare otherwise — the wizard changes a value,
/// never a style (spec §4.11 step 2; DESIGN §12.14's value codec).
pub fn encode(quote: Option<char>, raw: &str) -> String {
    match quote {
        Some(q) => format!("{q}{raw}{q}"),
        None => raw.to_string(),
    }
}

/// The §12.5 duration grammar: `<integer><ms|s|m|h>`, stackable
/// (`1h30m`); bare seconds with underscores are not supported. A duration
/// answer is validated **before** it is encoded (spec §4.11 step 2); the
/// loader validates the candidate again afterwards.
pub fn is_duration(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    let mut any = false;
    while i < b.len() {
        let digits_start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == digits_start {
            return false;
        }
        if b[i..].starts_with(b"ms") {
            i += 2;
        } else if matches!(b.get(i), Some(b's') | Some(b'm') | Some(b'h')) {
            i += 1;
        } else {
            return false;
        }
        any = true;
    }
    any
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(line: usize, start: usize, end: usize, repl: &str) -> Edit {
        Edit {
            kind: EditKind::SetValue,
            path: format!("k{line}"),
            line,
            range: start..end,
            replacement: repl.to_string(),
            old: String::new(),
        }
    }

    #[test]
    fn apply_splices_within_lines_and_keeps_terminators() {
        let base = "a: 1\r\nb: 22\nc: 3\n";
        let plan = Plan::build(vec![edit(0, 3, 4, "9"), edit(2, 3, 4, "7")]).unwrap();
        assert_eq!(apply(base, &plan), "a: 9\r\nb: 22\nc: 7\n");
    }

    #[test]
    fn overlapping_edits_are_refused() {
        let err = Plan::build(vec![edit(0, 3, 8, "x"), edit(0, 5, 6, "y")]).unwrap_err();
        assert!(err.contains("overlap"), "got: {err}");
    }

    #[test]
    fn edits_on_different_lines_never_overlap() {
        assert!(Plan::build(vec![edit(0, 0, 9, "x"), edit(1, 0, 9, "y")]).is_ok());
    }

    #[test]
    fn last_line_without_terminator_is_preserved() {
        let plan = Plan::build(vec![edit(0, 3, 4, "9")]).unwrap();
        assert_eq!(apply("a: 1", &plan), "a: 9");
    }

    #[test]
    fn codec_reuses_the_anchor_quote_style() {
        assert_eq!(encode(Some('"'), "0.0.0.0:8790"), "\"0.0.0.0:8790\"");
        assert_eq!(encode(None, "true"), "true");
    }

    #[test]
    fn duration_grammar() {
        for ok in ["60s", "10m", "12h", "1h30m", "300ms", "2h5m30s"] {
            assert!(is_duration(ok), "{ok} should parse");
        }
        for bad in ["", "s", "1", "1x", "1 h", "1_000s", "1h m", "ms"] {
            assert!(!is_duration(bad), "{bad} should not parse");
        }
    }
}
