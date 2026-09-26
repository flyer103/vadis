//! setup/split.rs — the shape step (ADR-038; spec §4.11's *shape step*,
//! §4.14): a writing run never leaves the roster inline.
//!
//! Pure over bytes: the moved span leaves the root and **is** the roster
//! file's content — entries, comments and `source:` citations verbatim —
//! and the root keeps every other byte, with the shipped template's own
//! `providers_file:` line in the header line's place. No document is
//! built, no value is re-encoded and no prose is composed (ADR-025's
//! write strategy: the file is edited, never reproduced).

use crate::setup::anchor::{self, AnchorError};

/// The `providers:` block's span in a root that carries the roster
/// inline: the header line and the last line the block owns (0-based,
/// inclusive; see `anchor::block_span` for the rule).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub header: usize,
    pub last: usize,
}

impl Span {
    /// How many lines moved.
    pub fn lines(&self) -> usize {
        self.last - self.header + 1
    }

    /// The first and last line as a user counts them (1-based) — the
    /// numbers the report prints.
    pub fn first_line(&self) -> usize {
        self.header + 1
    }

    pub fn last_line(&self) -> usize {
        self.last + 1
    }
}

/// The normalization's result: the root's new bytes, the roster file's
/// bytes, and the span that moved between them.
#[derive(Debug, Clone, PartialEq)]
pub struct Split {
    pub root: String,
    pub roster: String,
    pub span: Span,
}

impl Split {
    /// The moved content's size, in bytes (the report's figure; the same
    /// bytes are the roster file's).
    pub fn bytes(&self) -> usize {
        self.roster.len()
    }
}

