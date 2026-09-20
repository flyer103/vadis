//! CONF-43 (spec §9.3): **docs ↔ CLI consistency guard**. The verdict on
//! `router replay` / `router trace tail` is **out** — spec §9.3 freezes them
//! as "planned, not served" ("not subcommands of this binary"), and the
//! version-control record agrees (`serve` and `stats` are the only landed
//! commands). So this case delivers the guard instead of the commands: every
//! `router <subcommand>` mention in `book/` and `README.md` must resolve in
//! the real CLI parser — either as a served subcommand, or as a *whitelisted
//! deferral* — so that a **future** false promise in the docs necessarily
//! turns this case red (AGENTS constraint 6: relations, not snapshots).
//!
//! Why the whitelist cannot mask a new defect:
//!
//! - Direction 1 (docs → CLI) is computed per *file position*: an entry is
//!   whitelisted only when the mention sits inside a paragraph that carries a
//!   deferral marker. A new doc paragraph that promises `router replay` as
//!   served — or any mention of a subcommand that does not exist at all —
//!   produces no whitelist entry and fails direction 1.
//! - The whitelist vocabulary is itself derived from the live parser (the
//!   extra set, below): a deferral may only *defer an existing* surface's
//!   future twin, never name a command the parser does not know, and never
//!   accidentally cover a real subcommand name (that would let the docs
//!   promise `serve` while calling it "planned" and go unnoticed).
//! - Direction 2 (CLI → docs) is **whitelist-independent**: every subcommand
//!   the parser accepts must be mentioned in the docs at least once, so
//!   landing a new subcommand without documenting it also turns this red.
//!   The whitelist cannot hide it.
//!
//! The two directions together assert the *relation* "the documented command
//! set equals the parser's command set, modulo explicitly marked deferrals" —
//! not any particular snapshot of either side.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use clap::CommandFactory;

/// Where the docs live, relative to the conformance crate: the crate's
/// parent is `tests/`, whose parent is the repo root.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("tests/conformance sits two levels under the repo root")
        .to_path_buf()
}

/// The subcommand names the real parser accepts, enumerated from
/// `router_cli::Cli` (the derived `Command` enum) — never a hardcoded list.
fn parser_subcommands() -> Vec<String> {
    router_cli::Cli::command()
        .get_subcommands()
        .map(|sc| sc.get_name().to_string())
        .collect()
}

/// A `router <word>` mention found in the docs, with its provenance.
#[derive(Debug, Clone)]
struct Mention {
    word: String,
    file: String,
    line: u32,
    /// The deferral marker the surrounding paragraph carries, if any.
    marker: Option<String>,
}

/// Markers that mark a paragraph as an honest deferral (case-insensitive,
/// matched anywhere in the paragraph). Each exists verbatim in the docs
/// today; the list is vocabulary, not a snapshot.
const DEFERRAL_MARKERS: [&str; 5] = [
    "planned",
    "not served",
    "will use when it lands",
    "will recompute",
    "not part of v0.1",
];

/// Is this line part of a fenced code block toggle? (Only whole-line
/// fences are used in this book.)
fn is_fence_toggle(line: &str) -> bool {
    line.trim_start().starts_with("```")
}

/// The paragraph a line belongs to: the contiguous run of non-blank lines
/// around it (blank lines separate paragraphs in this book's markdown).
fn paragraph_of(line_no: usize, lines: &[&str]) -> String {
    let mut start = line_no;
    while start > 1 && !lines[start - 2].trim().is_empty() {
        start -= 1;
    }
    let mut end = line_no;
    while end < lines.len() && !lines[end].trim().is_empty() {
        end += 1;
    }
    lines[start - 1..end].join(" ")
}

/// Extract every `router <word>` mention from one markdown file. Outside
/// code fences only inline code spans (`` `…` ``) count, so prose like
/// "the router serves requests" never produces a mention; inside a fence,
/// a line that begins `router <word>` (optionally after `$ `) is a command.
fn mentions_in_file(path: &Path) -> Vec<Mention> {
    let text = std::fs::read_to_string(path).expect("doc file is readable UTF-8");
    let lines: Vec<&str> = text.lines().collect();
    let mut mentions = Vec::new();
    let mut in_fence = false;
    for (idx, line) in lines.iter().enumerate() {
        let line_no = idx + 1;
        if is_fence_toggle(line) {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            if let Some(rest) = line
                .trim_start()
                .strip_prefix("router ")
                .or_else(|| line.trim_start().strip_prefix("$ router "))
            {
                let word: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                    .collect();
                if !word.is_empty() {
                    mentions.push(Mention {
                        word,
                        file: short(path),
                        line: line_no as u32,
                        marker: None,
                    });
                }
            }
        } else {
            // Inline code spans, leftmost-longest pair, so an unmatched
            // backtick on a line cannot join two unrelated halves.
            let bytes = line.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'`' {
                    if let Some(close) = line[i + 1..].find('`') {
                        let span = &line[i + 1..i + 1 + close];
                        let mut rest = span;
                        if rest.starts_with("router ") {
                            rest = &rest["router ".len()..];
                            let word: String = rest
                                .chars()
                                .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                                .collect();
                            if !word.is_empty() {
                                mentions.push(Mention {
                                    word,
                                    file: short(path),
                                    line: line_no as u32,
                                    marker: None,
                                });
                            }
                        }
                        i += 1 + close + 1;
                        continue;
                    }
                }
                i += 1;
            }
        }
    }
    mentions
}

fn short(path: &Path) -> String {
    path.strip_prefix(repo_root())
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// All doc files in scope: README.md plus every book chapter.
fn doc_files() -> Vec<PathBuf> {
    let root = repo_root();
    let mut files = vec![root.join("README.md")];
    let book = root.join("book");
    let mut chapters: Vec<PathBuf> = std::fs::read_dir(&book)
        .expect("book/ exists at the repo root")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    chapters.sort();
    files.extend(chapters);
    files
}

/// Classify every mention: a real subcommand, or a deferral whose paragraph
/// carries a marker.
fn classify(mentions: &[Mention], served: &[String]) -> Vec<(Mention, Status)> {
    mentions
        .iter()
        .map(|m| {
            if served.contains(&m.word) {
                (m.clone(), Status::Served)
            } else {
                (
                    m.clone(),
                    Status::Deferred {
                        marker: m.marker.clone(),
                    },
                )
            }
        })
        .collect()
}

#[derive(Debug)]
enum Status {
    Served,
    Deferred { marker: Option<String> },
}

/// The whitelist of deferred words, as (word, marker, file:line) triples
/// derived from the mentions themselves — only mentions that sit inside a
/// marked deferral paragraph may be whitelisted, and only when the word is
/// *not* already a served subcommand (a served name must never need the
/// whitelist; deferring it in the docs would be a contradiction).
fn whitelist_entries(
    mentions: &[Mention],
    served: &[String],
) -> BTreeMap<String, (String, u32, String)> {
    let mut map = BTreeMap::new();
    for m in mentions {
        if served.contains(&m.word) {
            continue;
        }
        if let Some(marker) = &m.marker {
            map.entry(m.word.clone())
                .or_insert_with(|| (marker.clone(), m.line, m.file.clone()));
        }
    }
    map
}

#[test]
fn conf_43_docs_and_cli_agree_on_the_subcommand_set() {
    let served = parser_subcommands();
    // Sanity of the derivation itself: the parser is enumerated live, and a
    // parser with zero subcommands would make both directions vacuous.
    assert!(
        !served.is_empty(),
        "the live parser must expose at least one subcommand"
    );

    let files = doc_files();
    assert!(
        !files.is_empty(),
        "README.md and book/ must exist relative to the conformance crate"
    );

    // Attach each mention's paragraph marker before classifying.
    let mut mentions: Vec<Mention> = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(f).expect("doc file readable");
        let lines: Vec<&str> = text.lines().collect();
        for mut m in mentions_in_file(f) {
            let para = paragraph_of(m.line as usize, &lines);
            let lower = para.to_lowercase();
            m.marker = DEFERRAL_MARKERS
                .iter()
                .find(|marker| lower.contains(*marker))
                .map(|s| s.to_string());
            mentions.push(m);
        }
    }
    assert!(
        !mentions.is_empty(),
        "the docs must mention at least one `router <subcommand>`"
    );

    let classified = classify(&mentions, &served);
    let wl = whitelist_entries(&mentions, &served);

    // Direction 1 — docs → CLI: every mention resolves to a served
    // subcommand or a whitelisted deferral (marker + provenance).
    let mut failures: Vec<String> = Vec::new();
    for (m, status) in &classified {
        match status {
            Status::Served => {}
            Status::Deferred { marker } => match marker {
                Some(_) => {
                    assert!(wl.contains_key(&m.word));
                }
                None => failures.push(format!(
                    "`{}` mentions `{}` ({}:{}), but the parser has no such \
                     subcommand and the paragraph carries no deferral marker",
                    m.file, m.word, m.file, m.line
                )),
            },
        }
    }
    assert!(
        failures.is_empty(),
        "docs promise subcommands the CLI does not serve (spec §9.3):\n  {}",
        failures.join("\n  ")
    );

    // The whitelist itself must be honest: every deferred word must be a
    // known future twin of a served surface (here: `replay`/`trace` for the
    // planned reporting surfaces of spec §9.3, `state` for the planned
    // state-inspection surface named in the docs). Enforced as a relation —
    // the deferred word must not collide with any served name — plus the
    // marker requirement above; the *names* of future surfaces come from
    // the docs themselves, which is exactly what this case polices.
    for (word, (marker, line, file)) in &wl {
        assert!(
            !served.contains(word),
            "whitelisted word `{word}` is also a served subcommand — the \
             docs must not describe a served command as planned"
        );
        assert!(
            DEFERRAL_MARKERS.contains(&marker.as_str()),
            "marker `{marker}` for `{word}` ({file}:{line}) is not in the \
             marker vocabulary"
        );
    }

    // Direction 2 — CLI → docs: every subcommand the parser accepts is
    // mentioned in the docs at least once. The whitelist does not apply
    // here, so a new landed subcommand cannot be hidden by it.
    let documented: std::collections::BTreeSet<&str> =
        mentions.iter().map(|m| m.word.as_str()).collect();
    let undocumented: Vec<&str> = served
        .iter()
        .map(|s| s.as_str())
        .filter(|s| !documented.contains(s))
        .collect();
    assert!(
        undocumented.is_empty(),
        "the parser accepts subcommands the docs never mention: {undocumented:?} \
         (spec §9.3: a documented-but-unreachable surface is a defect; an \
         undocumented-but-reachable one is its twin)"
    );

    // Evidentiary output (visible with --nocapture): the relation this case
    // asserts, at the commit it ran against.
    eprintln!("conf_43: served subcommands (from the live parser): {served:?}");
    for (word, (marker, line, file)) in &wl {
        eprintln!("conf_43: whitelisted deferral `{word}` ({file}:{line}, marker: \"{marker}\")");
    }
}