/// Lines with their terminators kept, so a splice can reuse the base's own
/// endings (`\n` or `\r\n`) instead of choosing one.
fn lines_keep_ends(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

fn terminator(line: &str) -> &str {
    match line.strip_suffix("\r\n") {
        Some(_) => "\r\n",
        None => match line.strip_suffix('\n') {
            Some(_) => "\n",
            None => "",
        },
    }
}

/// Normalize a root that carries its roster inline: the block's bytes
/// become `roster` and `replacement_line` takes the header line's place.
///
/// `Ok(None)` when the text carries no such block the locator can find
/// uniquely — the run has nothing to move, and what to do about the file
/// is the loader's call, not this function's. `Err` carries the locator's
/// refusal (a commented copy of the key beside the enabled one is two
/// lines, and the shape step refuses rather than guessing which block to
/// move).
///
/// The replacement line is the caller's — the **shipped template's own**
/// `providers_file:` line (ADR-038 D3/D4), never a string assembled here.
pub fn inline_to_pair(root_text: &str, replacement_line: &str) -> Result<Option<Split>, String> {
    debug_assert!(
        !replacement_line.contains('\n'),
        "the replacement is one line (the template's own)"
    );
    let (header, last) = match anchor::block_span(root_text, "providers") {
        Ok(s) => s,
        Err(AnchorError::NoSuchKey) => return Ok(None),
        Err(e) => {
            return Err(format!(
                "providers: {} (the inline roster cannot be moved as a block); nothing was written",
                e.reason()
            ))
        }
    };
    let lines = lines_keep_ends(root_text);
    if lines.get(header).is_none() || lines.get(last).is_none() {
        return Ok(None);
    }
    let roster: String = lines[header..=last].concat();
    let mut root = String::with_capacity(root_text.len() + replacement_line.len() + 2);
    root.push_str(&lines[..header].concat());
    root.push_str(replacement_line);
    root.push_str(terminator(lines[header]));
    root.push_str(&lines[last + 1..].concat());
    Ok(Some(Split {
        root,
        roster,
        span: Span { header, last },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const INLINE: &str = "server:\n  addr: \"127.0.0.1:8790\"\nproviders:\n  # source: https://example.com @2026-09-26\n  - name: p\n    api_key_env: K\n    models:\n      - id: m\n\naliases:\n  a: p/m\n";

    /// The move: the roster's bytes are the span's bytes, and the root's
    /// bytes are the base's with the span replaced by the given line —
    /// nothing else moves, and the trailing blank line stays where it was
    /// (it separates the block from the next key; it is not part of the
    /// span).
    #[test]
    fn the_span_moves_and_nothing_else_does() {
        let line = "providers_file: providers.example.yaml  # the template's own line";
        let s = inline_to_pair(INLINE, line).unwrap().unwrap();
        assert_eq!(s.span.first_line(), 3);
        assert_eq!(s.span.last_line(), 8);
        assert_eq!(s.span.lines(), 6);
        assert_eq!(
            s.roster,
            "providers:\n  # source: https://example.com @2026-09-26\n  - name: p\n    api_key_env: K\n    models:\n      - id: m\n"
        );
        assert_eq!(s.bytes(), s.roster.len());
        assert_eq!(
            s.root,
            format!("server:\n  addr: \"127.0.0.1:8790\"\n{line}\n\naliases:\n  a: p/m\n")
        );
        // The relation, not the fixture: the base's lines outside the span
        // are the new root's lines outside the header, in order.
        let base: Vec<&str> = INLINE.split_inclusive('\n').collect();
        let after: Vec<&str> = s.root.split_inclusive('\n').collect();
        assert_eq!(&after[..2], &base[..2], "the lines above the span");
        assert_eq!(&after[3..], &base[8..], "the lines below the span");
    }

    /// A line ending the file chooses is the line ending the move keeps —
    /// a `\r\n` root yields a `\r\n` replacement and a `\r\n` roster.
    #[test]
    fn the_files_own_line_endings_survive() {
        let crlf = INLINE.replace('\n', "\r\n");
        let s = inline_to_pair(&crlf, "providers_file: r.yaml")
            .unwrap()
            .unwrap();
        assert!(s.roster.contains("\r\n"), "the roster keeps CRLF");
        assert!(s.root.contains("providers_file: r.yaml\r\n"));
        assert!(!s.root.contains("providers:\r\n  # source"));
        // And a file whose last line has no terminator still moves: the
        // block's own bytes end the roster, the root's last line is
        // whatever followed the span (here: nothing).
        let eof = "server:\n  addr: a\nproviders:\n  - name: p";
        let s = inline_to_pair(eof, "providers_file: r.yaml")
            .unwrap()
            .unwrap();
        assert_eq!(s.roster, "providers:\n  - name: p");
        assert_eq!(s.root, "server:\n  addr: a\nproviders_file: r.yaml\n");
    }

    /// A value on the header line's own line is a one-line span, and a
    /// text with no `providers:` line at all is `None` — the shape step
    /// has nothing to move and says so by doing nothing.
    #[test]
    fn one_line_and_absent_shapes() {
        let s = inline_to_pair(
            "trace:\n  dir: ./t\nproviders: []\naliases:\n  a: b\n",
            "providers_file: r.yaml",
        )
        .unwrap()
        .unwrap();
        assert_eq!(s.span.lines(), 1);
        assert_eq!(s.roster, "providers: []\n");
        assert_eq!(
            s.root,
            "trace:\n  dir: ./t\nproviders_file: r.yaml\naliases:\n  a: b\n"
        );

        assert_eq!(
            inline_to_pair("trace:\n  dir: ./t\n", "providers_file: r.yaml"),
            Ok(None)
        );
    }

    /// Two lines claiming the key is a refusal, not a guess: the shape
    /// step names the reason and the caller writes nothing (spec §4.11's
    /// refusal ladder read for the shape).
    #[test]
    fn an_ambiguous_header_refuses() {
        let two = "providers:\n  - name: p\n# providers:\n";
        let err = inline_to_pair(two, "providers_file: r.yaml").unwrap_err();
        assert!(err.contains("more than one line"), "got: {err}");
        assert!(err.contains("nothing was written"), "got: {err}");
    }

    /// The round trip against the **shipped pair**: the shipped root with
    /// the shipped roster spliced back in where its `providers_file:` line
    /// stands is the pre-split example, and splitting it reproduces the
    /// shipped root byte for byte — with the roster carrying the 91
    /// `source:` citations it is the reason the move is byte-for-byte.
    #[test]
    fn splitting_the_pre_split_example_reproduces_the_shipped_pair() {
        let root = include_str!("../../../../config.example.yaml");
        let roster = include_str!("../../../../providers.example.yaml");
        let line = root
            .lines()
            .find(|l| l.starts_with("providers_file:"))
            .expect("the shipped root names its roster");
        let inline = root.replace(&format!("{line}\n"), &format!("{}\n", roster.trim_end()));
        assert!(
            inline.contains("providers:\n")
                && !inline.lines().any(|l| l.starts_with("providers_file:")),
            "the fixture is the inline form"
        );

        let s = inline_to_pair(&inline, line).unwrap().unwrap();
        assert_eq!(
            s.root, root,
            "the split of the pre-split example is the shipped root"
        );
        assert_eq!(
            s.roster,
            roster[..roster.len() - 1],
            "the moved span is the shipped roster without its final blank line"
        );
        assert!(
            s.roster.matches("source:").count() == roster.matches("source:").count(),
            "every citation travelled with the block"
        );
        // Conservation: the pair's bytes are the inline text's, minus the
        // span, plus the one line the move wrote — nothing else.
        assert_eq!(
            s.root.len() + s.roster.len(),
            inline.len() + line.len() + 1,
            "the move conserves the bytes"
        );
    }
}
